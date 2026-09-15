//! プロジェクトのメンバー管理画面の結合テスト（設計書16.1のB領域、5章）。
//!
//! **この画面が守るべきものは3つある。**
//!
//! - 兼務を許す（5章、旧C-8）。ラジオボタンにしてはいけない
//! - `Administrator` をここで変えさせない（5章）。System Adminの任命を
//!   プロジェクト側から覆せると、正・副の一意性を守る責任者が2箇所に分かれる
//! - 変更できるのはAdministratorだけ（3章の権限マトリクス）

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, audit_log, project, project_member};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;
use tower::ServiceExt;

/// **兼務できること**（設計書5章、旧C-8）。
///
/// OperatorとApproverの兼務は普通にある。ここをラジオボタンにすると、
/// 11章の承認フローを一人で回せる小規模な現場が成立しなくなる。
async fn 複数のロールを兼務できる(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "admin@example.com", "Administrator").await;
    let 相手 = 利用者(db, "both@example.com").await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[
            ("user_id", &相手.id.to_string()),
            ("operator", "on"),
            ("approver", "on"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let mut roles = 保有ロール(db, p.id, 相手.id).await;
    roles.sort();
    assert_eq!(roles, vec!["Approver", "Operator"]);
}

/// チェックを外すとロールが消えること。全部外せばプロジェクトから外れる。
async fn チェックを外すと役割が消える(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "remove-admin@example.com", "Administrator").await;
    let 相手 = 利用者(db, "leaving@example.com").await;
    メンバー(db, 相手.id, p.id, "Operator", None).await;
    メンバー(db, 相手.id, p.id, "Viewer", None).await;

    // Viewerだけ残す
    let (状態, token) = 認証済み(db, &admin).await;
    送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &相手.id.to_string()), ("viewer", "on")],
    )
    .await;
    assert_eq!(保有ロール(db, p.id, 相手.id).await, vec!["Viewer"]);

    // 全部外す
    let (状態, token) = 認証済み(db, &admin).await;
    送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &相手.id.to_string())],
    )
    .await;
    assert!(保有ロール(db, p.id, 相手.id).await.is_empty());
}

/// **`Administrator` の行に手を出さないこと**（設計書5章）。
///
/// System Adminが任命した管理者を、プロジェクト側から外せてはならない。
/// 同じ利用者が兼務しているOperatorの行だけが対象になる。
async fn 管理者の行には触らない(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "keep-admin@example.com", "Administrator").await;
    // 管理者が Operator も兼務している状態
    メンバー(db, admin.id, p.id, "Operator", None).await;
    // 正管理者としての行を作り直す（admin_rankつき）
    project_member::Entity::delete_many()
        .filter(project_member::Column::UserId.eq(admin.id))
        .filter(project_member::Column::Role.eq("Administrator"))
        .exec(db)
        .await
        .unwrap();
    メンバー(db, admin.id, p.id, "Administrator", Some("Primary")).await;

    // 自分のチェックを全部外して保存する
    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &admin.id.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let roles = 保有ロール(db, p.id, admin.id).await;
    assert_eq!(
        roles,
        vec!["Administrator"],
        "管理者の行まで消えている（System Adminの任命を覆してしまう）"
    );
    // admin_rank も残っていること
    let 管理者行 = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(admin.id))
        .filter(project_member::Column::Role.eq("Administrator"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(管理者行.admin_rank.as_deref(), Some("Primary"));
}

/// 画面に管理者を変える手段が無いこと。
async fn 画面に管理者の編集欄が無い(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "view-admin@example.com", "Administrator").await;

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, body) = 取得(状態, &format!("/projects/{}/members", p.id), &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("System Admin が行います"), "注記が出ていない");
    assert!(
        !body.contains("name=\"administrator\""),
        "管理者を変えるフィールドが出ている"
    );
}

/// **System Admin をメンバーにできないこと**（設計書5章）。
///
/// ロールを持っても3章によりプロジェクトデータへアクセスできず、割り当てると
/// 画面と実際の挙動が食い違う。
async fn システム管理者は追加できない(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "sa-add@example.com", "Administrator").await;
    let sa = app_user::ActiveModel {
        name: Set("システム管理者".to_owned()),
        username: Set(("sa@example.com".to_owned()).replace('@', "_")),
        email: Set(Some("sa@example.com".to_owned())),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(true),
        locale: Set("ja".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &sa.id.to_string()), ("operator", "on")],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "エラーとして再表示されるはず");
    assert!(body.contains("System Admin はプロジェクトのメンバーにできません"));
    assert!(保有ロール(db, p.id, sa.id).await.is_empty());

    // 候補一覧にも出ないこと。**押せてしまう選択肢を出さない**
    let (状態, token) = 認証済み(db, &admin).await;
    let (_, 画面) = 取得(状態, &format!("/projects/{}/members", p.id), &token).await;
    assert!(!画面.contains("sa@example.com"));
}

/// **無効化された利用者は新規に追加できないこと**（設計書20.11）。
async fn 無効化された利用者は追加できない(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "disabled-add@example.com", "Administrator").await;
    let 退職者 = 利用者(db, "retired@example.com").await;
    app_user::ActiveModel {
        id: Set(退職者.id),
        disabled_at: Set(Some(Utc::now())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &退職者.id.to_string()), ("operator", "on")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("無効化された利用者は追加できません"));
    assert!(保有ロール(db, p.id, 退職者.id).await.is_empty());
}

