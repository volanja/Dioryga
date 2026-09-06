//! 倉庫領域の結合テスト（設計書16.1のC領域、12章、12.4、3章）。
//!
//! # 何を確かめているか
//!
//! **System Adminが入れないこと。**倉庫にある機器は過去にプロジェクトへ属して
//! いた履歴を持ち続けるため、倉庫を通せば3章の逆転制約が崩れる。ミドルウェアの
//! ガードとハンドラの双方で拒否している。
//!
//! **倉庫の中を全件・全履歴で見せること。**A-6の但し書きに対する意図した例外で
//! ある（16.1）。**A-6の判定をこの画面に持ち込むと、誰のプロジェクトにも属した
//! ことがない新品の予備が誰にも見えなくなる。**
//!
//! **払い出された機器が倉庫の一覧から消えること。**`to_date` が入った行を
//! 数えると、出ていった機器が在庫に居座る。

mod support;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::{Duration, Utc};
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, device, device_assignment, part_catalog, part_instance, part_instance_location,
    project, project_member, vendor, warehouse,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, PaginatorTrait, Set};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 認可（設計書3章、16.1のC領域）
// ---------------------------------------------------------------------------

/// **System Adminは倉庫に入れないこと**（3章）。
///
/// 倉庫にある機器は過去にプロジェクトへ属していた履歴を持ち続けるため、
/// 倉庫を通せばプロジェクト内データが見える。「倉庫はプロジェクト外だから
/// System Adminの管轄」という整理は、この一点で成立しない。
async fn システム管理者は倉庫に入れない(db: &DatabaseConnection) {
    let admin = 利用者(db, "wh-sysadmin@example.com", true).await;
    let w = 倉庫(db, admin.id, "本社倉庫").await;

    let (状態, token) = 認証済み(db, &admin).await;
    for uri in [
        "/warehouses".to_owned(),
        format!("/warehouses/{}/devices", w.id),
        format!("/warehouses/{}/parts", w.id),
    ] {
        let (status, _) = 取得(状態.clone(), &uri, &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} が通っています");
    }
}

