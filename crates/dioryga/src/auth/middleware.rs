//! 認証・認可のミドルウェア（設計書20.10）。
//!
//! 2層に分ける。
//!
//! ```text
//! ① 認証：Cookie → セッション → User を解決する
//! ② 認可：User + 対象 → 許可 / 拒否
//! ```
//!
//! **System Adminガードは②の中でも独立した層として置く。**ロールの強弱の判定と
//! 混ぜると、ロール判定の変更が意図せず「System Adminはプロジェクトデータに
//! 触れない」という制約（3章）を壊しうるため。

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use chrono::Utc;
use entity::app_user;
use sea_orm::EntityTrait;

use crate::auth::{authorization, cookie, csrf, session};
use crate::server::AppState;

/// 認証済みの利用者。ハンドラからは拡張として取り出す。
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub user: app_user::Model,
    pub session_id: i32,
    /// このセッションに対応するCSRFトークン。フォームへ埋め込む。
    pub csrf_token: String,
}

/// 認証を要求しない経路。
///
/// **許可リスト方式にしている**（設計書20.10）。新しい画面を追加したときに
/// 保護し忘れる事故を防ぐため、ここに書いたもの以外はすべて認証必須になる。
fn is_public(path: &str) -> bool {
    matches!(path, "/login" | "/setup" | "/health")
}

/// パスワード変更を強制されている間でも通す経路。
fn is_allowed_while_password_change_required(path: &str) -> bool {
    matches!(path, "/account/password" | "/logout" | "/health")
}

/// ① 認証。
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();

    let raw_token = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(cookie::extract)
        .map(str::to_owned);

    let now = Utc::now();
    let current = match &raw_token {
        Some(token) => resolve(&state, token, now).await,
        None => None,
    };

    match current {
        Some(current) => {
            // パスワードの変更を終えるまで他の画面へ進ませない（設計書20.6）
            if current.user.must_change_password
                && !is_allowed_while_password_change_required(&path)
            {
                return Redirect::to("/account/password").into_response();
            }

            request.extensions_mut().insert(current);
            next.run(request).await
        }
        None if is_public(&path) => next.run(request).await,
        None => Redirect::to("/login").into_response(),
    }
}

/// セッションを検証し、利用者を解決する。
async fn resolve(
    state: &AppState,
    raw_token: &str,
    now: chrono::DateTime<Utc>,
) -> Option<CurrentUser> {
    let session = session::validate(&state.db, raw_token, &state.config.session, now)
        .await
        .ok()
        .flatten()?;

    let user = app_user::Entity::find_by_id(session.user_id)
        .one(&state.db)
        .await
        .ok()
        .flatten()?;

    // 無効化された利用者のセッションは通さない（設計書20.6）
    if user.disabled_at.is_some() {
        return None;
    }

    // アイドルタイムアウトの起点を更新する。
    // セッションは監査ログの対象外（設計書24.4）。
    let session_id = session.id;
    let _ = session::touch(&state.db, session, now).await;

    Some(CurrentUser {
        csrf_token: csrf::derive(raw_token),
        user,
        session_id,
    })
}

/// ② 認可：System Adminガード。
///
/// `is_system_admin=true` の利用者による `/projects/**` へのアクセスを、
/// **ロールの強弱に無関係に拒否する**（設計書3章）。
pub async fn system_admin_guard(request: Request, next: Next) -> Response {
    let path = request.uri().path();

    if path == "/projects" || path.starts_with("/projects/") {
        if let Some(current) = request.extensions().get::<CurrentUser>() {
            if authorization::deny_system_admin(&current.user).is_err() {
                return (
                    StatusCode::FORBIDDEN,
                    "System Adminはプロジェクト内のデータにアクセスできません",
                )
                    .into_response();
            }
        }
    }

    next.run(request).await
}

/// CSRFトークンの検証。
///
/// `SameSite=Lax` だけに頼らず、**状態変更を伴うリクエストにはトークンを要求する**
/// （設計書20.5）。
pub async fn verify_csrf(request: Request, next: Next) -> Response {
    let 状態変更 = matches!(
        request.method(),
        &axum::http::Method::POST
            | &axum::http::Method::PUT
            | &axum::http::Method::PATCH
            | &axum::http::Method::DELETE
    );

    if !状態変更 {
        return next.run(request).await;
    }

    // 未認証の経路（ログイン・セットアップ）は、そもそもセッションが無いため
    // CSRFトークンを持ちようがない。SameSite=Lax と、これらが認証情報を
    // 要求すること自体で守る。
    let Some(current) = request.extensions().get::<CurrentUser>().cloned() else {
        return next.run(request).await;
    };

    let provided = request
        .headers()
        .get(csrf::HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();

    if csrf::verify(&current.csrf_token, &provided) {
        return next.run(request).await;
    }

    // ヘッダーに無ければフォーム本文を見る必要があるが、本文の読み取りは
    // ハンドラ側の抽出と競合する。フォームからの送信は htmx の hx-headers で
    // ヘッダーに載せる方針とし（設計書20.5）、ここではヘッダーのみを見る。
    (StatusCode::FORBIDDEN, "CSRFトークンが不正です").into_response()
}
