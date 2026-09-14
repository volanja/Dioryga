//! 開発専用の自動ログイン（#128）。
//!
//! **`dev-autologin` 機能を有効にしたビルドにしか存在しない。**配布物には入らない。
//!
//! # 何のためか
//!
//! 画面の見た目を、開発中に機械的に確かめるため。確かめる側（ブラウザを操作する
//! ツール）にパスワードやトークンを入力させない。
//!
//! # 仕組み
//!
//! 起動時に、環境変数で指定された利用者のセッションを作る。**有効なセッション
//! Cookieを持たないリクエストに、そのCookieを差し込む。**
//!
//! 認証・CSRF・System Adminガードは、手でログインしたときと同じ経路を通る。
//! **認証の中に自動ログインのための分岐を作らない**——分岐を作ると、配布物と
//! 開発時とで認証の経路が別物になり、開発時の確認が配布物の確認にならない。
//!
//! - **生のトークンはサーバの外に出さない。**`Set-Cookie` で返さず、リクエストの
//!   ヘッダーに差し込むだけにする
//! - 有効なCookieを持つリクエスト（手で別の利用者としてログインした場合）は、
//!   そのまま通す
//! - ログアウトなどでセッションが失効したら、次のリクエストで作り直す
//!
//! # 使い方
//!
//! ```bash
//! DIORYGA_DEV_AUTOLOGIN=利用者のユーザー名 cargo run --features dev-autologin
//! ```
//!
//! 環境変数を設定しなければ、機能を有効にしたビルドでも自動ログインしない。
//!
//! # リリースに入らないことの検査
//!
//! 有効なときは起動時に [`MARKER`] を含む警告を出す。CIは配布物のバイナリに
//! この文字列が含まれないことを確かめる。

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use axum::Router;
use chrono::{DateTime, Utc};
use entity::app_user;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tokio::sync::Mutex;

use crate::auth::{cookie, session};
use crate::server::AppState;

/// 自動ログインする利用者のユーザー名を指定する環境変数。
pub const ENV_VAR: &str = "DIORYGA_DEV_AUTOLOGIN";

/// **CIが、配布物のバイナリに含まれていないことを検査する目印。**
/// 変えるときは `.github/workflows/ci.yml` も直す。
pub const MARKER: &str = "DIORYGA-DEV-AUTOLOGIN-ENABLED";

/// 自動ログインの状態。
#[derive(Clone)]
pub struct DevAutologin {
    state: AppState,
    user_id: i32,
    /// 差し込んでいるセッションの生のトークン。サーバの外には出さない。
    token: Arc<Mutex<Option<String>>>,
}

impl DevAutologin {
    /// 環境変数が設定されていれば、自動ログインを始める。
    pub async fn from_env(state: &AppState) -> anyhow::Result<Option<Self>> {
        let Ok(username) = std::env::var(ENV_VAR) else {
            return Ok(None);
        };
        let username = username.trim();
        if username.is_empty() {
            return Ok(None);
        }
        Self::start(state, username).await.map(Some)
    }

    /// 指定した利用者として自動ログインを始める。
    ///
    /// **利用者が存在しない・無効化されている場合は起動を止める。**黙って
    /// ログイン画面に落ちると、指定を誤ったことに気付けない。
    pub async fn start(state: &AppState, username: &str) -> anyhow::Result<Self> {
        let username = crate::auth::username::正規化する(username);
        let user = app_user::Entity::find()
            .filter(app_user::Column::Username.eq(&username))
            .one(&state.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("{ENV_VAR} の利用者「{username}」が見つかりません"))?;
        if user.disabled_at.is_some() {
            anyhow::bail!("{ENV_VAR} の利用者「{username}」は無効化されています");
        }

        tracing::warn!(
            marker = MARKER,
            username,
            "開発専用の自動ログインが有効です。有効なCookieを持たないリクエストは、すべてこの利用者として扱います"
        );

        Ok(Self {
            state: state.clone(),
            user_id: user.id,
            token: Arc::new(Mutex::new(None)),
        })
    }

    /// ルータの**最も外側**に差し込む。認証より先に Cookie を整える必要がある。
    pub fn apply(self, router: Router) -> Router {
        router.layer(axum::middleware::from_fn(move |request, next| {
            let this = self.clone();
            async move { this.inject(request, next).await }
        }))
    }

    async fn inject(&self, mut request: Request, next: Next) -> Response {
        let now = Utc::now();

        let existing = request
            .headers()
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(cookie::extract)
            .map(str::to_owned);
        if let Some(token) = &existing {
            // 手でログインしたセッションは尊重する
            if self.valid(token, now).await {
                return next.run(request).await;
            }
        }

        match self.token(now).await {
            Ok(token) => {
                let value = format!("{}={token}", session::COOKIE_NAME);
                if let Ok(value) = HeaderValue::from_str(&value) {
                    request.headers_mut().insert(header::COOKIE, value);
                }
            }
            // 差し込めなければ通常どおりログイン画面に回る
            Err(e) => tracing::error!(error = %e, "自動ログインのセッションを作れませんでした"),
        }
        next.run(request).await
    }

    async fn valid(&self, token: &str, now: DateTime<Utc>) -> bool {
        matches!(
            session::validate(&self.state.db, token, &self.state.config.session, now).await,
            Ok(Some(_))
        )
    }

    /// 有効なセッションのトークン。**失効していれば作り直す**（ログアウト後など）。
    async fn token(&self, now: DateTime<Utc>) -> anyhow::Result<String> {
        let mut held = self.token.lock().await;
        if let Some(token) = held.as_deref() {
            if self.valid(token, now).await {
                return Ok(token.to_owned());
            }
        }
        let (_, raw) = session::create(
            &self.state.db,
            self.user_id,
            "127.0.0.1",
            "dev-autologin",
            &self.state.config.session,
            now,
        )
        .await?;
        let token = raw.as_str().to_owned();
        *held = Some(token.clone());
        Ok(token)
    }
}
