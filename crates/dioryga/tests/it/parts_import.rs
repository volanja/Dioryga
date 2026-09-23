//! 部品の取込の結合テスト（設計書23.5、6.2、12.4、23.10）。
//!
//! # 何を確かめているか
//!
//! **シリアル番号を必須にすること。**部品は識別子をこれしか持たず、空のまま
//! 受け付けると突合できずに毎回新規になる——**二度流すと部品が倍になる。**
//!
//! **シリアルはベンダーの中で一意とみなすこと**（23.10）。別ベンダーの同じ
//! 番号を同一とみなすと、実在する別々の部品を取り違える。
//!
//! **このプロジェクトに関わった部品だけを動かせること。**候補外の部品を
//! 書き換えることも、黙って2つ目を作ることもしない。

use chrono::{Duration, Utc};
use dioryga::import::{parts, Outcome};
use entity::{
    app_user, chassis_model, chassis_slot, configuration, device, device_assignment, part_catalog,
    part_instance, part_instance_location, project, vendor, warehouse,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

const 見出し: &str = "serial_number,part_vendor,part_number,status,location_type,location_hostname,location_name,chassis_slot\n";

fn csv(rows: &[&str]) -> String {
    format!("{見出し}{}", rows.join("\n"))
}

// ---------------------------------------------------------------------------
// 登録と識別
// ---------------------------------------------------------------------------

/// **機器に載った部品を新規登録できること。**
async fn 機器に載せて登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品登録").await;

    let rows = parts::parse_parts(&csv(&["SN-1,Samsung,DIMM-32G,running,Device,web01,,"])).unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Created), 1, "{report}");

    parts::apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();

    let p = 部品(db, "SN-1").await.expect("部品が作られていません");
    let l = 現在の所在(db, p.id).await.expect("所在がありません");
    assert_eq!(l.location_type, "Device");
    assert_eq!(l.location_id, Some(場.device.id));
}

/// **シリアル番号が無ければエラー。**二度流すと部品が倍になるため。
async fn シリアルは必須(db: &DatabaseConnection) {
    let 場 = 舞台(db, "シリアル必須").await;

    let rows = parts::parse_parts(&csv(&[",Samsung,DIMM-32G,running,Device,web01,,"])).unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **二度流しても部品も所在も増えないこと**（23.1）。
async fn 二度流しても増えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品冪等").await;

    let rows = parts::parse_parts(&csv(&["SN-1,Samsung,DIMM-32G,running,Device,web01,,"])).unwrap();
    parts::apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();

    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Unchanged), 1, "{report}");

    parts::apply(db, 場.project.id, &rows, Utc::now(), 2)
        .await
        .unwrap();
    assert_eq!(part_instance::Entity::find().count(db).await.unwrap(), 1);
    assert_eq!(
        part_instance_location::Entity::find()
            .count(db)
            .await
            .unwrap(),
        1,
        "所在の履歴が増えています"
    );
}

/// **別ベンダーの同じシリアルは別の部品であること**（23.10）。
///
/// 全体で一意とみなすと、実在する別々の部品を取り違える。
async fn ベンダーが違えば別の部品(db: &DatabaseConnection) {
    let 場 = 舞台(db, "ベンダー別").await;
    let _ = カタログ(db, 場.user_id, "Hynix", "DIMM-32G").await;

    let rows = parts::parse_parts(&csv(&[
        "SN-SAME,Samsung,DIMM-32G,running,Device,web01,,",
        "SN-SAME,Hynix,DIMM-32G,running,Device,web01,,",
    ]))
    .unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Created), 2, "{report}");

    parts::apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();
    assert_eq!(part_instance::Entity::find().count(db).await.unwrap(), 2);
}

/// **同じベンダー・同じシリアルが2行あればエラー**。どちらが正しいか決められない。
async fn ファイル内の重複はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品重複").await;

    let rows = parts::parse_parts(&csv(&[
        "SN-1,Samsung,DIMM-32G,running,Device,web01,,",
        "SN-1,Samsung,DIMM-32G,running,Warehouse,,本社倉庫,",
    ]))
    .unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **このプロジェクトに関わったことのない部品は動かせないこと。**
///
/// 倉庫にある部品でも、関わりが無ければ取込では引き込めない。倉庫からの
/// 払い出しは変更管理チケットの担当（11章）。黙って2つ目も作らない。
async fn 関わりの無い部品は動かせない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品候補外").await;

    // どのプロジェクトにも関わっていない、倉庫の部品
    let c = カタログを引く(db, "Samsung", "DIMM-32G").await;
    let p = part_instance::ActiveModel {
        part_catalog_id: Set(c.id),
        serial_number: Set(Some("SN-STOCK".to_owned())),
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

    let rows =
        parts::parse_parts(&csv(&["SN-STOCK,Samsung,DIMM-32G,running,Device,web01,,"])).unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();

    assert_eq!(report.count(Outcome::Error), 1, "{report}");
    assert_eq!(
        part_instance::Entity::find().count(db).await.unwrap(),
        1,
        "2つ目を作っています"
    );
}

