//! 配置の取込の結合テスト（設計書23.1、23.5、12.3）。
//!
//! # 何を確かめているか
//!
//! **宣言であって命令ではないこと**（23.1）。同じファイルを2回流しても履歴行が
//! 増えない。現在の事実と一致していれば何も書かない。**命令形なら2回流した
//! 時点で事実が壊れる**ため、ここが崩れると取込の前提そのものが失われる。
//!
//! **変更は閉じて開くこと**（4章、不変条件1/2）。既存行を書き換えると、
//! いつ移ったのかが失われる。
//!
//! **他プロジェクトへ移す取込を受け付けないこと**（23.5）。受け付けると、
//! **取込ファイル1つで他プロジェクトのデータを書き換えられる。**
//!
//! **重複配置は警告に留めること**（12.3、不変条件6）。実機が規則の想定外で
//! あることはあり、誤って拒否すると事実を記録できなくなる。

mod support;

use chrono::{Duration, Utc};
use dioryga::import::{placement, Outcome};
use entity::{device, device_assignment, device_mount, mount_container, project, warehouse};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

const 所属見出し: &str = "uid,external_id,hostname,serial_number,location_type,location_name\n";
const 搭載見出し: &str = "uid,external_id,hostname,serial_number,container,position,horizontal_position,depth_position,host_hostname\n";

fn 所属csv(rows: &[&str]) -> String {
    format!("{所属見出し}{}", rows.join("\n"))
}

fn 搭載csv(rows: &[&str]) -> String {
    format!("{搭載見出し}{}", rows.join("\n"))
}

// ---------------------------------------------------------------------------
// 所属（DEVICE_ASSIGNMENT）
// ---------------------------------------------------------------------------

/// **倉庫へ払い出せること。**閉じて開く（4章）。
async fn 倉庫へ払い出せる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "払い出し").await;
    let d = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_assignments(&所属csv(&[",,web01,,Warehouse,本社倉庫"])).unwrap();

    let report = placement::assignments_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Updated), 1, "{report}");

    placement::assignments_apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();

    let 履歴 = 所属の履歴(db, d.id).await;
    assert_eq!(履歴.len(), 2, "閉じて開いていません");
    assert_eq!(履歴[0].location_type, "Project");
    assert!(履歴[0].to_date.is_some(), "前の行を閉じていません");
    assert_eq!(履歴[1].location_type, "Warehouse");
    assert_eq!(履歴[1].location_id, Some(場.warehouse.id));
    assert!(履歴[1].to_date.is_none());
}

/// **廃棄は参照先を持たないこと**（12章）。
async fn 廃棄は参照先を持たない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "廃棄").await;
    let d = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_assignments(&所属csv(&[",,web01,,Disposed,"])).unwrap();
    placement::assignments_apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();

    let 履歴 = 所属の履歴(db, d.id).await;
    assert_eq!(履歴[1].location_type, "Disposed");
    assert!(履歴[1].location_id.is_none());
}

/// **他プロジェクトへ移す取込は受け付けないこと**（23.5）。
///
/// 受け付けると、取込ファイル1つで他プロジェクトのデータを書き換えられる。
/// 移譲は11章の変更管理チケット（Transfer）の担当である。
async fn 他プロジェクトへは移せない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "移譲").await;
    let _ = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    // プロジェクト名を location_name に書いても通らないこと
    let rows =
        placement::parse_assignments(&所属csv(&[",,web01,,OtherProject,よそのプロジェクト"]))
            .unwrap();

    let report = placement::assignments_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
    assert!(
        report.errors().any(|e| e.detail.contains("location_type")),
        "理由が location_type に触れていません"
    );
}

/// **倉庫名が解決できなければエラー。**黙って別の倉庫に入れない。
async fn 知らない倉庫はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "未知倉庫").await;
    let _ = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_assignments(&所属csv(&[",,web01,,Warehouse,無い倉庫"])).unwrap();
    let report = placement::assignments_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **廃止した倉庫は置き場にできないこと**（#133）。
///
/// 廃止は「もう使わない」という表明であり、取込から入れられると意味がない。
async fn 廃止した倉庫はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "廃止倉庫").await;
    let _ = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let mut w: warehouse::ActiveModel = 場.warehouse.clone().into();
    w.retired_at = Set(Some(Utc::now()));
    w.update(db).await.unwrap();

    let rows = placement::parse_assignments(&所属csv(&[",,web01,,Warehouse,本社倉庫"])).unwrap();
    let report = placement::assignments_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **`Warehouse` に倉庫名が無ければエラー。**どこへ入れるか決められない。
