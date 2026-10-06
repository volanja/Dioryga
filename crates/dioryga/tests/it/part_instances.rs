//! プロジェクトの部品の画面の結合テスト（設計書6.2、12.4、16.1）。
//!
//! 部品の実物を登録し、置き場所を移す。**置けるのはこのプロジェクトの中だけで、
//! 移すと所在の履歴を閉じて開く。ケーブルが挿さっている部品は動かさない。**

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, cable_catalog, cable_connection, cable_end_slot, device, device_assignment,
    mount_container, part_catalog, part_instance, part_instance_location, part_port_slot, project,
    project_member, vendor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

/// **シリアルの無い部品も登録でき、置き場所ごとに一覧へ出ること。**
async fn 登録すると一覧に出る(db: &DatabaseConnection) {
    let 場 = 舞台(db, "pi-new@example.com", "Operator").await;
    let 棚 = 設備(db, &場, "予備棚").await;
    let d = 機器(db, &場.project, "pi-new-web01").await;
    let 型 = 型(db, &場, "DIMM-NEW").await.to_string();

    // シリアルを入れずに、プロジェクトに置く
    let (status, body) = 登録(db, &場, &型, "", "Project").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    // 棚と機器にも置ける
    let (status, body) = 登録(
        db,
        &場,
        &型,
        "SN-SHELF",
        &format!("MountContainer:{}", 棚.id),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let (status, body) = 登録(db, &場, &型, "SN-DEV", &format!("Device:{}", d.id)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let 全部 = part_instance::Entity::find().all(db).await.unwrap();
    assert_eq!(全部.len(), 3);
    assert!(全部.iter().any(|p| p.serial_number.is_none()));
    let p = 部品(db, "SN-SHELF").await;
    let 所在 = 履歴(db, p.id).await;
    assert_eq!(所在.len(), 1);
    assert_eq!(所在[0].location_type, "MountContainer");
    assert_eq!(所在[0].location_id, Some(棚.id));
    // この画面が書く所在は、変更管理チケットを持たない
    assert_eq!(所在[0].work_order_id, None);

    let (status, body) = 一覧(db, &場, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("SN-SHELF"), "{body}");
    assert!(body.contains("SN-DEV"), "{body}");
    assert!(body.contains("予備棚"), "{body}");
    assert!(body.contains("pi-new-web01"), "{body}");

    // 置き場所で絞り込める
    let (_, body) = 一覧(db, &場, "?at=container").await;
    assert!(body.contains("SN-SHELF"), "{body}");
    assert!(!body.contains("SN-DEV"), "{body}");
    let (_, body) = 一覧(db, &場, "?at=device").await;
    assert!(body.contains("SN-DEV"), "{body}");
    assert!(!body.contains("SN-SHELF"), "{body}");

    // 機器の詳細の「搭載部品」から部品の画面へ入れる
    let dp = 部品(db, "SN-DEV").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, body) = 取得(
        状態,
        &format!("/projects/{}/devices/{}", 場.project.id, d.id),
        &token,
    )
    .await;
    assert!(
        body.contains(&format!("/projects/{}/parts/{}", 場.project.id, dp.id)),
        "{body}"
    );
}

/// **置き場所を移すと、現行の行を閉じて新しい行を開くこと**（不変条件1）。
async fn 移すと閉じて開く(db: &DatabaseConnection) {
    let 場 = 舞台(db, "pi-move@example.com", "Operator").await;
    let 棚 = 設備(db, &場, "移動先の棚").await;
    let d = 機器(db, &場.project, "pi-move-web01").await;
    let 型 = 型(db, &場, "DIMM-MOVE").await.to_string();
    登録(db, &場, &型, "SN-MOVE", "Project").await;
    let p = 部品(db, "SN-MOVE").await;

    // 同じ場所へは移せない
    let (status, body) = 移す(db, &場, p.id, "Project").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既にその場所にあります"), "{body}");
    assert_eq!(履歴(db, p.id).await.len(), 1);

    let (status, body) = 移す(db, &場, p.id, &format!("MountContainer:{}", 棚.id)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let (status, body) = 移す(db, &場, p.id, &format!("Device:{}", d.id)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let 行 = 履歴(db, p.id).await;
    assert_eq!(行.len(), 3, "閉じて開いていません");
    assert!(行[0].to_date.is_some());
    assert!(行[1].to_date.is_some());
    assert_eq!(行[1].location_type, "MountContainer");
    assert_eq!(行[2].location_type, "Device");
    assert_eq!(行[2].location_id, Some(d.id));
    assert_eq!(行[2].to_date, None);
    // 閉じた時刻と開いた時刻がつながる
    assert_eq!(行[1].to_date, Some(行[2].from_date));

    // 詳細に履歴が出る
    let (status, body) = 詳細(db, &場, p.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("移動先の棚"), "{body}");
    assert!(body.contains("pi-move-web01"), "{body}");
}

/// **ケーブルが挿さっている部品は移せず、外すと移せること。**自動では外さない。
async fn ケーブルが挿さっていると移せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "pi-cable@example.com", "Operator").await;
    let d = 機器(db, &場.project, "pi-cable-web01").await;
    let nic = ポートつきの型(db, &場, "NIC-CABLE").await;
    登録(
        db,
        &場,
        &nic.0.to_string(),
        "SN-NIC",
        &format!("Device:{}", d.id),
    )
    .await;
    let p = 部品(db, "SN-NIC").await;

    // ポート接続の画面でケーブルを挿す
    let 端 = ケーブルの端(db, &場).await;
    let 接続 = format!("/projects/{}/devices/{}/connections", 場.project.id, d.id);
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 送信(
        状態,
        &接続,
        &token,
        &[
            ("port", &format!("{}:{}", p.id, nic.1)),
            ("cable", &format!("new:{端}")),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    // 一覧に印が出て、詳細には移すフォームの代わりに案内が出る
    let (_, body) = 一覧(db, &場, "").await;
    assert!(body.contains("ケーブルあり"), "{body}");
    let (_, body) = 詳細(db, &場, p.id).await;
    assert!(body.contains(&接続), "外す画面への導線がありません: {body}");
    assert!(!body.contains(&format!("/parts/{}/move", p.id)), "{body}");

    // POSTを直接叩いても移せない
    let (status, body) = 移す(db, &場, p.id, "Project").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ケーブルが挿さっています"), "{body}");
    assert_eq!(履歴(db, p.id).await.len(), 1);
    // 接続は外されていない
    let c = cable_connection::Entity::find()
        .filter(cable_connection::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .expect("接続が勝手に外されています");

    // 外すと移せる
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 送信(
        状態,
        &format!("{接続}/disconnect"),
        &token,
        &[("connection_id", &c.id.to_string())],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let (status, body) = 移す(db, &場, p.id, "Project").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(履歴(db, p.id).await.len(), 2);
}

/// **置けるのはこのプロジェクトの中だけであること。**他のプロジェクトの機器・
/// 設備・什器、撤去した設備・什器、過去にあった機器には置けない。
async fn このプロジェクトの外には置けない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "pi-out@example.com", "Operator").await;
    let 他 = プロジェクト(db, "pi-out の他プロジェクト").await;
    let 他の機器 = 機器(db, &他, "pi-out-other").await;
    let 他の棚 = 設備の行(db, 他.id, 場.user.id, "他の棚", false).await;
    let 撤去した棚 = 設備の行(db, 場.project.id, 場.user.id, "撤去した棚", true).await;
    // 過去にこのプロジェクトにあり、今は他のプロジェクトにある機器
    let 去った機器 = 機器(db, &他, "pi-out-gone").await;
    device_assignment::ActiveModel {
        device_id: Set(去った機器.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(場.project.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - chrono::Duration::days(30)),
        to_date: Set(Some(Utc::now() - chrono::Duration::days(10))),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let 型 = 型(db, &場, "DIMM-OUT").await.to_string();

    for 置き場所 in [
        format!("Device:{}", 他の機器.id),
        format!("Device:{}", 去った機器.id),
        format!("MountContainer:{}", 他の棚.id),
        format!("MountContainer:{}", 撤去した棚.id),
        "Disposed".to_owned(),
        format!("Project:{}", 他.id),
        String::new(),
    ] {
        let (status, body) = 登録(db, &場, &型, "", &置き場所).await;
        assert_eq!(status, StatusCode::OK, "{置き場所}: {body}");
        assert!(
            body.contains("置き場所を選んでください"),
            "{置き場所}: {body}"
        );
    }
    assert!(part_instance::Entity::find()
        .all(db)
        .await
        .unwrap()
        .is_empty());

    // 移す先としても選べない
    登録(db, &場, &型, "SN-OUT", "Project").await;
    let p = 部品(db, "SN-OUT").await;
    let (status, body) = 移す(db, &場, p.id, &format!("Device:{}", 他の機器.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("置き場所を選んでください"), "{body}");
    assert_eq!(履歴(db, p.id).await.len(), 1);
    // 候補にも出ない
    let (_, body) = 詳細(db, &場, p.id).await;
    assert!(!body.contains("pi-out-other"), "{body}");
    assert!(!body.contains("撤去した棚"), "{body}");
}

/// **他のプロジェクトにある部品は、この画面に出ず、開けも移せもしないこと。**
async fn 他のプロジェクトの部品は扱えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "pi-other@example.com", "Operator").await;
    let 他 = プロジェクト(db, "pi-other の他プロジェクト").await;
    let 型 = 型(db, &場, "DIMM-OTHER").await;
    let p = part_instance::ActiveModel {
        part_catalog_id: Set(型),
        serial_number: Set(Some("SN-ELSEWHERE".to_owned())),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    part_instance_location::ActiveModel {
        part_instance_id: Set(p.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(他.id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (_, body) = 一覧(db, &場, "").await;
    assert!(!body.contains("SN-ELSEWHERE"), "{body}");
    let (status, _) = 詳細(db, &場, p.id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = 移す(db, &場, p.id, "Project").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(履歴(db, p.id).await.len(), 1);
}

/// **同じベンダーで同じシリアルの部品は2つ作れないこと**（取込の突合が決まらなくなる）。
/// 廃番の型も選べない。
async fn 重複と廃番は登録できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "pi-dup@example.com", "Operator").await;
    let 型a = 型(db, &場, "DIMM-DUP-A").await;
    let 型b = 型(db, &場, "DIMM-DUP-B").await;
    登録(db, &場, &型a.to_string(), "SN-DUP", "Project").await;

    // 型番が違っても、ベンダーが同じなら重複
    let (status, body) = 登録(db, &場, &型b.to_string(), "SN-DUP", "Project").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に登録されています"), "{body}");
    assert_eq!(
        part_instance::Entity::find().all(db).await.unwrap().len(),
        1
    );

    // 廃番にした型は選べない
    let c = part_catalog::Entity::find_by_id(型b)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut active: part_catalog::ActiveModel = c.into();
    active.retired_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
    let (status, body) = 登録(db, &場, &型b.to_string(), "SN-RETIRED", "Project").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("廃番の型番は選べません"), "{body}");
    assert_eq!(
        part_instance::Entity::find().all(db).await.unwrap().len(),
        1
    );
}

/// **閲覧者は見られるが、登録も移動もできないこと。**
async fn 閲覧者は書き換えられない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "pi-edit@example.com", "Operator").await;
    let 型 = 型(db, &場, "DIMM-VIEW").await.to_string();
    登録(db, &場, &型, "SN-VIEW", "Project").await;
    let p = 部品(db, "SN-VIEW").await;

    let 閲覧者 = 利用者(db, "pi-viewer@example.com").await;
    メンバー(db, 閲覧者.id, 場.project.id, "Viewer").await;
    let 見る人 = 舞台情報 {
        user: 閲覧者,
        project: 場.project.clone(),
        vendor_id: 場.vendor_id,
    };

    let (status, body) = 一覧(db, &見る人, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("SN-VIEW"), "{body}");
    assert!(!body.contains("name=\"part_catalog_id\""), "{body}");
    let (status, body) = 詳細(db, &見る人, p.id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains(&format!("/parts/{}/move", p.id)), "{body}");

    let (status, _) = 登録(db, &見る人, &型, "SN-NO", "Project").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = 移す(db, &見る人, p.id, "Project").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        part_instance::Entity::find().all(db).await.unwrap().len(),
        1
    );

    // メンバーでなければ見えもしない
    let 部外者 = 舞台情報 {
        user: 利用者(db, "pi-outsider@example.com").await,
        project: 場.project.clone(),
        vendor_id: 場.vendor_id,
    };
    let (status, _) = 一覧(db, &部外者, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 用意
// ---------------------------------------------------------------------------

struct 舞台情報 {
    user: app_user::Model,
    project: project::Model,
    vendor_id: i32,
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

/// プロジェクトに今ある機器。
async fn 機器(db: &DatabaseConnection, p: &project::Model, hostname: &str) -> device::Model {
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
        location_id: Set(Some(p.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    d
}

async fn 設備(db: &DatabaseConnection, 場: &舞台情報, name: &str) -> mount_container::Model {
    設備の行(db, 場.project.id, 場.user.id, name, false).await
}

async fn 設備の行(
    db: &DatabaseConnection,
    project_id: i32,
    user_id: i32,
    name: &str,
    撤去済み: bool,
) -> mount_container::Model {
    mount_container::ActiveModel {
        name: Set(name.to_owned()),
        location_type: Set("Project".to_owned()),
        location_id: Set(project_id),
        retired_at: Set(撤去済み.then(Utc::now)),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// 部品の型。`PART_CATALOG.id` を返す。
async fn 型(db: &DatabaseConnection, 場: &舞台情報, part_number: &str) -> i32 {
    part_catalog::ActiveModel {
        category: Set("Memory".to_owned()),
        vendor_id: Set(場.vendor_id),
        part_number: Set(part_number.to_owned()),
        core_count: Set(None),
        capacity_gb: Set(Some(32)),
        spec_json: Set("{}".to_owned()),
        retired_at: Set(None),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

/// ポートを1つ持つ部品の型。`(PART_CATALOG.id, PART_PORT_SLOT.id)` を返す。
async fn ポートつきの型(
    db: &DatabaseConnection,
    場: &舞台情報,
    part_number: &str,
) -> (i32, i32) {
    let id = 型(db, 場, part_number).await;
    let slot = part_port_slot::ActiveModel {
        part_catalog_id: Set(id),
        port_kind: Set("Network".to_owned()),
        port_label: Set("Port1".to_owned()),
        connector_type: Set("LC".to_owned()),
        port_speed: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    (id, slot.id)
}

/// ケーブルの型を作り、その端（`CABLE_END_SLOT.id`）を1つ返す。
async fn ケーブルの端(db: &DatabaseConnection, 場: &舞台情報) -> i32 {
    let c = cable_catalog::ActiveModel {
        cable_kind: Set(Some("Network".to_owned())),
        cable_type: Set("Networkケーブル".to_owned()),
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
    cable_end_slot::ActiveModel {
        cable_catalog_id: Set(c.id),
        end_label: Set("A".to_owned()),
        connector_type: Set("LC".to_owned()),
        port_speed: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn 一覧(db: &DatabaseConnection, 場: &舞台情報, query: &str) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    取得(
        状態,
        &format!("/projects/{}/parts{query}", 場.project.id),
        &token,
    )
    .await
}

async fn 詳細(db: &DatabaseConnection, 場: &舞台情報, part_id: i32) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    取得(
        状態,
        &format!("/projects/{}/parts/{part_id}", 場.project.id),
        &token,
    )
    .await
}

async fn 登録(
    db: &DatabaseConnection,
    場: &舞台情報,
    part_catalog_id: &str,
    serial: &str,
    place: &str,
) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(
        状態,
        &format!("/projects/{}/parts", 場.project.id),
        &token,
        &[
            ("part_catalog_id", part_catalog_id),
            ("serial_number", serial),
            ("status", "standby"),
            ("health", "ok"),
            ("place", place),
        ],
    )
    .await
}

async fn 移す(
    db: &DatabaseConnection,
    場: &舞台情報,
    part_id: i32,
    place: &str,
) -> (StatusCode, String) {
    let (状態, token) = 認証済み(db, &場.user).await;
    送信(
        状態,
        &format!("/projects/{}/parts/{part_id}/move", 場.project.id),
        &token,
        &[("place", place)],
    )
    .await
}

async fn 部品(db: &DatabaseConnection, serial: &str) -> part_instance::Model {
    part_instance::Entity::find()
        .filter(part_instance::Column::SerialNumber.eq(serial))
        .one(db)
        .await
        .unwrap()
        .expect("部品が作られていません")
}

async fn 履歴(db: &DatabaseConnection, id: i32) -> Vec<part_instance_location::Model> {
    part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(id))
        .order_by_asc(part_instance_location::Column::Id)
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
        全検証!(@one $用意, $属性, 登録すると一覧に出る);
        全検証!(@one $用意, $属性, 移すと閉じて開く);
        全検証!(@one $用意, $属性, ケーブルが挿さっていると移せない);
        全検証!(@one $用意, $属性, このプロジェクトの外には置けない);
        全検証!(@one $用意, $属性, 他のプロジェクトの部品は扱えない);
        全検証!(@one $用意, $属性, 重複と廃番は登録できない);
        全検証!(@one $用意, $属性, 閲覧者は書き換えられない);
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