/// **既にメンバーの利用者が無効化された場合は、一覧に残って外せること**（20.11）。
///
/// 一覧から消してしまうと、退職者のロールを外す手段が無くなる。
async fn 無効化された既存メンバーは外せる(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "disabled-remove@example.com", "Administrator").await;
    let 退職者 = 利用者(db, "gone@example.com").await;
    メンバー(db, 退職者.id, p.id, "Operator", None).await;
    app_user::ActiveModel {
        id: Set(退職者.id),
        disabled_at: Set(Some(Utc::now())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &admin).await;
    let (_, 画面) = 取得(状態, &format!("/projects/{}/members", p.id), &token).await;
    assert!(画面.contains("gone@example.com"), "一覧から消えている");

    let (状態, token) = 認証済み(db, &admin).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &退職者.id.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(保有ロール(db, p.id, 退職者.id).await.is_empty());
}

/// **Operatorはロールを変えられないこと**（3章の権限マトリクス）。
///
/// 画面はボタンを出さないが、POSTは直接叩ける。
async fn 操作者はロールを変えられない(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "op-change@example.com", "Operator").await;
    let 相手 = 利用者(db, "victim@example.com").await;

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &相手.id.to_string()), ("operator", "on")],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(保有ロール(db, p.id, 相手.id).await.is_empty());
}

/// Operatorも一覧は見られること。誰が承認者かは全員が知る必要がある。
async fn 操作者も一覧は見られる(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "op-view@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, body) = 取得(状態, &format!("/projects/{}/members", p.id), &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("op-view@example.com"));
    assert!(!body.contains("メンバーを追加"), "編集欄が出ている");
}

/// 他プロジェクトのメンバーは見られないこと（設計書3章）。
async fn 他プロジェクトのメンバーは見られない(db: &DatabaseConnection) {
    let (よそ者, _) = 準備(db, "outsider@example.com", "Administrator").await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;

    let (状態, token) = 認証済み(db, &よそ者).await;
    let (status, _) = 取得(状態, &format!("/projects/{}/members", 他人の.id), &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// ロールの変更が監査ログに残ること（設計書15.2）。
async fn 変更が監査ログに残る(db: &DatabaseConnection) {
    let (admin, p) = 準備(db, "audit@example.com", "Administrator").await;
    let 相手 = 利用者(db, "audited@example.com").await;

    let (状態, token) = 認証済み(db, &admin).await;
    送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &相手.id.to_string()), ("approver", "on")],
    )
    .await;
    let (状態, token) = 認証済み(db, &admin).await;
    送信(
        状態,
        &format!("/projects/{}/members", p.id),
        &token,
        &[("user_id", &相手.id.to_string())],
    )
    .await;

    let ログ = audit_log::Entity::find()
        .filter(audit_log::Column::TableName.eq("project_member"))
        .all(db)
        .await
        .unwrap();

    assert!(ログ.iter().any(|l| l.action == "insert"));
    assert!(
        ログ.iter().any(|l| l.action == "delete"),
        "外した記録が残っていない"
    );
    assert!(ログ.iter().all(|l| l.user_id == admin.id));
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 保有ロール(db: &DatabaseConnection, project_id: i32, user_id: i32) -> Vec<String> {
    let mut roles: Vec<String> = project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .filter(project_member::Column::UserId.eq(user_id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.role)
        .collect();
    roles.sort();
    roles
}

async fn 準備(
    db: &DatabaseConnection,
    email: &str,
    role: &str,
) -> (app_user::Model, project::Model) {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, &format!("{role}のプロジェクト")).await;
    メンバー(db, user.id, p.id, role, None).await;
    (user, p)
}

async fn 認証済み(db: &DatabaseConnection, user: &app_user::Model) -> (AppState, String) {
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
    (state, token.as_str().to_owned())
}

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

fn cookie_header(token: &str) -> String {
    format!("{}={token}", session::COOKIE_NAME)
}

async fn 取得(state: AppState, uri: &str, token: &str) -> (StatusCode, String) {
    let res = router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie_header(token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn 送信(
    state: AppState,
    uri: &str,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    let csrf = dioryga::auth::csrf::derive(token);
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
                .header(header::COOKIE, cookie_header(token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn 分解(res: Response<Body>) -> (StatusCode, String) {
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(email.to_owned()),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        password_hash: Set("$argon2id$dummy".to_owned()),
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

async fn メンバー(
    db: &DatabaseConnection,
    user_id: i32,
    project_id: i32,
    role: &str,
    rank: Option<&str>,
) {
    project_member::ActiveModel {
        user_id: Set(user_id),
        project_id: Set(project_id),
        role: Set(role.to_owned()),
        admin_rank: Set(rank.map(str::to_owned)),
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
        全検証!(@one $用意, $属性, 複数のロールを兼務できる);
        全検証!(@one $用意, $属性, チェックを外すと役割が消える);
        全検証!(@one $用意, $属性, 管理者の行には触らない);
        全検証!(@one $用意, $属性, 画面に管理者の編集欄が無い);
        全検証!(@one $用意, $属性, システム管理者は追加できない);
        全検証!(@one $用意, $属性, 無効化された利用者は追加できない);
        全検証!(@one $用意, $属性, 無効化された既存メンバーは外せる);
        全検証!(@one $用意, $属性, 操作者はロールを変えられない);
        全検証!(@one $用意, $属性, 操作者も一覧は見られる);
        全検証!(@one $用意, $属性, 他プロジェクトのメンバーは見られない);
        全検証!(@one $用意, $属性, 変更が監査ログに残る);
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