async fn 倉庫名が無ければエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "倉庫名なし").await;
    let _ = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_assignments(&所属csv(&[",,web01,,Warehouse,"])).unwrap();
    let report = placement::assignments_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **二度流しても履歴が増えないこと**（23.1）。
///
/// 宣言的であることの核心。命令形なら2回目で履歴行が重複し、事実が壊れる。
async fn 所属は二度流しても増えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "冪等").await;
    let d = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_assignments(&所属csv(&[",,web01,,Warehouse,本社倉庫"])).unwrap();
    placement::assignments_apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();
    assert_eq!(所属の履歴(db, d.id).await.len(), 2);

    // 2回目。**現在の事実と一致するので何も書かない**
    let report = placement::assignments_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Unchanged), 1, "{report}");

    placement::assignments_apply(db, 場.project.id, &rows, Utc::now(), 2)
        .await
        .unwrap();
    assert_eq!(所属の履歴(db, d.id).await.len(), 2, "履歴が増えています");
}

/// **機器を特定できなければエラー。**黙って読み飛ばさない。
async fn 知らない機器はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "未知機器").await;

    let rows = placement::parse_assignments(&所属csv(&[",,いない,,Warehouse,本社倉庫"])).unwrap();
    let report = placement::assignments_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

// ---------------------------------------------------------------------------
// 搭載（DEVICE_MOUNT）
// ---------------------------------------------------------------------------

/// **什器に載せられること**（12.3）。
async fn 什器に載せられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "搭載").await;
    let d = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_mounts(&搭載csv(&[",,web01,,Rack-01,10,Full,Front,"])).unwrap();

    let report = placement::mounts_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Created), 1, "{report}");

    placement::mounts_apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();

    let m = 現在の搭載(db, d.id).await.expect("搭載されていません");
    assert_eq!(m.container_id, Some(場.container.id));
    assert_eq!(m.position, Some(10));
    assert_eq!(m.horizontal_position.as_deref(), Some("Full"));
}

/// **他の機器の上に載せられること**（12.3、13章）。
///
/// 棚板の上のNAS、ハイパーバイザ上のVMが同じ形になる。
async fn 機器の上に載せられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "ホスト").await;
    let host = 機器(db, 場.project.id, "esxi01", Some("SN-H")).await;
    let vm = 機器(db, 場.project.id, "vm01", Some("SN-V")).await;

    let rows = placement::parse_mounts(&搭載csv(&[",,vm01,,,,,,esxi01"])).unwrap();
    placement::mounts_apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();

    let m = 現在の搭載(db, vm.id).await.expect("搭載されていません");
    assert_eq!(m.host_device_id, Some(host.id));
    assert!(m.container_id.is_none(), "什器と両立させています");
}

/// **什器と host_hostname は同時に指定できないこと**（12.3）。
///
/// どちらに載っているのか決められない。
async fn 什器とホストは併記できない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "併記").await;
    let _ = 機器(db, 場.project.id, "esxi01", Some("SN-H")).await;
    let _ = 機器(db, 場.project.id, "vm01", Some("SN-V")).await;

    let rows = placement::parse_mounts(&搭載csv(&[",,vm01,,Rack-01,10,,,esxi01"])).unwrap();
    let report = placement::mounts_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **自分自身の上には載せられないこと。**辿ると止まらなくなる。
async fn 自分の上には載せられない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "自己").await;
    let _ = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_mounts(&搭載csv(&[",,web01,,,,,,web01"])).unwrap();
    let report = placement::mounts_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

