//! 上部メニューと左メニューの結合テスト（#122、#123）。
//!
//! **上部は領域、左は今いる領域の中の画面。**どちらも今いる場所に印を付け、
//! 左メニューの印は常に1つにする（印が2つあるとどちらにいるのか読めない）。
//! 共有カタログの左メニューは `catalog.rs` で確かめている。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, project, project_member, warehouse};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// プロジェクト
// ---------------------------------------------------------------------------

/// **ダッシュボードから、プロジェクト内の各画面へ移れること**（#123）。
async fn ダッシュボードにプロジェクトの画面が並ぶ(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-dashboard", false, false).await;
    let p = プロジェクト(db, "カタクリ基盤更改").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let body = 開く(db, &user, &format!("/projects/{}", p.id)).await;
    let 左 = 左メニュー(&body);
    let base = format!("/projects/{}", p.id);
    for path in [
        "/devices",
        "/containers",
        "/network/ip-addresses",
        "/software/components",
        "/work-orders",
        "/milestones",
        "/costs",
        "/power",
        "/members",
        "/import",
    ] {
        assert!(
            左.contains(&format!(r#"href="{base}{path}""#)),
            "{path}: {左}"
        );
    }
    // プロジェクト名は見出し。印はダッシュボードに1つ
    assert!(
        左.contains(r#"<span class="heading">カタクリ基盤更改</span>"#),
        "{左}"
    );
    assert!(
        左.contains(&format!(r#"href="{base}" class="sub current""#)),
        "{左}"
    );
    assert_eq!(左.matches("current").count(), 1, "{左}");
    // 上部は「プロジェクト」に印
    assert!(
        上部メニュー(&body).contains(r#"href="/projects" class="current""#),
        "{body}"
    );
}

/// **詳細や内訳の画面では、親の項目に印を付けること**（#118と同じ規則）。
async fn 内訳の画面では親の項目に印が付く(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-sub", false, false).await;
    let p = プロジェクト(db, "印の検証").await;
    メンバー(db, user.id, p.id, "Operator").await;
    let base = format!("/projects/{}", p.id);

    for (path, 親) in [
        ("/devices/new", "/devices"),
        ("/costs/recurring", "/costs"),
        ("/network/subnets", "/network/ip-addresses"),
        ("/work-orders/new", "/work-orders"),
    ] {
        let body = 開く(db, &user, &format!("{base}{path}")).await;
        let 左 = 左メニュー(&body);
        assert!(
            左.contains(&format!(r#"href="{base}{親}" class="sub current""#)),
            "{path}: {左}"
        );
        assert_eq!(左.matches("current").count(), 1, "{path}: {左}");
    }
}

/// **取込は編集権のある利用者にだけ出すこと。**押しても入れない項目を並べない。
async fn 閲覧者には取込が出ない(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-viewer", false, false).await;
    let p = プロジェクト(db, "閲覧の検証").await;
    メンバー(db, user.id, p.id, "Viewer").await;

    let body = 開く(db, &user, &format!("/projects/{}/devices", p.id)).await;
    let 左 = 左メニュー(&body);
    assert!(左.contains("/members"), "{左}");
    assert!(!左.contains("/import"), "{左}");
}

/// **機器一覧の上部から、他の画面へのボタン列を外したこと**（#122）。
///
/// 左メニューと同じリンクが2か所に並ばない。この画面の操作（登録）だけ残す。
async fn 機器一覧にボタン列が無い(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-buttons", false, false).await;
    let p = プロジェクト(db, "ボタンの検証").await;
    メンバー(db, user.id, p.id, "Operator").await;

    let body = 開く(db, &user, &format!("/projects/{}/devices", p.id)).await;
    let 本文 = &body[body.find(r#"<main class="content">"#).unwrap()..];
    assert!(!本文.contains("/work-orders"), "{本文}");
    assert!(本文.contains(&format!("/projects/{}/devices/new", p.id)));
}

/// **プロジェクト一覧の左メニューは「プロジェクト一覧」だけ。**
async fn プロジェクト一覧の左メニュー(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-projects", false, false).await;

    let body = 開く(db, &user, "/projects").await;
    let 左 = 左メニュー(&body);
    assert!(左.contains(r#"href="/projects" class="current""#), "{左}");
    assert_eq!(左.matches("<a ").count(), 1, "{左}");
}

// ---------------------------------------------------------------------------
// 倉庫・個人設定・System Admin
// ---------------------------------------------------------------------------

/// 倉庫を開くと、倉庫名の下に中の画面が並ぶこと。
async fn 倉庫の中の画面が並ぶ(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-warehouse", false, false).await;
    let p = プロジェクト(db, "倉庫の検証").await;
    メンバー(db, user.id, p.id, "Viewer").await;
    let w = 倉庫(db, user.id, "第一倉庫").await;

    let body = 開く(db, &user, &format!("/warehouses/{}/parts", w.id)).await;
    let 左 = 左メニュー(&body);
    assert!(左.contains(r#"href="/warehouses" class="""#), "{左}");
    assert!(
        左.contains(r#"<span class="heading">第一倉庫</span>"#),
        "{左}"
    );
    assert!(
        左.contains(&format!(
            r#"href="/warehouses/{}/parts" class="sub current""#,
            w.id
        )),
        "{左}"
    );
    assert_eq!(左.matches("current").count(), 1, "{左}");
    assert!(
        上部メニュー(&body).contains(r#"href="/warehouses" class="current""#),
        "{body}"
    );
}

/// **個人設定はメニューつきで出し、強制変更のときはメニューを出さないこと。**
///
/// 強制変更の間は他の画面へ進めない（20.6）。押しても戻される項目を並べない。
async fn 個人設定と強制変更(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-account", false, false).await;
    let body = 開く(db, &user, "/account/password").await;
    assert!(
        上部メニュー(&body).contains(r#"href="/account/display" class="current""#),
        "{body}"
    );
    let 左 = 左メニュー(&body);
    assert!(
        左.contains(r#"href="/account/password" class="current""#),
        "{左}"
    );
    // 表示（言語・表示モード、#120）も並ぶ
    assert!(左.contains(r#"href="/account/display" class="""#), "{左}");

    let forced = 利用者(db, "nav-forced", false, true).await;
    let body = 開く(db, &forced, "/account/password").await;
    assert!(body.contains(r#"name="new_password""#), "{body}");
    assert!(!body.contains("topnav"), "{body}");
    assert!(!body.contains("sidenav"), "{body}");
}

/// **System Adminの上部には、入れる領域だけを並べること**（3章）。
async fn システム管理者の上部メニュー(db: &DatabaseConnection) {
    let admin = 利用者(db, "nav-admin", true, false).await;

    let body = 開く(db, &admin, "/admin/projects").await;
    let 上 = 上部メニュー(&body);
    assert!(上.contains(r#"href="/admin/users" class="""#), "{上}");
    assert!(
        上.contains(r#"href="/admin/projects" class="current""#),
        "{上}"
    );
    assert!(上.contains(r#"href="/account/display""#), "{上}");
    assert!(!上.contains("/catalog"), "{上}");
    assert!(!上.contains("/warehouses"), "{上}");
    assert!(左メニュー(&body).contains(r#"href="/admin/projects" class="current""#));
}

/// **画面の下にステータスバーが出て、右端に版が入ること**（#162）。
async fn ステータスバーに版が出る(db: &DatabaseConnection) {
    let user = 利用者(db, "nav-status", false, false).await;

    let body = 開く(db, &user, "/projects").await;
    let start = body
        .find(r#"<footer class="statusbar">"#)
        .expect("ステータスバーがありません");
    let bar = &body[start..];
    assert!(bar.contains("Dioryga "), "版が出ていません: {bar}");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 左メニュー(body: &str) -> &str {
    let start = body
        .find(r#"<nav class="sidenav">"#)
        .expect("左メニューがありません");
    let end = start + body[start..].find("</nav>").unwrap();
    &body[start..end]
}

fn 上部メニュー(body: &str) -> &str {
    let start = body
        .find(r#"<nav class="topnav">"#)
        .expect("上部メニューがありません");
    let end = start + body[start..].find("</nav>").unwrap();
    &body[start..end]
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

async fn 利用者(
    db: &DatabaseConnection,
    username: &str,
    is_system_admin: bool,
    must_change_password: bool,
) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証".to_owned()),
        username: Set(username.to_owned()),
        email: Set(None),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(must_change_password),
        is_system_admin: Set(is_system_admin),
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

async fn プロジェクト(db: &DatabaseConnection, name: &str) -> project::Model {
    project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(name.to_owned()),
        description: Set(String::new()),
        currency: Set("JPY".to_owned()),
        archived_at: Set(None),
        closure_reason: Set(None),
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

async fn 倉庫(db: &DatabaseConnection, user_id: i32, name: &str) -> warehouse::Model {
    warehouse::ActiveModel {
        name: Set(name.to_owned()),
        address: Set("東京".to_owned()),
        created_by: Set(user_id),
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
        全検証!(@one $用意, $属性, ダッシュボードにプロジェクトの画面が並ぶ);
        全検証!(@one $用意, $属性, 内訳の画面では親の項目に印が付く);
        全検証!(@one $用意, $属性, 閲覧者には取込が出ない);
        全検証!(@one $用意, $属性, 機器一覧にボタン列が無い);
        全検証!(@one $用意, $属性, プロジェクト一覧の左メニュー);
        全検証!(@one $用意, $属性, 倉庫の中の画面が並ぶ);
        全検証!(@one $用意, $属性, 個人設定と強制変更);
        全検証!(@one $用意, $属性, システム管理者の上部メニュー);
        全検証!(@one $用意, $属性, ステータスバーに版が出る);
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