// ---------------------------------------------------------------------------
// 所在（12.4）
// ---------------------------------------------------------------------------

/// **機器から倉庫へ移すと閉じて開くこと**（4章、12.4）。
async fn 倉庫へ戻すと閉じて開く(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品移動").await;

    let 載せる =
        parts::parse_parts(&csv(&["SN-1,Samsung,DIMM-32G,running,Device,web01,,"])).unwrap();
    parts::apply(
        db,
        場.project.id,
        &載せる,
        Utc::now() - Duration::days(5),
        1,
    )
    .await
    .unwrap();

    let 戻す = parts::parse_parts(&csv(&[
        "SN-1,Samsung,DIMM-32G,running,Warehouse,,本社倉庫,",
    ]))
    .unwrap();
    let report = parts::dry_run(db, 場.project.id, &戻す).await.unwrap();
    assert_eq!(report.count(Outcome::Updated), 1, "{report}");

    parts::apply(db, 場.project.id, &戻す, Utc::now(), 2)
        .await
        .unwrap();

    let p = 部品(db, "SN-1").await.unwrap();
    let 履歴 = 所在の履歴(db, p.id).await;
    assert_eq!(履歴.len(), 2, "閉じて開いていません");
    assert!(履歴[0].to_date.is_some());
    assert_eq!(履歴[1].location_type, "Warehouse");
}

/// **状態だけが変わったときは、所在の履歴を作らないこと。**
///
/// 部品そのものは履歴ではなく、**所在だけが履歴**である（12.4）。
async fn 状態だけの変更で所在は増えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "状態変更").await;

    let 元 = parts::parse_parts(&csv(&["SN-1,Samsung,DIMM-32G,running,Device,web01,,"])).unwrap();
    parts::apply(db, 場.project.id, &元, Utc::now(), 1)
        .await
        .unwrap();

    let 故障 = parts::parse_parts(&csv(&["SN-1,Samsung,DIMM-32G,failed,Device,web01,,"])).unwrap();
    let report = parts::dry_run(db, 場.project.id, &故障).await.unwrap();
    assert_eq!(report.count(Outcome::Updated), 1, "{report}");

    parts::apply(db, 場.project.id, &故障, Utc::now(), 2)
        .await
        .unwrap();

    let p = 部品(db, "SN-1").await.unwrap();
    assert_eq!(p.status, "failed");
    assert_eq!(所在の履歴(db, p.id).await.len(), 1, "所在が増えています");
}

/// **スロットをラベルで指定できること**（6.2）。
async fn スロットを指定できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "スロット").await;

    let rows = parts::parse_parts(&csv(&[
        "SN-1,Samsung,DIMM-32G,running,Device,web01,,DIMM_A1",
    ]))
    .unwrap();
    parts::apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();

    let p = 部品(db, "SN-1").await.unwrap();
    let l = 現在の所在(db, p.id).await.unwrap();
    assert_eq!(l.chassis_slot_id, Some(場.slot_id));
}

/// **筐体に無いスロットはエラー。**
async fn 無いスロットはエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "無スロット").await;

    let rows = parts::parse_parts(&csv(&[
        "SN-1,Samsung,DIMM-32G,running,Device,web01,,DIMM_Z9",
    ]))
    .unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **倉庫にある部品にスロットは付けられないこと。**挿さっている先が無い。
async fn 倉庫ではスロットを指定できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "倉庫スロット").await;

    let rows = parts::parse_parts(&csv(&[
        "SN-1,Samsung,DIMM-32G,running,Warehouse,,本社倉庫,DIMM_A1",
    ]))
    .unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **部品カタログが無ければエラー。**黙って作らない（カタログは18章の対象）。
async fn 知らないカタログはエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "未知カタログ").await;

    let rows = parts::parse_parts(&csv(&["SN-1,Samsung,NOPE-1,running,Device,web01,,"])).unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **状態の語彙外は拒否すること**（8.6）。