/// **同じUの衝突は警告に留め、取り込むこと**（12.3、不変条件6）。
///
/// 実機が規則の想定外であることはあり、**誤って拒否すると事実を記録できない。**
async fn 重複配置は警告(db: &DatabaseConnection) {
    let 場 = 舞台(db, "重複").await;
    let 先客 = 機器(db, 場.project.id, "web01", Some("SN-1")).await;
    let 後客 = 機器(db, 場.project.id, "web02", Some("SN-2")).await;

    let 一台目 = placement::parse_mounts(&搭載csv(&[",,web01,,Rack-01,10,Full,Full,"])).unwrap();
    placement::mounts_apply(db, 場.project.id, &一台目, Utc::now(), 1)
        .await
        .unwrap();

    let 二台目 = placement::parse_mounts(&搭載csv(&[",,web02,,Rack-01,10,Full,Full,"])).unwrap();
    let report = placement::mounts_dry_run(db, 場.project.id, &二台目)
        .await
        .unwrap();

    assert_eq!(report.count(Outcome::Warning), 1, "{report}");
    assert!(!report.has_error(), "警告で止めてはなりません");

    // **取り込めること。**警告は記録するが拒否しない
    placement::mounts_apply(db, 場.project.id, &二台目, Utc::now(), 2)
        .await
        .unwrap();
    assert!(現在の搭載(db, 後客.id).await.is_some());
    assert!(
        現在の搭載(db, 先客.id).await.is_some(),
        "先客が消えています"
    );
}

/// **左右で分かれていれば警告を出さないこと**（12.3）。
async fn 左右に分かれていれば警告しない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "左右").await;
    let _ = 機器(db, 場.project.id, "web01", Some("SN-1")).await;
    let _ = 機器(db, 場.project.id, "web02", Some("SN-2")).await;

    let 左 = placement::parse_mounts(&搭載csv(&[",,web01,,Rack-01,10,Left,Full,"])).unwrap();
    placement::mounts_apply(db, 場.project.id, &左, Utc::now(), 1)
        .await
        .unwrap();

    let 右 = placement::parse_mounts(&搭載csv(&[",,web02,,Rack-01,10,Right,Full,"])).unwrap();
    let report = placement::mounts_dry_run(db, 場.project.id, &右)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Warning), 0, "{report}");
    assert_eq!(report.count(Outcome::Created), 1, "{report}");
}

/// **搭載も二度流して増えないこと**（23.1）。
async fn 搭載は二度流しても増えない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "搭載冪等").await;
    let d = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let rows = placement::parse_mounts(&搭載csv(&[",,web01,,Rack-01,10,Full,Front,"])).unwrap();
    placement::mounts_apply(db, 場.project.id, &rows, Utc::now(), 1)
        .await
        .unwrap();
    assert_eq!(搭載の件数(db, d.id).await, 1);

    let report = placement::mounts_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Unchanged), 1, "{report}");

    placement::mounts_apply(db, 場.project.id, &rows, Utc::now(), 2)
        .await
        .unwrap();
    assert_eq!(搭載の件数(db, d.id).await, 1, "履歴が増えています");
}

/// **位置が変われば閉じて開くこと**（4章）。
async fn 移設は閉じて開く(db: &DatabaseConnection) {
    let 場 = 舞台(db, "移設").await;
    let d = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let 元 = placement::parse_mounts(&搭載csv(&[",,web01,,Rack-01,10,Full,Front,"])).unwrap();
    placement::mounts_apply(db, 場.project.id, &元, Utc::now(), 1)
        .await
        .unwrap();

    let 先 = placement::parse_mounts(&搭載csv(&[",,web01,,Rack-01,20,Full,Front,"])).unwrap();
    placement::mounts_apply(db, 場.project.id, &先, Utc::now(), 2)
        .await
        .unwrap();

    assert_eq!(搭載の件数(db, d.id).await, 2, "履歴になっていません");
    let m = 現在の搭載(db, d.id).await.unwrap();
    assert_eq!(m.position, Some(20));
}

/// **`as_of` が履歴の開始日になること**（23.1）。
async fn as_ofが開始日になる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "asof").await;
    let d = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    let 基準 = Utc::now() - Duration::days(10);
    let rows = placement::parse_assignments(&所属csv(&[",,web01,,Warehouse,本社倉庫"])).unwrap();
    placement::assignments_apply(db, 場.project.id, &rows, 基準, 1)
        .await
        .unwrap();

    let 履歴 = 所属の履歴(db, d.id).await;
    assert_eq!(マイクロ秒(履歴[1].from_date), マイクロ秒(基準));
    // **前の行は同じ時刻で閉じる。**間に穴も重なりも作らない
    assert_eq!(履歴[0].to_date.map(マイクロ秒), Some(マイクロ秒(基準)));
}

