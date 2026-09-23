//! ログイン画面の結合テスト（設計書20.1、20.6、#136）。
//!
//! 確かめること：
//!
//! - **ユーザー名でログインでき、大文字で入力しても同じ利用者になる**
//! - メールアドレスはログインIDではない
//! - 存在しないユーザー名とパスワード誤りで、文言が同じ（利用者の列挙を防ぐ）
//! - 待ち時間中は断り、**あと何秒待てばよいか**を出す

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::rate_limit;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::app_user;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use tower::ServiceExt;

const パスワード: &str = "Tanigawa-Bridge-7391";
const 失敗の文言: &str = "ユーザー名またはパスワードが正しくありません";

/// ユーザー名でログインできること。**大文字で入力しても同じ利用者になる**（20.1）。
async fn 大文字で入力してもログインできる(db: &DatabaseConnection) {
    let state = 状態(db).await;
    利用者(db, &state, "hotaka.kojo", Some("hotaka@example.invalid")).await;

    let (status, location, _) = ログイン(&state, "Hotaka.Kojo", パスワード).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "ログインできません");
    assert_eq!(location.as_deref(), Some("/"));
}

/// **メールアドレスではログインできないこと**（ログインIDではない、20.1）。
async fn メールアドレスではログインできない(db: &DatabaseConnection) {
    let state = 状態(db).await;
    利用者(db, &state, "yarigatake", Some("yarigatake@example.invalid")).await;

    let (status, _, body) = ログイン(&state, "yarigatake@example.invalid", パスワード).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(失敗の文言));
}

/// **存在しないユーザー名とパスワード誤りで、文言が同じであること**（20.6）。
async fn 失敗の文言は存在の有無で変わらない(db: &DatabaseConnection) {
    let state = 状態(db).await;
    利用者(db, &state, "hakuba", None).await;

    let (_, _, 誤り) = ログイン(&state, "hakuba", "wrong-password-0000").await;
    let (_, _, 不在) = ログイン(&state, "nobody", "wrong-password-0000").await;

    assert!(誤り.contains(失敗の文言));
    assert!(不在.contains(失敗の文言));
}

/// **待ち時間中は断り、残り秒数を出すこと**（20.6）。
///
/// 正しいパスワードでも断る。待ち時間は推測の速度を抑えるためのもので、
/// 正しいときだけ通すと、推測が当たったかどうかが応答で分かってしまう。
async fn 待ち時間中は残り秒数を出す(db: &DatabaseConnection) {
    let state = 状態(db).await;
    利用者(db, &state, "tateyama", None).await;

    let now = Utc::now();
    for _ in 0..(rate_limit::FREE_FAILURES + 1) {
        rate_limit::record(db, "tateyama", "198.51.100.1", false, now)
            .await
            .unwrap();
    }

    let (status, _, body) = ログイン(&state, "TATEYAMA", パスワード).await;
    assert_eq!(status, StatusCode::OK, "待ち時間中なのにログインできました");
    assert!(body.contains("秒後に再度お試しください"), "{body}");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

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

/// パスワードを実際にハッシュ化して利用者を作る。
async fn 利用者(
    db: &DatabaseConnection,
    state: &AppState,
    username: &str,
    email: Option<&str>,
) -> app_user::Model {
    let hash = state.passwords.hash(パスワード).await.unwrap();
    app_user::ActiveModel {
        name: Set(format!("{username} の表示名")),
        username: Set(username.to_owned()),
        email: Set(email.map(str::to_owned)),
        password_hash: Set(hash),
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

/// ログインを送る。ログインは接続元IPを要するため `MockConnectInfo` を差し込む。
async fn ログイン(
    state: &AppState,
    username: &str,
    password: &str,
) -> (StatusCode, Option<String>, String) {
    // 利用者を作った後で状態を読み直す（初回セットアップへの誘導を外すため）
    let (setup, _) = SetupState::initialize(&state.db).await.unwrap();
    let state = AppState {
        setup,
        ..state.clone()
    };
    let app: Router = router(state).layer(MockConnectInfo(SocketAddr::from((
        [198, 51, 100, 1],
        40000,
    ))));

    let body =
        serde_urlencoded::to_string([("username", username), ("password", password)]).unwrap();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
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
        location,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 大文字で入力してもログインできる);
        全検証!(@one $用意, $属性, メールアドレスではログインできない);
        全検証!(@one $用意, $属性, 失敗の文言は存在の有無で変わらない);
        全検証!(@one $用意, $属性, 待ち時間中は残り秒数を出す);
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
