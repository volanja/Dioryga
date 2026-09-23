//! 個人設定の結合テスト（#120。設計書16.4、16.5）。
//!
//! 言語と表示モードは利用者（`USER`）に持つ。**端末をまたいで同じになる**
//! ——同じ個人設定にある2つの項目が、別々の場所に保存されていると説明できない。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::app_user;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use std::sync::Arc;
use tower::ServiceExt;

/// **言語を変えると、その場から表示が切り替わること**（設計書16.5）。
async fn 言語を変えると表示が切り替わる(db: &DatabaseConnection) {
    let user = 利用者(db, "acc-locale", "ja", "system").await;

    let (status, body) = 送信(db, &user, &[("locale", "en"), ("theme", "system")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Display"), "{body}");
    assert!(body.contains("Saved"), "保存した知らせが出ていません");

    // 保存されており、次の画面も英語になる
    assert_eq!(読み直す(db, user.id).await.locale, "en");
    let body = 開く(db, &読み直す(db, user.id).await, "/projects").await;
    assert!(body.contains("Projects"), "{body}");
}

/// **表示モードを選ぶと `data-theme` が付き、OSに従うときは付かないこと。**
///
/// 属性を置かない形にしているのは、`prefers-color-scheme` にそのまま任せるため。
async fn 表示モードは属性で切り替える(db: &DatabaseConnection) {
    let user = 利用者(db, "acc-theme", "ja", "system").await;

    let body = 開く(db, &user, "/account/display").await;
    assert!(
        !body.contains("data-theme"),
        "OSに従うのに属性が付いています"
    );

    送信(db, &user, &[("locale", "ja"), ("theme", "dark")]).await;
    let user = 読み直す(db, user.id).await;
    assert_eq!(user.theme, "dark");
    let body = 開く(db, &user, "/projects").await;
    assert!(body.contains(r#"data-theme="dark""#), "{body}");

    送信(db, &user, &[("locale", "ja"), ("theme", "light")]).await;
    let user = 読み直す(db, user.id).await;
    let body = 開く(db, &user, "/projects").await;
    assert!(body.contains(r#"data-theme="light""#), "{body}");

    送信(db, &user, &[("locale", "ja"), ("theme", "system")]).await;
    let user = 読み直す(db, user.id).await;
    let body = 開く(db, &user, "/projects").await;
    assert!(!body.contains("data-theme"), "{body}");
}

/// **語彙外は既定へ倒さず拒否すること**（設計書8.6、Q-21）。
async fn 語彙外の値は拒否される(db: &DatabaseConnection) {
    let user = 利用者(db, "acc-invalid", "ja", "dark").await;

    for fields in [
        [("locale", "fr"), ("theme", "dark")],
        [("locale", "ja"), ("theme", "でたらめ")],
    ] {
        let (status, _) = 送信(db, &user, &fields).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{fields:?}");
    }

    let 後 = 読み直す(db, user.id).await;
    assert_eq!((後.locale.as_str(), 後.theme.as_str()), ("ja", "dark"));
}

/// 強制変更の画面にも表示モードが効くこと（メニューは出さない、#122）。
async fn 強制変更の画面にも表示モードが効く(db: &DatabaseConnection) {
    let user = app_user::ActiveModel {
        must_change_password: Set(true),
        ..下書き("acc-forced", "ja", "dark")
    }
    .insert(db)
    .await
    .unwrap();

    let body = 開く(db, &user, "/account/password").await;
    assert!(body.contains(r#"data-theme="dark""#), "{body}");
    assert!(!body.contains("sidenav"), "{body}");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

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

async fn 合言葉(db: &DatabaseConnection, state: &AppState, user_id: i32) -> String {
    let (_, token) = session::create(
        db,
        user_id,
        "127.0.0.1",
        "test",
        &state.config.session,
        Utc::now(),
    )
    .await
    .unwrap();
    token.as_str().to_owned()
}

async fn 開く(db: &DatabaseConnection, user: &app_user::Model, uri: &str) -> String {
    let state = 状態(db).await;
    let token = 合言葉(db, &state, user.id).await;
    let res = router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, format!("{}={token}", session::COOKIE_NAME))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{uri}");
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn 送信(
    db: &DatabaseConnection,
    user: &app_user::Model,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    let state = 状態(db).await;
    let token = 合言葉(db, &state, user.id).await;
    let csrf = dioryga::auth::csrf::derive(&token);
    let mut pairs: Vec<(String, String)> = vec![(dioryga::auth::csrf::FIELD_NAME.to_owned(), csrf)];
    for (key, value) in fields {
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }
    let body = serde_urlencoded::to_string(&pairs).unwrap();

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/account/display")
                .header(header::COOKIE, format!("{}={token}", session::COOKIE_NAME))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
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

async fn 読み直す(db: &DatabaseConnection, id: i32) -> app_user::Model {
    app_user::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

fn 下書き(username: &str, locale: &str, theme: &str) -> app_user::ActiveModel {
    app_user::ActiveModel {
        name: Set("検証".to_owned()),
        username: Set(username.to_owned()),
        email: Set(None),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set(locale.to_owned()),
        theme: Set(theme.to_owned()),
        last_login_at: Set(None),
        disabled_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
}

async fn 利用者(
    db: &DatabaseConnection,
    username: &str,
    locale: &str,
    theme: &str,
) -> app_user::Model {
    下書き(username, locale, theme).insert(db).await.unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 言語を変えると表示が切り替わる);
        全検証!(@one $用意, $属性, 表示モードは属性で切り替える);
        全検証!(@one $用意, $属性, 語彙外の値は拒否される);
        全検証!(@one $用意, $属性, 強制変更の画面にも表示モードが効く);
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