async fn 語彙外の状態は拒否(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品語彙").await;

    let rows =
        parts::parse_parts(&csv(&["SN-1,Samsung,DIMM-32G,こわれた,Device,web01,,"])).unwrap();
    let report = parts::dry_run(db, 場.project.id, &rows).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

// ---------------------------------------------------------------------------
// 用意
// ---------------------------------------------------------------------------

struct 舞台情報 {
    project: project::Model,
    device: device::Model,
    warehouse: warehouse::Model,
    slot_id: i32,
    user_id: i32,
}

/// Samsung DIMM-32G のカタログ、DIMM_A1 を持つ筐体の web01、本社倉庫を用意する。
async fn 舞台(db: &DatabaseConnection, name: &str) -> 舞台情報 {
    let p = プロジェクト(db, name).await;
    let user_id = 利用者(db, &format!("{name}@example.com")).await;

    let v = ベンダー(db, user_id, "Samsung").await;
    part_catalog::ActiveModel {
        category: Set("Memory".to_owned()),
        vendor_id: Set(v),
        part_number: Set("DIMM-32G".to_owned()),
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

    let fv = ベンダー(db, user_id, &format!("筐体-{name}")).await;
    let m = chassis_model::ActiveModel {
        vendor_id: Set(fv),
        model_name: Set(format!("型-{name}")),
        device_category: Set("Server".to_owned()),
        height_u: Set(1),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(Some("Full".to_owned())),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let slot = chassis_slot::ActiveModel {
        chassis_model_id: Set(m.id),
        slot_type: Set("DIMM".to_owned()),
        slot_label: Set("DIMM_A1".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let c = configuration::ActiveModel {
        chassis_model_id: Set(m.id),
        name: Set("標準構成".to_owned()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(Some(c.id)),
        hostname: Set("web01".to_owned()),
        device_type: Set("Physical".to_owned()),
        serial_number: Set(Some(format!("SV-{name}"))),
        power_watt: Set(400),
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
        from_date: Set(Utc::now() - Duration::days(30)),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let w = warehouse::ActiveModel {
        name: Set("本社倉庫".to_owned()),
        address: Set("東京".to_owned()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    舞台情報 {
        project: p,
        device: d,
        warehouse: w,
        slot_id: slot.id,
        user_id,
    }
}

async fn ベンダー(db: &DatabaseConnection, user_id: i32, name: &str) -> i32 {
    vendor::ActiveModel {
        name: Set(name.to_owned()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn カタログ(
    db: &DatabaseConnection,
    user_id: i32,
    vendor_name: &str,
    part_number: &str,
) -> part_catalog::Model {
    let v = ベンダー(db, user_id, vendor_name).await;
    part_catalog::ActiveModel {
        category: Set("Memory".to_owned()),
        vendor_id: Set(v),
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
    .unwrap()
}

async fn カタログを引く(
    db: &DatabaseConnection,
    vendor_name: &str,
    part_number: &str,
) -> part_catalog::Model {
    let v = vendor::Entity::find()
        .filter(vendor::Column::Name.eq(vendor_name))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    part_catalog::Entity::find()
        .filter(part_catalog::Column::VendorId.eq(v.id))
        .filter(part_catalog::Column::PartNumber.eq(part_number))
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

async fn 部品(db: &DatabaseConnection, serial: &str) -> Option<part_instance::Model> {
    part_instance::Entity::find()
        .filter(part_instance::Column::SerialNumber.eq(serial))
        .one(db)
        .await
        .unwrap()
}

async fn 現在の所在(
    db: &DatabaseConnection,
    id: i32,
) -> Option<part_instance_location::Model> {
    part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
}

async fn 所在の履歴(db: &DatabaseConnection, id: i32) -> Vec<part_instance_location::Model> {
    part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(id))
        .order_by_asc(part_instance_location::Column::FromDate)
        .order_by_asc(part_instance_location::Column::Id)
        .all(db)
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

async fn 利用者(db: &DatabaseConnection, email: &str) -> i32 {
    app_user::ActiveModel {
        name: Set("部品".to_owned()),
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
    .id
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 機器に載せて登録できる);
        全検証!(@one $用意, $属性, シリアルは必須);
        全検証!(@one $用意, $属性, 二度流しても増えない);
        全検証!(@one $用意, $属性, ベンダーが違えば別の部品);
        全検証!(@one $用意, $属性, ファイル内の重複はエラー);
        全検証!(@one $用意, $属性, 関わりの無い部品は動かせない);
        全検証!(@one $用意, $属性, 倉庫へ戻すと閉じて開く);
        全検証!(@one $用意, $属性, 状態だけの変更で所在は増えない);
        全検証!(@one $用意, $属性, スロットを指定できる);
        全検証!(@one $用意, $属性, 無いスロットはエラー);
        全検証!(@one $用意, $属性, 倉庫ではスロットを指定できない);
        全検証!(@one $用意, $属性, 知らないカタログはエラー);
        全検証!(@one $用意, $属性, 語彙外の状態は拒否);
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