/// 時刻をマイクロ秒に丸める。
///
/// **PostgreSQLの `timestamptz` はマイクロ秒までしか持たない。**`Utc::now()` の
/// ナノ秒は往復で落ちるため、そのまま突き合わせるとPostgreSQL側だけが落ちる
/// （実際にCIで落ちた。SQLiteは文字列で保持するため通る）。
///
/// **丸めるのは検証側であって、保存する値ではない。**片方のDBの精度に
/// 合わせて書き込む値を削ると、もう片方で持てるはずの情報を捨てることになる。
fn マイクロ秒(t: chrono::DateTime<Utc>) -> i64 {
    t.timestamp_micros()
}

/// **倉庫の什器には載せられないこと**（16.1のC領域）。
///
/// 搭載のCSVはプロジェクトに閉じており、倉庫内の配置は倉庫領域の担当である。
async fn 倉庫の什器には載せられない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "倉庫什器").await;
    let _ = 機器(db, 場.project.id, "web01", Some("SN-1")).await;

    mount_container::ActiveModel {
        name: Set("倉庫ラック".to_owned()),
        container_type: Set("Rack".to_owned()),
        location_type: Set("Warehouse".to_owned()),
        location_id: Set(場.warehouse.id),
        capacity: Set(Some(42)),
        created_by: Set(場.user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let rows = placement::parse_mounts(&搭載csv(&[",,web01,,倉庫ラック,1,Full,Full,"])).unwrap();
    let report = placement::mounts_dry_run(db, 場.project.id, &rows)
        .await
        .unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
}

// ---------------------------------------------------------------------------
// 用意
// ---------------------------------------------------------------------------

struct 舞台情報 {
    project: project::Model,
    warehouse: warehouse::Model,
    container: mount_container::Model,
    user_id: i32,
}

async fn 舞台(db: &DatabaseConnection, name: &str) -> 舞台情報 {
    let p = プロジェクト(db, name).await;
    let user_id = 利用者(db, &format!("{name}@example.com")).await;

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

    let c = mount_container::ActiveModel {
        name: Set("Rack-01".to_owned()),
        container_type: Set("Rack".to_owned()),
        location_type: Set("Project".to_owned()),
        location_id: Set(p.id),
        capacity: Set(Some(42)),
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
        warehouse: w,
        container: c,
        user_id,
    }
}

async fn 所属の履歴(db: &DatabaseConnection, device_id: i32) -> Vec<device_assignment::Model> {
    device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .order_by_asc(device_assignment::Column::FromDate)
        .order_by_asc(device_assignment::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 現在の搭載(db: &DatabaseConnection, device_id: i32) -> Option<device_mount::Model> {
    device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
}

async fn 搭載の件数(db: &DatabaseConnection, device_id: i32) -> u64 {
    device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .count(db)
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
    entity::app_user::ActiveModel {
        name: Set("配置".to_owned()),
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

/// 機器を作り、そのプロジェクトへの割当も作る。
async fn 機器(
    db: &DatabaseConnection,
    project_id: i32,
    hostname: &str,
    serial: Option<&str>,
) -> device::Model {
    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        external_id: Set(None),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(None),
        device_type: Set("Physical".to_owned()),
        device_category: Set(None),
        hostname: Set(hostname.to_owned()),
        serial_number: Set(serial.map(str::to_owned)),
        asset_number: Set(None),
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
        location_id: Set(Some(project_id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now() - Duration::days(30)),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    d
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 倉庫へ払い出せる);
        全検証!(@one $用意, $属性, 廃棄は参照先を持たない);
        全検証!(@one $用意, $属性, 他プロジェクトへは移せない);
        全検証!(@one $用意, $属性, 知らない倉庫はエラー);
        全検証!(@one $用意, $属性, 廃止した倉庫はエラー);
        全検証!(@one $用意, $属性, 倉庫名が無ければエラー);
        全検証!(@one $用意, $属性, 所属は二度流しても増えない);
        全検証!(@one $用意, $属性, 知らない機器はエラー);
        全検証!(@one $用意, $属性, 什器に載せられる);
        全検証!(@one $用意, $属性, 機器の上に載せられる);
        全検証!(@one $用意, $属性, 什器とホストは併記できない);
        全検証!(@one $用意, $属性, 自分の上には載せられない);
        全検証!(@one $用意, $属性, 重複配置は警告);
        全検証!(@one $用意, $属性, 左右に分かれていれば警告しない);
        全検証!(@one $用意, $属性, 搭載は二度流しても増えない);
        全検証!(@one $用意, $属性, 移設は閉じて開く);
        全検証!(@one $用意, $属性, as_ofが開始日になる);
        全検証!(@one $用意, $属性, 倉庫の什器には載せられない);
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
