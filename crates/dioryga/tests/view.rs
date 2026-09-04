//! 画面（テンプレート・i18n・静的アセット）の結合テスト。

mod support;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::app_user;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use std::sync::Arc;
use tower::ServiceExt;

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

async fn 状態(db: &DatabaseConnection) -> AppState {
    let config = 設定();
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: Default::default(),
    }
}

async fn 本文(db: &DatabaseConnection, uri: &str, accept_language: &str) -> (StatusCode, String) {
    let res = router(状態(db).await)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::ACCEPT_LANGUAGE, accept_language)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ---------------------------------------------------------------------------

/// ログイン画面が描画されること。
async fn ログイン画面が描画される(db: &DatabaseConnection) {
    let _ = 利用者(db).await;
    let (status, body) = 本文(db, "/login", "ja").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<!doctype html>"));
    assert!(body.contains("ログイン"));
    assert!(
        body.contains("/assets/dioryga.css"),
        "CSSが読み込まれていません"
    );
}

/// 表示言語がAccept-Languageで切り替わること（設計書16.5）。
async fn 表示言語が切り替わる(db: &DatabaseConnection) {
    let _ = 利用者(db).await;

    let (_, 日本語) = 本文(db, "/login", "ja").await;
    assert!(日本語.contains("メールアドレス"));
    assert!(日本語.contains(r#"lang="ja""#));

    let (_, 英語) = 本文(db, "/login", "en-US,en;q=0.9").await;
    assert!(英語.contains("Email"));
    assert!(英語.contains(r#"lang="en""#));
    assert!(!英語.contains("メールアドレス"));
}

/// **翻訳キーがそのまま画面に出ていないこと。**
///
/// 翻訳ファイルの形式を誤ると `login.title` のようなキーが表示されるが、
/// 画面は描画されるため見落としやすい。
async fn 翻訳キーが露出しない(db: &DatabaseConnection) {
    let _ = 利用者(db).await;

    for lang in ["ja", "en"] {
        let (_, body) = 本文(db, "/login", lang).await;
        for キー in ["login.title", "login.email", "app.name"] {
            assert!(
                !body.contains(キー),
                "翻訳されず「{キー}」がそのまま出ています（lang={lang}）"
            );
        }
    }
}

/// 静的アセットがバイナリから配信されること（設計書2章）。
async fn 静的アセットが配信される(db: &DatabaseConnection) {
    let _ = 利用者(db).await;
    let (status, body) = 本文(db, "/assets/dioryga.css", "ja").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("--accent"), "CSSの中身が返っていません");
}

/// 静的アセットが認証で弾かれないこと。
///
/// 弾かれるとログイン画面が素のHTMLになる。
async fn 静的アセットは認証不要(db: &DatabaseConnection) {
    let _ = 利用者(db).await;
    let (status, _) = 本文(db, "/assets/dioryga.css", "ja").await;
    assert_eq!(status, StatusCode::OK, "CSSが認証で弾かれています");
}

/// 存在しないアセットは404であること。
async fn 存在しないアセットは404(db: &DatabaseConnection) {
    let _ = 利用者(db).await;
    let (status, _) = 本文(db, "/assets/none.css", "ja").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// セットアップ画面が描画されること（利用者0件のとき）。
async fn セットアップ画面が描画される(db: &DatabaseConnection) {
    let (status, body) = 本文(db, "/setup", "ja").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("初回セットアップ"));
    assert!(body.contains("セットアップトークン"));
}

// ---------------------------------------------------------------------------

async fn 利用者(db: &DatabaseConnection) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証用".to_owned()),
        email: Set("view@example.com".to_owned()),
        password_hash: Set("dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set("ja".to_owned()),
        last_login_at: Set(None),
        disabled_at: Set(None),
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
        全検証!(@one $用意, $属性, ログイン画面が描画される);
        全検証!(@one $用意, $属性, 表示言語が切り替わる);
        全検証!(@one $用意, $属性, 翻訳キーが露出しない);
        全検証!(@one $用意, $属性, 静的アセットが配信される);
        全検証!(@one $用意, $属性, 静的アセットは認証不要);
        全検証!(@one $用意, $属性, 存在しないアセットは404);
        全検証!(@one $用意, $属性, セットアップ画面が描画される);
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
