//! ポート接続の結合テスト（設計書16.1、8.4、8.7）。
//!
//! ケーブルの端を、機器に載っている部品のポートに挿す。**ケーブルの実物は
//! 最初の接続と同時に作り、プロジェクトへの所属は持たない。**

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, cable_catalog, cable_connection, cable_end_slot, cable_instance, device,
    device_assignment, part_catalog, part_instance, part_instance_location, part_port_slot,
    project, project_member, vendor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

/// **新しいケーブルを挿すと実物ができ、反対の端は相手の機器から挿せること。**
async fn 両端を挿すと接続先が出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-both@example.com", "Operator").await;
    let a = 機器(db, &場, "conn-a", "Network", "LC").await;
    let b = 機器(db, &場, "conn-b", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;

    // A端を conn-a に挿す。実物はここでできる
    let (status, body) = 挿す(
        db,
        &場,
        &a,
        &format!("new:{}", 型.ends[0].id),
        &[("serial_number", "CBL-001")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let 実物 = cable_instance::Entity::find().all(db).await.unwrap();
    assert_eq!(実物.len(), 1);
    assert_eq!(実物[0].serial_number.as_deref(), Some("CBL-001"));
    let 接続 = 現行の接続(db).await;
    assert_eq!(接続.len(), 1);
    // この画面が書く接続は、変更管理チケットを持たない
    assert_eq!(接続[0].work_order_id, None);

    // 反対の端はまだ挿していない
    let (_, body) = 画面(db, &場, &a).await;
    assert!(body.contains("CBL-001"), "{body}");
    assert!(body.contains("B: 未接続"), "{body}");

    // conn-b の画面に、空いた端が候補として出る
    let 空いた端 = format!("open:{}:{}", 実物[0].id, 型.ends[1].id);
    let (_, body) = 画面(db, &場, &b).await;
    assert!(
        body.contains(&空いた端),
        "空いた端が候補に出ていません: {body}"
    );

    let (status, body) = 挿す(db, &場, &b, &空いた端, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    // 実物は増えない
    assert_eq!(
        cable_instance::Entity::find().all(db).await.unwrap().len(),
        1
    );
    assert_eq!(現行の接続(db).await.len(), 2);

    // どちらの画面にも、相手の機器が出る
    let (_, body) = 画面(db, &場, &a).await;
    assert!(body.contains("B: conn-b"), "{body}");
    let (_, body) = 画面(db, &場, &b).await;
    assert!(body.contains("A: conn-a"), "{body}");
    // 両端が埋まったので、候補から消える
    assert!(!body.contains("open:"), "{body}");
}

/// **外すと現行行を閉じ、行は残ること。**すべての端を外したケーブルは、
/// 候補に出ない（画面から辿れなくなる）。
async fn 外すと閉じて残る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-off@example.com", "Operator").await;
    let a = 機器(db, &場, "off-a", "Network", "LC").await;
    let b = 機器(db, &場, "off-b", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    let c = 現行の接続(db).await.remove(0);

    // 別の機器の画面からは外せない
    let (status, _) = 外す(db, &場, &b, c.id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(現行の接続(db).await.len(), 1);

    let (status, body) = 外す(db, &場, &a, c.id).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(現行の接続(db).await.is_empty());
    let 履歴 = cable_connection::Entity::find().all(db).await.unwrap();
    assert_eq!(履歴.len(), 1, "行が消えています");
    assert!(履歴[0].to_date.is_some());

    // 実物は残るが、どの画面からも辿れない
    assert_eq!(
        cable_instance::Entity::find().all(db).await.unwrap().len(),
        1
    );
    let (_, body) = 画面(db, &場, &b).await;
    assert!(!body.contains("open:"), "{body}");

    // ポートは空いたので、もう一度挿せる（新しい実物になる）
    let (status, _) = 挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        cable_instance::Entity::find().all(db).await.unwrap().len(),
        2
    );
}

/// **ケーブルの種別とポートの種別が違えば拒否すること。**
async fn 種別が違えば挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-kind@example.com", "Operator").await;
    let a = 機器(db, &場, "kind-a", "Network", "LC").await;
    let 電源 = ケーブルの型(db, &場, "Power", &[("A", "IEC C13"), ("B", "IEC C14")]).await;

    let (status, body) = 挿す(db, &場, &a, &format!("new:{}", 電源.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("種別が違います"), "{body}");
    assert!(現行の接続(db).await.is_empty());
    assert!(cable_instance::Entity::find()
        .all(db)
        .await
        .unwrap()
        .is_empty());
}

/// **コネクタ形状が違っても挿せて、警告が出ること。**対になる組（C13とC14）は
/// 値が違うので、形状では拒否しない。
async fn 形状が違えば警告を出す(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-shape@example.com", "Operator").await;
    let psu = 機器(db, &場, "shape-a", "Power", "IEC C14").await;
    let nic = 機器(db, &場, "shape-b", "Network", "LC").await;
    let 電源 = ケーブルの型(db, &場, "Power", &[("A", "IEC C13"), ("B", "NEMA 5-15P")]).await;
    let 光 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;

    let (status, body) = 挿す(db, &場, &psu, &format!("new:{}", 電源.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let (_, body) = 画面(db, &場, &psu).await;
    assert!(body.contains("コネクタ形状が違います"), "{body}");

    // 同じ値なら警告は出ない
    挿す(db, &場, &nic, &format!("new:{}", 光.ends[0].id), &[]).await;
    let (_, body) = 画面(db, &場, &nic).await;
    assert!(!body.contains("コネクタ形状が違います"), "{body}");
}

/// **使用中のポートと端には挿せないこと。**
async fn 使用中のポートと端には挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-used@example.com", "Operator").await;
    let a = 機器(db, &場, "used-a", "Network", "LC").await;
    let b = 機器(db, &場, "used-b", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();

    // 同じポートにもう1本
    let (status, body) = 挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("すでにケーブルが挿さっています"), "{body}");

    // すでに挿さっている端を、別の機器に
    let (status, body) = 挿す(
        db,
        &場,
        &b,
        &format!("open:{}:{}", 実物.id, 型.ends[0].id),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("すでに挿さっています"), "{body}");

    // 別の型の端は、このケーブルの端ではない
    let 他 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    let (status, _) = 挿す(
        db,
        &場,
        &b,
        &format!("open:{}:{}", 実物.id, 他.ends[1].id),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(現行の接続(db).await.len(), 1);
}

/// **他のプロジェクトのケーブルの端は挿せないこと。**IDを直接送っても通さない。
async fn 他のプロジェクトのケーブルは挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-mine@example.com", "Operator").await;
    let 他 = 舞台(db, "conn-other@example.com", "Operator").await;
    let a = 機器(db, &場, "mine-a", "Network", "LC").await;
    let x = 機器(db, &他, "other-x", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;
    挿す(db, &他, &x, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let 空いた端 = format!("open:{}:{}", 実物.id, 型.ends[1].id);

    // 候補に出ない
    let (_, body) = 画面(db, &場, &a).await;
    assert!(!body.contains(&空いた端), "{body}");
    assert!(!body.contains("other-x"), "{body}");

    let (status, _) = 挿す(db, &場, &a, &空いた端, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(現行の接続(db).await.len(), 1);
}

/// **閲覧だけのロールは、画面は開けるが挿せないこと。**
async fn 閲覧者は挿せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-view@example.com", "Viewer").await;
    let a = 機器(db, &場, "view-a", "Network", "LC").await;
    let 型 = ケーブルの型(db, &場, "Network", &[("A", "LC"), ("B", "LC")]).await;

    let (status, body) = 画面(db, &場, &a).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("name=\"cable\""),
        "接続のフォームが出ています"
    );

    let (status, _) = 挿す(db, &場, &a, &format!("new:{}", 型.ends[0].id), &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(現行の接続(db).await.is_empty());
}

/// **ブレイクアウトの端を、1本ずつ別々に挿せること。**
async fn ブレイクアウトの端を別々に挿せる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "conn-bo@example.com", "Operator").await;
    let sw = 機器(db, &場, "bo-sw", "Network", "MPO12").await;
    let s1 = 機器(db, &場, "bo-s1", "Network", "LC").await;
    let s2 = 機器(db, &場, "bo-s2", "Network", "LC").await;
    let 型 = ケーブルの型(
        db,
        &場,
        "Network",
        &[("Trunk", "MPO12"), ("Branch1", "LC"), ("Branch2", "LC")],
    )
    .await;

    挿す(db, &場, &sw, &format!("new:{}", 型.ends[0].id), &[]).await;
    let 実物 = cable_instance::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let (s, _) = 挿す(
        db,
        &場,
        &s1,
        &format!("open:{}:{}", 実物.id, 型.ends[1].id),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);

    // 1本挿しても、残りの端は候補に残る
    let 残り = format!("open:{}:{}", 実物.id, 型.ends[2].id);
    let (_, body) = 画面(db, &場, &s2).await;
    assert!(body.contains(&残り), "{body}");
    let (s, _) = 挿す(db, &場, &s2, &残り, &[]).await;
    assert_eq!(s, StatusCode::SEE_OTHER);

    let (_, body) = 画面(db, &場, &sw).await;
    assert!(body.contains("Branch1: bo-s1"), "{body}");
    assert!(body.contains("Branch2: bo-s2"), "{body}");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    vendor_id: i32,
}

/// ポートを1つ持つ部品を載せた機器。
struct 機器情報 {
    device: device::Model,
    part_instance_id: i32,
    port_slot_id: i32,
}

struct 型情報 {
    ends: Vec<cable_end_slot::Model>,
}

async fn 舞台(db: &DatabaseConnection, email: &str, role: &str) -> 舞台情報 {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, &format!("{email}のプロジェクト")).await;
    メンバー(db, user.id, p.id, role).await;
    let v = vendor::ActiveModel {
        name: Set(format!("ベンダー-{email}")),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    舞台情報 {
        user,
        project: p,
        vendor_id: v.id,
    }
}

/// 機器を作り、ポートを1つ持つ部品を載せる。
async fn 機器(
    db: &DatabaseConnection,
    場: &舞台情報,
    hostname: &str,
    port_kind: &str,
    connector: &str,
) -> 機器情報 {
    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(None),
        hostname: Set(hostname.to_owned()),
        device_type: Set("Physical".to_owned()),
        device_category: Set(Some("Server".to_owned())),
        power_watt: Set(350),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
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

    let catalog = part_catalog::ActiveModel {
        category: Set("NIC".to_owned()),
        vendor_id: Set(場.vendor_id),
        part_number: Set(format!("PN-{hostname}")),
        core_count: Set(None),
        capacity_gb: Set(None),
        spec_json: Set("{}".to_owned()),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let slot = part_port_slot::ActiveModel {
        part_catalog_id: Set(catalog.id),
        port_kind: Set(port_kind.to_owned()),
        port_label: Set("Port1".to_owned()),
        connector_type: Set(connector.to_owned()),
        port_speed: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let instance = part_instance::ActiveModel {
        part_catalog_id: Set(catalog.id),
        serial_number: Set(None),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    part_instance_location::ActiveModel {
        part_instance_id: Set(instance.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(d.id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    機器情報 {
        device: d,
        part_instance_id: instance.id,
        port_slot_id: slot.id,
    }
}

async fn ケーブルの型(
    db: &DatabaseConnection,
    場: &舞台情報,
    kind: &str,
    ends: &[(&str, &str)],
) -> 型情報 {
    let c = cable_catalog::ActiveModel {
        cable_kind: Set(Some(kind.to_owned())),
        cable_type: Set(format!("{kind}ケーブル")),
        length_mm: Set(Some(2000)),
        color: Set(String::new()),
        vendor_id: Set(None),
        part_number: Set(None),
        rated_voltage: Set(None),
        rated_current_ma: Set(None),
        retired_at: Set(None),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let mut out = Vec::new();
    for (label, connector) in ends {
        out.push(
            cable_end_slot::ActiveModel {
                cable_catalog_id: Set(c.id),
                end_label: Set((*label).to_owned()),
                connector_type: Set((*connector).to_owned()),
                port_speed: Set(None),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
                ..Default::default()
            }
            .insert(db)
            .await
            .unwrap(),
        );
    }
    型情報 { ends: out }
}

fn 経路(場: &舞台情報, d: &機器情報) -> String {
    format!(
        "/projects/{}/devices/{}/connections",
        場.project.id, d.device.id
    )
}

async fn 画面(
    db: &DatabaseConnection, 場: &舞台情報, d: &機器情報
) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    取得(状態, &経路(場, d), &token).await
}

async fn 挿す(
    db: &DatabaseConnection,
    場: &舞台情報,
    d: &機器情報,
    cable: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, String) {
    let port = format!("{}:{}", d.part_instance_id, d.port_slot_id);
    let mut fields = vec![("port", port.as_str()), ("cable", cable)];
    fields.extend_from_slice(extra);
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(状態, &経路(場, d), &token, &fields).await
}

async fn 外す(
    db: &DatabaseConnection,
    場: &舞台情報,
    d: &機器情報,
    connection_id: i32,
) -> (StatusCode, String) {
    let id = connection_id.to_string();
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(
        状態,
        &format!("{}/disconnect", 経路(場, d)),
        &token,
        &[("connection_id", id.as_str())],
    )
    .await
}

async fn 現行の接続(db: &DatabaseConnection) -> Vec<cable_connection::Model> {
    cable_connection::Entity::find()
        .filter(cable_connection::Column::ToDate.is_null())
        .order_by_asc(cable_connection::Column::Id)
        .all(db)
        .await
        .unwrap()
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 両端を挿すと接続先が出る);
        全検証!(@one $用意, $属性, 外すと閉じて残る);
        全検証!(@one $用意, $属性, 種別が違えば挿せない);
        全検証!(@one $用意, $属性, 形状が違えば警告を出す);
        全検証!(@one $用意, $属性, 使用中のポートと端には挿せない);
        全検証!(@one $用意, $属性, 他のプロジェクトのケーブルは挿せない);
        全検証!(@one $用意, $属性, 閲覧者は挿せない);
        全検証!(@one $用意, $属性, ブレイクアウトの端を別々に挿せる);
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