/// **どのプロジェクトにも属していない利用者は入れないこと**（16.1）。
async fn 非メンバーは入れない(db: &DatabaseConnection) {
    let 部外者 = 利用者(db, "wh-outsider@example.com", false).await;
    let (状態, token) = 認証済み(db, &部外者).await;
    let (status, _) = 取得(状態, "/warehouses", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// **閲覧はロールを問わないこと。**Viewerでも在庫は見られる（16.1）。
async fn 閲覧者も見られる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-viewer@example.com", "Viewer").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(状態, "/warehouses", &token).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    // 閲覧だけの利用者に登録欄を出さない
    assert!(
        !body.contains("name=\"address\""),
        "閲覧者に登録欄が出ています: {body}"
    );
}

/// **Viewerは倉庫を登録できないこと**（16.1、Operator以上）。
async fn 閲覧者は登録できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-viewer-post@example.com", "Viewer").await;
    let (状態, token) = 認証済み(db, &場.user).await;

    let (status, _) = 送信(
        状態,
        "/warehouses",
        &token,
        &[("name", "勝手な倉庫"), ("address", "どこか")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(warehouse::Entity::find().count(db).await.unwrap(), 1);
}

/// **Operatorは登録できること**（16.1、カタログ編集と同じ条件）。
async fn 作業者は登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-operator@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &場.user).await;

    let (status, _) = 送信(
        状態,
        "/warehouses",
        &token,
        &[("name", "第二倉庫"), ("address", "川崎")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(warehouse::Entity::find().count(db).await.unwrap(), 2);
}

/// **名前が空の倉庫は作れないこと。**
async fn 名前は必須(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-noname@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &場.user).await;

    let (status, _) = 送信(
        状態,
        "/warehouses",
        &token,
        &[("name", "  "), ("address", "")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "エラーは一覧に戻して表示する");
    assert_eq!(warehouse::Entity::find().count(db).await.unwrap(), 1);
}

// ---------------------------------------------------------------------------
// 在庫（設計書12章、12.4）
// ---------------------------------------------------------------------------

/// **新品の予備が、関わりのない利用者にも見えること。**
///
/// A-6をそのまま倉庫の一覧に適用すると、**誰のプロジェクトにも属したことが
/// ない機器は誰にも見えない。**最も見たいものが最も見えなくなる（16.1）。
async fn 新品の予備も見える(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-spare@example.com", "Viewer").await;

    // どのプロジェクトにも属したことがない機器
    let d = 機器(db, "spare-001", "running").await;
    倉庫へ(db, d.id, 場.warehouse.id, 0).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(
        状態,
        &format!("/warehouses/{}/devices", 場.warehouse.id),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("spare-001"), "{body}");
}

/// **払い出された機器は倉庫の一覧から消えること。**
///
/// `DEVICE_ASSIGNMENT` は履歴であり、出ていっても行は残る。`to_date` を
/// 見ずに数えると、出ていった機器が在庫に居座る。
async fn 払い出した機器は出ない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-issued@example.com", "Operator").await;

    let d = 機器(db, "issued-001", "running").await;
    // 倉庫にいた期間は閉じている
    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Warehouse".to_owned()),
        location_id: Set(Some(場.warehouse.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(60)),
        to_date: Set(Some(Utc::now() - Duration::days(10))),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    // 今はプロジェクトにいる
    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(場.project.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(10)),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/warehouses/{}/devices", 場.warehouse.id),
        &token,
    )
    .await;

    assert!(
        !body.contains("issued-001"),
        "払い出し済みが出ています: {body}"
    );
}

/// **倉庫内のパーツ在庫が出ること**（12.4）。
async fn パーツ在庫が出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-parts@example.com", "Viewer").await;
    let pi = パーツ(db, 場.user.id, "PN-DIMM-32G", "SN-AAA").await;

    part_instance_location::ActiveModel {
        part_instance_id: Set(pi.id),
        location_type: Set("Warehouse".to_owned()),
        location_id: Set(Some(場.warehouse.id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(
        状態,
        &format!("/warehouses/{}/parts", 場.warehouse.id),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("PN-DIMM-32G"), "{body}");
    assert!(body.contains("SN-AAA"), "{body}");
}

// ---------------------------------------------------------------------------
// 機器の詳細（全履歴）
// ---------------------------------------------------------------------------

/// **他プロジェクトに属していた期間も出ること。**
///
/// A-6の但し書きに対する意図した例外である（16.1）。倉庫にある間は、
/// 関わりのない利用者にも「元々どこで使っていた機体か」を見せる。
async fn 他プロジェクトの所属歴も出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-history@example.com", "Viewer").await;
    let よそ = プロジェクト(db, "よそのプロジェクト").await;

    let d = 機器(db, "used-001", "running").await;
    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(よそ.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(400)),
        to_date: Set(Some(Utc::now() - Duration::days(30))),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    倉庫へ(db, d.id, 場.warehouse.id, 30).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 取得(
        状態,
        &format!("/warehouses/{}/devices/{}", 場.warehouse.id, d.id),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    // 閲覧者はこのプロジェクトのメンバーではない。それでも所属歴が見える
    assert!(body.contains("よそのプロジェクト"), "{body}");
    assert!(
        body.contains("現在"),
        "現在有効な行の印がありません: {body}"
    );
}

/// **この倉庫にない機器は開けないこと**（3章）。
///
/// 一覧から外すだけでは、URLに機器IDを直接指定された場合に素通りする。
async fn 倉庫にない機器は開けない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "wh-direct@example.com", "Viewer").await;

    // プロジェクトにいる機器。倉庫には無い
    let d = 機器(db, "in-project", "running").await;
    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(場.project.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 取得(
        状態,
        &format!("/warehouses/{}/devices/{}", 場.warehouse.id, d.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 用意
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    warehouse: warehouse::Model,
}

async fn 舞台(db: &DatabaseConnection, email: &str, role: &str) -> 舞台情報 {
    let user = 利用者(db, email, false).await;
    let p = プロジェクト(db, email).await;
    メンバー(db, user.id, p.id, role).await;
    let w = 倉庫(db, user.id, "本社倉庫").await;

    舞台情報 {
        user,
        project: p,
        warehouse: w,
    }
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

async fn 機器(db: &DatabaseConnection, hostname: &str, status: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(None),
        hostname: Set(hostname.to_owned()),
        device_type: Set("Physical".to_owned()),
        serial_number: Set(Some(format!("SN-{hostname}"))),
        power_watt: Set(350),
        status: Set(status.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// `days_ago` 日前から倉庫にある、という現在有効な行を作る。
async fn 倉庫へ(db: &DatabaseConnection, device_id: i32, warehouse_id: i32, days_ago: i64) {
    device_assignment::ActiveModel {
        device_id: Set(device_id),
        location_type: Set("Warehouse".to_owned()),
        location_id: Set(Some(warehouse_id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(days_ago)),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn パーツ(
    db: &DatabaseConnection,
    user_id: i32,
    part_number: &str,
    serial: &str,
) -> part_instance::Model {
    let v = vendor::ActiveModel {
        name: Set(format!("ベンダー-{part_number}")),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let c = part_catalog::ActiveModel {
        category: Set("Memory".to_owned()),
        vendor_id: Set(v.id),
        part_number: Set(part_number.to_owned()),
        core_count: Set(None),
        capacity_gb: Set(Some(32)),
        spec_json: Set("{}".to_owned()),
        retired_at: Set(None),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    part_instance::ActiveModel {
        part_catalog_id: Set(c.id),
        serial_number: Set(Some(serial.to_owned())),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 利用者(db: &DatabaseConnection, email: &str, is_system_admin: bool) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(email.to_owned()),
        email: Set(email.to_owned()),
        password_hash: Set("$argon2id$dummy".to_owned()),
        must_change_password: Set(false),
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
        project_id: Set(project_id),
        user_id: Set(user_id),
        role: Set(role.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
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
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
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
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, システム管理者は倉庫に入れない);
        全検証!(@one $用意, $属性, 非メンバーは入れない);
        全検証!(@one $用意, $属性, 閲覧者も見られる);
        全検証!(@one $用意, $属性, 閲覧者は登録できない);
        全検証!(@one $用意, $属性, 作業者は登録できる);
        全検証!(@one $用意, $属性, 名前は必須);
        全検証!(@one $用意, $属性, 新品の予備も見える);
        全検証!(@one $用意, $属性, 払い出した機器は出ない);
        全検証!(@one $用意, $属性, パーツ在庫が出る);
        全検証!(@one $用意, $属性, 他プロジェクトの所属歴も出る);
        全検証!(@one $用意, $属性, 倉庫にない機器は開けない);
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
