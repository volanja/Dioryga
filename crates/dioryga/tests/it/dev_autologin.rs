//! 開発専用の自動ログインの結合テスト（#128）。
//!
//! **`dev-autologin` 機能を有効にしたときだけコンパイルされる。**
//!
//! ```bash
//! cargo test --features dioryga/dev-autologin --test it dev_autologin::
//! ```
//!
//! 確かめること：
//!
//! - Cookieを持たないリクエストが、指定した利用者として通る
//! - **フォームを送れる**（CSRFトークンが、差し込んだセッションから導出される）
//! - ログアウトしても、次のリクエストで作り直される
//! - **手でログインした別の利用者のCookieは尊重する**
//! - 存在しない・無効化された利用者を指定すると、起動を止める
#![cfg(feature = "dev-autologin")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use dioryga::auth::dev_autologin::DevAutologin;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::app_user;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use tower::ServiceExt;

/// Cookieを持たないリクエストが、指定した利用者として通ること。
async fn cookieが無くても指定した利用者として通る(db: &DatabaseConnection) {
    利用者(db, "yarigatake@example.invalid", "槍ヶ岳 大川", false).await;
    let state = 状態(db).await;
    let app = 自動ログイン(&state, "yarigatake_example.invalid").await;

    let (status, body, _) = 取得(app, "/projects", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("槍ヶ岳 大川"),
        "指定した利用者として表示されていません"
    );
}

/// **フォームを送れること。**CSRFトークンは差し込んだセッションから導出される。
///
/// 送れなければ、画面の確認がGETだけに限られる。ログアウトで確かめ、
/// **次のリクエストで作り直される**ことも同時に見る。
async fn フォームを送れてログアウト後も作り直される(db: &DatabaseConnection) {
    利用者(db, "hotaka@example.invalid", "穂高 古城", false).await;
    let state = 状態(db).await;
    let app = 自動ログイン(&state, "hotaka_example.invalid").await;

    let (_, body, _) = 取得(app.clone(), "/projects", None).await;
    let csrf = csrfトークン(&body);

    let status = 送信(app.clone(), "/logout", &csrf).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "CSRFトークンが通りません");

    let (status, body, _) = 取得(app, "/projects", None).await;
    assert_eq!(status, StatusCode::OK, "ログアウト後に作り直されていません");
    assert!(body.contains("穂高 古城"));
}

/// **手でログインした別の利用者のCookieは尊重すること。**
///
/// 上書きすると、自動ログイン中に他の役割の画面を確かめられない。
async fn 有効なcookieがあればその利用者を優先する(db: &DatabaseConnection) {
    利用者(db, "hakuba@example.invalid", "白馬 石垣", false).await;
    let 別 = 利用者(db, "tateyama@example.invalid", "立山 滝見", false).await;
    let state = 状態(db).await;
    let app = 自動ログイン(&state, "hakuba_example.invalid").await;

    let (_, token) = session::create(
        db,
        別.id,
        "127.0.0.1",
        "test",
        &state.config.session,
        Utc::now(),
    )
    .await
    .unwrap();

    let (status, body, _) = 取得(app, "/projects", Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("立山 滝見"),
        "手でログインした利用者が上書きされました"
    );
    assert!(!body.contains("白馬 石垣"));
}

/// **存在しない・無効化された利用者を指定したら起動を止めること。**
///
/// 黙ってログイン画面に落ちると、指定を誤ったことに気付けない。
async fn 指定を誤ったら起動を止める(db: &DatabaseConnection) {
    let state = 状態(db).await;
    assert!(DevAutologin::start(&state, "nobody_example.invalid")
        .await
        .is_err());

    利用者(db, "yatsugatake@example.invalid", "八ヶ岳 天守", true).await;
    assert!(DevAutologin::start(&state, "yatsugatake_example.invalid")
        .await
        .is_err());
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 自動ログイン(state: &AppState, email: &str) -> Router {
    DevAutologin::start(state, email)
        .await
        .unwrap()
        .apply(router(state.clone()))
}

async fn 状態(db: &DatabaseConnection) -> AppState {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: Default::default(),
    }
}

async fn 取得(
    app: Router,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, String, Option<String>) {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::COOKIE, format!("{}={token}", session::COOKIE_NAME));
    }
    let res = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let location = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        location,
    )
}

async fn 送信(app: Router, uri: &str, csrf: &str) -> StatusCode {
    let body = format!("{}={csrf}", dioryga::auth::csrf::FIELD_NAME);
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap(),
    )
    .await
    .unwrap()
    .status()
}

/// 画面に埋め込まれたCSRFトークンを取り出す。
fn csrfトークン(body: &str) -> String {
    let needle = r#"name="csrf_token" value=""#;
    let start = body.find(needle).expect("CSRFトークンがありません") + needle.len();
    let end = body[start..].find('"').unwrap();
    body[start..start + end].to_owned()
}

async fn 利用者(
    db: &DatabaseConnection,
    email: &str,
    name: &str,
    disabled: bool,
) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(name.to_owned()),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set("ja".to_owned()),
        last_login_at: Set(None),
        disabled_at: Set(disabled.then(Utc::now)),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, cookieが無くても指定した利用者として通る);
        全検証!(@one $用意, $属性, フォームを送れてログアウト後も作り直される);
        全検証!(@one $用意, $属性, 有効なcookieがあればその利用者を優先する);
        全検証!(@one $用意, $属性, 指定を誤ったら起動を止める);
    };
    (@one $用意:path, $属性:meta, $名前:ident) => {
        #[tokio::test]
        #[$属性]
        async fn $名前() {
            let db = $用意().await;
            super::$名前(&db.conn).await;
        }
    };
}

mod sqlite {
    全検証!(crate::support::sqlite, cfg(all()));
}

mod postgres {
    全検証!(
        crate::support::postgres,
        ignore = "Dockerが必要。cargo test -- --ignored で実行する"
    );
}
