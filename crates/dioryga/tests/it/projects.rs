//! プロジェクト一覧の結合テスト（設計書16.1のB領域、#121）。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, project, project_member};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use std::sync::Arc;
use tower::ServiceExt;

/// **プロジェクト名を押して開けること**（#121）。
///
/// 入口はダッシュボード（設計書16.1）。名前がリンクになったので、右端の
/// 「開く」は置かない——同じ行から同じ画面へ2つのリンクを出さない。
async fn 名前を押して開ける(db: &DatabaseConnection) {
    let user = 利用者(db, "pj-link").await;
    let p = プロジェクト(db, "カタクリ基盤更改", false).await;
    メンバー(db, user.id, p.id, "Operator").await;

    let body = 開く(db, &user, "/projects").await;
    let 本文 = 本文だけ(&body);
    assert!(
        本文.contains(&format!(
            r#"<a href="/projects/{}">カタクリ基盤更改</a>"#,
            p.id
        )),
        "{本文}"
    );
    assert!(!本文.contains("開く"), "「開く」が残っています: {本文}");
}

/// **アーカイブ済みの印は名前の横に残ること**（設計書5.4）。
async fn アーカイブの印は残る(db: &DatabaseConnection) {
    let user = 利用者(db, "pj-archived").await;
    let p = プロジェクト(db, "終わったプロジェクト", true).await;
    メンバー(db, user.id, p.id, "Viewer").await;

    let 本文 = 本文だけ(&開く(db, &user, "/projects").await).to_owned();
    assert!(
        本文.contains(&format!(r#"href="/projects/{}""#, p.id)),
        "{本文}"
    );
    assert!(本文.contains("アーカイブ済み"), "{本文}");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// メニューを除いた本文。メニューにも `/projects` へのリンクがあるため分ける。
fn 本文だけ(body: &str) -> &str {
    let start = body
        .find(r#"<main class="content">"#)
        .expect("本文がありません");
    &body[start..]
}

async fn 開く(db: &DatabaseConnection, user: &app_user::Model, uri: &str) -> String {
    let config = 設定();
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    let state = AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: Default::default(),
    };
    let (_, token) = session::create(
        db,
        user.id,
        "127.0.0.1",
        "test",
        &state.config.session,
        Utc::now(),
    )
    .await
    .unwrap();

    let res = router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!("{}={}", session::COOKIE_NAME, token.as_str()),
                )
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

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

async fn 利用者(db: &DatabaseConnection, username: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証".to_owned()),
        username: Set(username.to_owned()),
        email: Set(None),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set("ja".to_owned()),
        theme: Set("system".to_owned()),
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

async fn プロジェクト(db: &DatabaseConnection, name: &str, archived: bool) -> project::Model {
    project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(name.to_owned()),
        description: Set(String::new()),
        currency: Set("JPY".to_owned()),
        archived_at: Set(archived.then(Utc::now)),
        closure_reason: Set(archived.then(|| "Completed".to_owned())),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn メンバー(db: &DatabaseConnection, user_id: i32, project_id: i32, role: &str) {
    project_member::ActiveModel {
        user_id: Set(user_id),
        project_id: Set(project_id),
        role: Set(role.to_owned()),
        admin_rank: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 名前を押して開ける);
        全検証!(@one $用意, $属性, アーカイブの印は残る);
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
