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

/// **タイムゾーンを保存でき、空は「既定」、語彙外は拒否されること**（設計書24.2.3、#210）。
async fn タイムゾーンを保存できる(db: &DatabaseConnection) {
    let user = 利用者(db, "acc-tz", "ja", "system").await;
    let 送る = |tz: &'static str| [("locale", "ja"), ("theme", "system"), ("timezone", tz)];

    let (status, _) = 送信(db, &user, &送る("America/New_York")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        読み直す(db, user.id).await.timezone.as_deref(),
        Some("America/New_York")
    );

    // 語彙外は既定へ倒さず拒否し、保存済みの値を変えない（Q-21）
    let (status, _) = 送信(db, &user, &送る("Asia/Tokio")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        読み直す(db, user.id).await.timezone.as_deref(),
        Some("America/New_York")
    );

    // 空は「サーバーの既定に従う」
    送信(db, &user, &送る("")).await;
    assert_eq!(読み直す(db, user.id).await.timezone, None);
}

/// **同じ日時が、見る人のタイムゾーンで表示されること**（24.2.3、#210）。
///
/// 未設定なら設定ファイルの `timezone`（既定は Asia/Tokyo）に従う。
async fn 日時は利用者のタイムゾーンで表示される(db: &DatabaseConnection) {
    let admin = app_user::ActiveModel {
        is_system_admin: Set(true),
        ..下書き("acc-tz-admin", "ja", "system")
    }
    .insert(db)
    .await
    .unwrap();
    // UTCの 2026-03-31 15:00 は、東京では 4月1日 0:00
    app_user::ActiveModel {
        last_login_at: Set(Some("2026-03-31T15:00:00Z".parse().unwrap())),
        ..下書き("acc-tz-seen", "ja", "system")
    }
    .insert(db)
    .await
    .unwrap();

    let body = 開く(db, &admin, "/admin/users").await;
    assert!(body.contains("2026-04-01 00:00"), "{body}");

    let mut active: app_user::ActiveModel = admin.clone().into();
    active.timezone = Set(Some("UTC".to_owned()));
    let admin = active.update(db).await.unwrap();
    let body = 開く(db, &admin, "/admin/users").await;
    assert!(body.contains("2026-03-31 15:00"), "{body}");
}

/// **パスワードの規則の誤りは、利用者の言語で出ること**（#191）。
///
/// 誤りの文言はコンソールと共有している。コンソールの言語（OSのロケール）で
/// 画面に出してはならない。
async fn パスワードの誤りは利用者の言語で出る(db: &DatabaseConnection) {
    let hash = 状態(db)
        .await
        .passwords
        .hash("Current-Pass-4821")
        .await
        .unwrap();
    for (username, locale, 期待) in [
        ("acc-pw-en", "en", "Passwords must be at least"),
        ("acc-pw-ja", "ja", "パスワードは"),
    ] {
        let user = app_user::ActiveModel {
            password_hash: Set(hash.clone()),
            ..下書き(username, locale, "system")
        }
        .insert(db)
        .await
        .unwrap();
        let (_, body) = 送信先(
            db,
            &user,
            "/account/password",
            &[
                ("current_password", "Current-Pass-4821"),
                ("new_password", "short"),
                ("confirm_password", "short"),
            ],
        )
        .await;
        assert!(body.contains(期待), "{locale}: {body}");
    }
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
    送信先(db, user, "/account/display", fields).await
}

async fn 送信先(
    db: &DatabaseConnection,
    user: &app_user::Model,
    uri: &str,
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
                .uri(uri)
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
        全検証!(@one $用意, $属性, タイムゾーンを保存できる);
        全検証!(@one $用意, $属性, パスワードの誤りは利用者の言語で出る);
        全検証!(@one $用意, $属性, 日時は利用者のタイムゾーンで表示される);
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
