//! 費用取込の結合テスト（設計書23.5、10.2、10.3、24.2.1、24.2.2）。
//!
//! # 何を確かめているか
//!
//! **金額を画面と同じ経路で解釈すること**（24.2.1）。桁数は通貨から決まり、
//! 円に小数は無い。ここがずれると**金額が100倍ずれる。**
//!
//! **按分できない入力を取り込まないこと**（24.2.2）。耐用年数0や逆転した
//! 契約期間を入れると、10.3の集計がその行を黙って落とす。
//!
//! **多態的参照を型＋自然キーで引けること**（23.5）。`item_id` は人が書く
//! ファイルに現れない。

mod support;

use chrono::{Duration, Utc};
use dioryga::import::{costs, Outcome};
use dioryga::repository::{Actor, AuditedTx};
use entity::{
    app_user, device, device_assignment, fixed_asset, maintenance_contract,
    maintenance_contract_item, part_catalog, part_instance, part_instance_location, project,
    purchase, vendor,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, PaginatorTrait, Set};

const 購入見出し: &str =
    "item_type,item_hostname,item_serial_number,order_number,acquired_on,amount,supplier\n";
const 資産見出し: &str =
    "item_type,item_hostname,item_serial_number,acquisition_cost,depreciation_method,useful_life_years,acquisition_date\n";
const 契約見出し: &str =
    "contract_number,vendor,start_date,end_date,amount,quote_contact,failure_contact,item_type,item_hostname,item_serial_number\n";

fn 購入(rows: &[&str]) -> Vec<costs::PurchaseRow> {
    costs::parse_purchases(&format!("{購入見出し}{}", rows.join("\n"))).unwrap()
}
fn 資産(rows: &[&str]) -> Vec<costs::FixedAssetRow> {
    costs::parse_fixed_assets(&format!("{資産見出し}{}", rows.join("\n"))).unwrap()
}
fn 契約(rows: &[&str]) -> Vec<costs::MaintenanceContractRow> {
    costs::parse_maintenance_contracts(&format!("{契約見出し}{}", rows.join("\n"))).unwrap()
}

// ---------------------------------------------------------------------------
// 購入（10.2）
// ---------------------------------------------------------------------------

/// **購入の記録を登録でき、二度流しても増えないこと。**
async fn 購入を登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "購入", "JPY").await;
    let rows = 購入(&["Device,web01,,PO-1,2026-04-01,600000,〇〇商事"]);

    let tx = 取込(db).await;
    let r = costs::購入を取り込む(&tx, 場.project.id, &rows, "JPY")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Created), 1, "{r}");

    let p = purchase::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(p.amount, 600000);
    assert_eq!(p.order_number.as_deref(), Some("PO-1"));
    assert_eq!(p.supplier.as_deref(), Some("〇〇商事"));
    assert_eq!(p.acquired_on, chrono::NaiveDate::from_ymd_opt(2026, 4, 1));
    assert_eq!(p.item_type, "Device");
    assert_eq!(p.item_id, 場.device.id);

    let tx = 取込(db).await;
    let r = costs::購入を取り込む(&tx, 場.project.id, &rows, "JPY")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Unchanged), 1, "{r}");
    assert_eq!(purchase::Entity::find().count(db).await.unwrap(), 1);
}

/// **品目で突合し、金額が変われば上書きすること。**発注番号は重複してよいため、
/// 突合の鍵にしない。
async fn 購入は品目で突合する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "購入更新", "JPY").await;
    let tx = 取込(db).await;
    costs::購入を取り込む(
        &tx,
        場.project.id,
        &購入(&["Device,web01,,PO-1,2026-04-01,600000,"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let tx = 取込(db).await;
    let r = costs::購入を取り込む(
        &tx,
        場.project.id,
        &購入(&["Device,web01,,PO-2,2026-04-01,650000,"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Updated), 1, "{r}");
    let p = purchase::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(p.amount, 650000);
    assert_eq!(p.order_number.as_deref(), Some("PO-2"));
    assert_eq!(
        purchase::Entity::find().count(db).await.unwrap(),
        1,
        "購入の記録が増えています"
    );
}

/// **発注番号・取得日・購入元は空でよいこと。**取得日の無い購入は、年間コストの
/// 合算から外して画面に列挙する側で扱う（10.3）。
async fn 取得日と発注番号は空でよい(db: &DatabaseConnection) {
    let 場 = 舞台(db, "空欄", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::購入を取り込む(
        &tx,
        場.project.id,
        &購入(&["Device,web01,,,,600000,"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Created), 1, "{r}");
    let p = purchase::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(p.order_number, None);
    assert_eq!(p.acquired_on, None);
    assert_eq!(p.supplier, None);
}

/// **読めない取得日はエラーにすること。**空欄と違い、書いた値が解釈できない。
async fn 読めない取得日はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "日付", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::購入を取り込む(
        &tx,
        場.project.id,
        &購入(&["Device,web01,,PO-1,2026/04/01,600000,"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// 金額の解釈（24.2.1）
// ---------------------------------------------------------------------------

/// **円に小数は無い**（24.2.1）。黙って切り捨てると入力の意図が失われる。
async fn 円に小数はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "円小数", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,1200000.5,straight_line,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **USDでは小数2桁を受け、最小通貨単位へ直すこと**（24.2.1）。
///
/// ここを取り違えると金額が100倍ずれる。
async fn usdは小数2桁を受ける(db: &DatabaseConnection) {
    let 場 = 舞台(db, "USD資産", "USD").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,12000.50,straight_line,5,2026-04-01"]),
        "USD",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Created), 1, "{r}");
    let a = fixed_asset::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(a.acquisition_cost, 1_200_050);
}

/// **負の金額は受けない**（24.2.2の「金額が負」）。
async fn 負の金額はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "負金額", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,-100,straight_line,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// 固定資産（10.3、24.2.2）
// ---------------------------------------------------------------------------

/// **1つの品目につき1件であること。**2件あると10.3が二重に数える。
async fn 資産は品目につき1件(db: &DatabaseConnection) {
    let 場 = 舞台(db, "資産1件", "JPY").await;

    let tx = 取込(db).await;
    costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,1200000,straight_line,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,1500000,straight_line,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Updated), 1, "{r}");
    assert_eq!(fixed_asset::Entity::find().count(db).await.unwrap(), 1);
    let a = fixed_asset::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(a.acquisition_cost, 1_500_000);
}

/// **耐用年数が0以下では按分できない**（24.2.2）。
async fn 耐用年数が0以下ならエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "耐用年数", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,1200000,straight_line,0,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **償却方法の語彙外は拒否すること**（8.6）。
async fn 語彙外の償却方法は拒否(db: &DatabaseConnection) {
    let 場 = 舞台(db, "償却方法", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,1200000,てきとう,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **定率法も値として記録すること**（10.3）。按分しないだけで、入力は受ける。
async fn 定率法も記録する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "定率法", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["Device,web01,,1200000,declining_balance,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Created), 1, "{r}");
    let a = fixed_asset::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(a.depreciation_method, "declining_balance");
}

// ---------------------------------------------------------------------------
// 多態的参照（23.5）
// ---------------------------------------------------------------------------

/// **部品にも費用を付けられること**（23.5）。`item_type` で見る列が変わる。
async fn 部品にも資産を付けられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品資産", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["PartInstance,,SN-P1,48000,straight_line,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Created), 1, "{r}");
    let a = fixed_asset::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(a.item_type, "PartInstance");
    assert_eq!(a.item_id, 場.part_id);
}

/// **`PartInstance` にシリアルが無ければエラー。**引く手段が無い。
async fn 部品にシリアルが無ければエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "部品シリアル", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["PartInstance,,,48000,straight_line,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **扱えない `item_type` は拒否すること。**
async fn 知らない品目の型はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "品目型", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::固定資産を取り込む(
        &tx,
        場.project.id,
        &資産(&["SoftwareInstance,web01,,48000,straight_line,5,2026-04-01"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// 保守契約（10.2、24.2.2）
// ---------------------------------------------------------------------------

/// **1契約が複数品目をカバーできること**（10.2）。
async fn 契約は複数品目をカバーする(db: &DatabaseConnection) {
    let 場 = 舞台(db, "契約複数", "JPY").await;
    let rows = 契約(&[
        "CT-1,Fujitsu,2026-04-01,2027-03-31,240000,q@example.com,f@example.com,Device,web01,",
        "CT-1,Fujitsu,2026-04-01,2027-03-31,240000,q@example.com,f@example.com,PartInstance,,SN-P1",
    ]);

    let tx = 取込(db).await;
    let r = costs::保守契約を取り込む(&tx, 場.project.id, &rows, "JPY")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(!r.has_error(), "{r}");
    assert_eq!(
        maintenance_contract::Entity::find()
            .count(db)
            .await
            .unwrap(),
        1,
        "契約が重複して作られています"
    );
    assert_eq!(
        maintenance_contract_item::Entity::find()
            .count(db)
            .await
            .unwrap(),
        2
    );

    // 二度流しても品目は増えない
    let tx = 取込(db).await;
    costs::保守契約を取り込む(&tx, 場.project.id, &rows, "JPY")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        maintenance_contract_item::Entity::find()
            .count(db)
            .await
            .unwrap(),
        2,
        "同じ品目を二重に付けています"
    );
}

/// **契約期間が逆転していたらエラー**（24.2.2の「期間が0以下」）。
async fn 期間が逆ならエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "契約期間", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::保守契約を取り込む(
        &tx,
        場.project.id,
        &契約(&["CT-1,Fujitsu,2027-03-31,2026-04-01,240000,,,Device,web01,"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **同じ契約番号で内容が食い違う行はエラー。**どちらが正しいか決められない。
async fn 同じ契約で内容が食い違えばエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "契約食い違い", "JPY").await;
    let rows = 契約(&[
        "CT-1,Fujitsu,2026-04-01,2027-03-31,240000,,,Device,web01,",
        "CT-1,Fujitsu,2026-04-01,2027-03-31,999999,,,PartInstance,,SN-P1",
    ]);

    let tx = 取込(db).await;
    let r = costs::保守契約を取り込む(&tx, 場.project.id, &rows, "JPY")
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **知らないベンダーはエラー。**黙って作らない（カタログは18章の対象）。
async fn 知らないベンダーはエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "契約ベンダー", "JPY").await;
    let tx = 取込(db).await;
    let r = costs::保守契約を取り込む(
        &tx,
        場.project.id,
        &契約(&["CT-1,知らない社,2026-04-01,2027-03-31,240000,,,Device,web01,"]),
        "JPY",
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// 用意
// ---------------------------------------------------------------------------

struct 舞台情報 {
    project: project::Model,
    device: device::Model,
    part_id: i32,
}

async fn 取込(db: &DatabaseConnection) -> AuditedTx {
    AuditedTx::begin(db, Actor::Import { import_run_id: 1 })
        .await
        .unwrap()
}

/// Fujitsu、web01、web01に載った部品 SN-P1 を用意する。
async fn 舞台(db: &DatabaseConnection, name: &str, currency: &str) -> 舞台情報 {
    let p = project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(name.to_owned()),
        description: Set(String::new()),
        currency: Set(currency.to_owned()),
        archived_at: Set(None),
        closure_reason: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let u = app_user::ActiveModel {
        name: Set("費用".to_owned()),
        username: Set((format!("{}@example.com", uuid::Uuid::new_v4())).replace('@', "_")),
        email: Set(Some(format!("{}@example.com", uuid::Uuid::new_v4()))),
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
    .unwrap();

    let v = vendor::ActiveModel {
        name: Set("Fujitsu".to_owned()),
        created_by: Set(u.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        configuration_id: Set(None),
        hostname: Set("web01".to_owned()),
        device_type: Set("Physical".to_owned()),
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

    let c = part_catalog::ActiveModel {
        category: Set("Memory".to_owned()),
        vendor_id: Set(v.id),
        part_number: Set("DIMM-32G".to_owned()),
        core_count: Set(None),
        capacity_gb: Set(Some(32)),
        spec_json: Set("{}".to_owned()),
        retired_at: Set(None),
        created_by: Set(u.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let pi = part_instance::ActiveModel {
        part_catalog_id: Set(c.id),
        serial_number: Set(Some("SN-P1".to_owned())),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    part_instance_location::ActiveModel {
        part_instance_id: Set(pi.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(d.id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(10)),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    舞台情報 {
        project: p,
        device: d,
        part_id: pi.id,
    }
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 購入を登録できる);
        全検証!(@one $用意, $属性, 購入は品目で突合する);
        全検証!(@one $用意, $属性, 取得日と発注番号は空でよい);
        全検証!(@one $用意, $属性, 読めない取得日はエラー);
        全検証!(@one $用意, $属性, 円に小数はエラー);
        全検証!(@one $用意, $属性, usdは小数2桁を受ける);
        全検証!(@one $用意, $属性, 負の金額はエラー);
        全検証!(@one $用意, $属性, 資産は品目につき1件);
        全検証!(@one $用意, $属性, 耐用年数が0以下ならエラー);
        全検証!(@one $用意, $属性, 語彙外の償却方法は拒否);
        全検証!(@one $用意, $属性, 定率法も記録する);
        全検証!(@one $用意, $属性, 部品にも資産を付けられる);
        全検証!(@one $用意, $属性, 部品にシリアルが無ければエラー);
        全検証!(@one $用意, $属性, 知らない品目の型はエラー);
        全検証!(@one $用意, $属性, 契約は複数品目をカバーする);
        全検証!(@one $用意, $属性, 期間が逆ならエラー);
        全検証!(@one $用意, $属性, 同じ契約で内容が食い違えばエラー);
        全検証!(@one $用意, $属性, 知らないベンダーはエラー);
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
