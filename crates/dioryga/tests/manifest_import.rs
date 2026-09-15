//! マニフェスト単位の取込の結合テスト（設計書23.5、23.6）。
//!
//! # 何を確かめているか
//!
//! **同じマニフェストの中で、後のエンティティが前のエンティティを参照できること。**
//! 初期構築（22.2）では、機器・配置・部品・インタフェース・IPアドレスを
//! **1回の取込でまとめて入れる**のが普通の使い方である。
//!
//! `files` の順序を解釈せず依存順に処理する（23.5）のは、まさにこの参照を
//! 成り立たせるためである。**ドライランが「取込前のDB」だけを見ていると、
//! 同じファイルで作る機器を配置が見つけられず、エラーになる。**そして
//! エラーのあるドライランは反映できないため、**初回取込が1回では通らない。**
//!
//! **反映が途中で止まったとき、何も残らないこと。**エンティティごとに
//! 別々にコミットすると、機器だけ入って配置が入らない状態が残る。

mod support;

use std::path::PathBuf;

use chrono::Utc;
use dioryga::import::{run, Outcome};
use entity::{
    app_user, device, device_mount, import_run, mount_container, project, project_member,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};

/// **新しい機器とその搭載を1つのマニフェストで取り込めること。**
///
/// ドライランの時点では機器はまだDBに無い。それでも搭載の行が
/// 「機器が見つかりません」にならず、反映まで通ること。
async fn 同じ取込で作る機器を配置できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "同梱").await;
    let dir = 取込ファイル(
        &場,
        &[
            (
                "device",
                "devices.csv",
                "uid,external_id,hostname,serial_number,asset_number,device_type,power_watt,status\n\
                 ,,web01,SN-NEW-1,,Physical,400,running\n",
            ),
            (
                "device_mount",
                "mounts.csv",
                "uid,external_id,hostname,serial_number,container,position,horizontal_position,depth_position,host_hostname\n\
                 ,,web01,,Rack-01,10,Full,Front,\n",
            ),
        ],
    );

    let 下見 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        false,
    )
    .await
    .unwrap();
    assert!(
        !下見.report.has_error(),
        "ドライランでエラーになっています: {}\n{:?}",
        下見.report,
        下見.report.errors().collect::<Vec<_>>()
    );
    assert_eq!(下見.report.count(Outcome::Created), 2, "{}", 下見.report);

    // **ドライランは何も残さない**（23.6）
    assert_eq!(device::Entity::find().count(db).await.unwrap(), 0);
    assert_eq!(import_run::Entity::find().count(db).await.unwrap(), 0);

    let 実行 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        true,
    )
    .await
    .unwrap();
    assert!(実行.import_run_id.is_some());

    let d = device::Entity::find()
        .filter(device::Column::Hostname.eq("web01"))
        .one(db)
        .await
        .unwrap()
        .expect("機器が作られていません");
    let m = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(d.id))
        .one(db)
        .await
        .unwrap()
        .expect("搭載されていません");
    assert_eq!(m.position, Some(10));
}

/// **ドライランの件数と、反映した件数が一致すること**（23.6）。
///
/// 利用者はドライランの差分を見て実行を決める。**見せた差分と違うことが
/// 起きたら、2段階にした意味がない。**
async fn ドライランと反映の件数が一致する(db: &DatabaseConnection) {
    let 場 = 舞台(db, "件数一致").await;
    let dir = 取込ファイル(
        &場,
        &[
            (
                "device",
                "devices.csv",
                "uid,external_id,hostname,serial_number,asset_number,device_type,power_watt,status\n\
                 ,,web01,SN-A,,Physical,400,running\n\
                 ,,web02,SN-B,,Physical,400,running\n",
            ),
            (
                "device_mount",
                "mounts.csv",
                "uid,external_id,hostname,serial_number,container,position,horizontal_position,depth_position,host_hostname\n\
                 ,,web01,,Rack-01,10,Full,Front,\n\
                 ,,web02,,Rack-01,12,Full,Front,\n",
            ),
        ],
    );

    let 下見 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        false,
    )
    .await
    .unwrap();
    let 実行 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        true,
    )
    .await
    .unwrap();

    for o in [
        Outcome::Created,
        Outcome::Updated,
        Outcome::Unchanged,
        Outcome::Warning,
        Outcome::Error,
    ] {
        assert_eq!(
            下見.report.count(o),
            実行.report.count(o),
            "{} の件数が食い違っています。下見: {} / 実行: {}",
            o.as_str(),
            下見.report,
            実行.report
        );
    }
}

/// **1件でもエラーがあれば、何も書き込まないこと**（23.6）。
///
/// 機器の行は正しく、搭載の行だけが誤っている。**エンティティごとに
/// 別々にコミットすると、機器だけ入った状態が残る。**
async fn エラーがあれば何も残らない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "全か無か").await;
    let dir = 取込ファイル(
        &場,
        &[
            (
                "device",
                "devices.csv",
                "uid,external_id,hostname,serial_number,asset_number,device_type,power_watt,status\n\
                 ,,web01,SN-OK,,Physical,400,running\n",
            ),
            (
                "device_mount",
                "mounts.csv",
                "uid,external_id,hostname,serial_number,container,position,horizontal_position,depth_position,host_hostname\n\
                 ,,web01,,存在しないラック,10,Full,Front,\n",
            ),
        ],
    );

    let 下見 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        false,
    )
    .await
    .unwrap();
    assert_eq!(下見.report.count(Outcome::Error), 1, "{}", 下見.report);

    let 実行 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        true,
    )
    .await;
    assert!(実行.is_err(), "エラーがあるのに反映しています");

    assert_eq!(
        device::Entity::find().count(db).await.unwrap(),
        0,
        "機器だけが入っています"
    );
    assert_eq!(
        import_run::Entity::find().count(db).await.unwrap(),
        0,
        "取り込んでいないのに IMPORT_RUN が残っています"
    );
}

/// **ネットワークを同じマニフェストで取り込めること**（23.5、#102）。
///
/// 機器・サブネット・インタフェース・束ね・IPアドレスが**互いを参照する。**
/// 依存順（機器 → サブネット → インタフェース → 束ね → IP）に処理されて
/// いなければ、どこかが「参照先が無い」で落ちる。
async fn ネットワークをまとめて取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "網一式").await;
    // VLANはカタログ側の担当なので、ここでは先に用意しておく（23.5）
    let vlan_id = vlan(db, 場.project.id, 100, "web").await;

    let dir = 取込ファイル(
        &場,
        &[
            (
                "device",
                "devices.csv",
                "uid,external_id,hostname,serial_number,asset_number,device_type,power_watt,status
                 ,,web01,SN-NW-1,,Physical,400,running
",
            ),
            (
                "subnet",
                "subnets.csv",
                "cidr,vlan_tag,vlan_name,zone,description
10.0.1.0/24,100,web,DMZ,
",
            ),
            (
                "os_interface",
                "interfaces.csv",
                "hostname,os_interface_name,interface_type,aggregation_mode
                 web01,ens1f0,Physical,
                 web01,bond0,Bond,LACP
",
            ),
            (
                "interface_stack",
                "bonds.csv",
                "hostname,upper_interface,lower_interface
web01,bond0,ens1f0
",
            ),
            (
                "interface_vlan",
                "interface-vlans.csv",
                "hostname,os_interface_name,vlan_tag,vlan_name,tagging_mode
web01,bond0,100,web,Tagged
",
            ),
            (
                "ip_address",
                "ips.csv",
                "hostname,os_interface_name,ip_address,prefix_length,subnet_cidr
                 web01,bond0,10.0.1.10,24,10.0.1.0/24
",
            ),
        ],
    );

    let 下見 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        false,
    )
    .await
    .unwrap();
    assert!(
        !下見.report.has_error(),
        "ドライランでエラーになっています: {}\n{:?}",
        下見.report,
        下見.report.errors().collect::<Vec<_>>()
    );

    run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        true,
    )
    .await
    .unwrap();

    let ip = entity::ip_address::Entity::find()
        .filter(entity::ip_address::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .expect("IPアドレスが入っていません");
    assert_eq!(ip.ip_address, "10.0.1.10");
    assert!(ip.subnet_id.is_some(), "サブネットに紐づいていません");

    let iv = entity::interface_vlan::Entity::find()
        .one(db)
        .await
        .unwrap()
        .expect("VLANの結びが入っていません");
    assert_eq!(iv.vlan_id, vlan_id);

    assert_eq!(
        entity::interface_stack::Entity::find()
            .count(db)
            .await
            .unwrap(),
        1,
        "束ねが入っていません"
    );
}

/// **費用を同じマニフェストで取り込めること**（23.5、#103）。
///
/// 発注・固定資産・保守契約は、いずれも**同じファイルで作られる機器を指す。**
/// 費用が機器より先に処理されると「参照先が無い」で落ちる。
async fn 費用をまとめて取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "費用一式").await;
    ベンダー(db, "Fujitsu").await;

    let dir = 取込ファイル(
        &場,
        &[
            (
                "device",
                "devices.csv",
                "uid,external_id,hostname,serial_number,asset_number,device_type,power_watt,status\n\
                 ,,web01,SN-COST-1,,Physical,400,running\n",
            ),
            (
                "purchase_order",
                "orders.csv",
                "order_number,order_date,vendor,item_type,item_hostname,item_serial_number,quantity,unit_price\n\
                 PO-1,2026-04-01,Fujitsu,Device,web01,,1,1200000\n",
            ),
            (
                "fixed_asset",
                "assets.csv",
                "item_type,item_hostname,item_serial_number,acquisition_cost,depreciation_method,useful_life_years,acquisition_date\n\
                 Device,web01,,1200000,straight_line,5,2026-04-01\n",
            ),
            (
                "maintenance_contract",
                "contracts.csv",
                "contract_number,vendor,start_date,end_date,amount,quote_contact,failure_contact,item_type,item_hostname,item_serial_number\n\
                 CT-1,Fujitsu,2026-04-01,2027-03-31,240000,q@example.com,f@example.com,Device,web01,\n",
            ),
        ],
    );

    let 下見 = run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        false,
    )
    .await
    .unwrap();
    assert!(
        !下見.report.has_error(),
        "ドライランでエラーになっています: {}\n{:?}",
        下見.report,
        下見.report.errors().collect::<Vec<_>>()
    );

    run::run(
        db,
        &dir.join("manifest.yaml"),
        &場.email.replace('@', "_"),
        true,
    )
    .await
    .unwrap();

    let asset = entity::fixed_asset::Entity::find()
        .one(db)
        .await
        .unwrap()
        .expect("固定資産が入っていません");
    assert_eq!(asset.acquisition_cost, 1_200_000);

    let item = entity::purchase_order_item::Entity::find()
        .one(db)
        .await
        .unwrap()
        .expect("発注明細が入っていません");
    // **同じファイルで作られた機器を指せていること**
    assert_eq!(item.item_id, asset.item_id);

    assert_eq!(
        entity::maintenance_contract_item::Entity::find()
            .count(db)
            .await
            .unwrap(),
        1,
        "保守契約の品目が入っていません"
    );
}

// ---------------------------------------------------------------------------
// 用意
// ---------------------------------------------------------------------------

struct 舞台情報 {
    project: project::Model,
    email: String,
}

async fn 舞台(db: &DatabaseConnection, name: &str) -> 舞台情報 {
    let p = project::ActiveModel {
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
    .unwrap();

    let email = format!("{}@example.com", uuid::Uuid::new_v4());
    let u = app_user::ActiveModel {
        name: Set("取込者".to_owned()),
        username: Set((email.clone()).replace('@', "_")),
        email: Set(Some(email.clone())),
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

    project_member::ActiveModel {
        project_id: Set(p.id),
        user_id: Set(u.id),
        role: Set("Operator".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    mount_container::ActiveModel {
        name: Set("Rack-01".to_owned()),
        container_type: Set("Rack".to_owned()),
        location_type: Set("Project".to_owned()),
        location_id: Set(p.id),
        capacity: Set(Some(42)),
        created_by: Set(u.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    舞台情報 { project: p, email }
}

/// マニフェストとCSVを一時ディレクトリに書き出す。
///
/// `files` は `(entity, ファイル名, 中身)`。**依存順と逆に並べて書く**——
/// 取込側が順序を解釈しないこと（23.5）もあわせて確かめるため。
fn 取込ファイル(場: &舞台情報, files: &[(&str, &str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dioryga-manifest-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut entries = String::new();
    for (entity, name, body) in files.iter().rev() {
        std::fs::write(dir.join(name), body).unwrap();
        entries.push_str(&format!("  - {{ entity: {entity}, path: {name} }}\n"));
    }

    let manifest = format!(
        "format_version: 1\nkind: instances\nproject: \"{}\"\nfiles:\n{entries}",
        場.project.uid
    );
    std::fs::write(dir.join("manifest.yaml"), manifest).unwrap();
    dir
}

async fn vlan(db: &DatabaseConnection, _project_id: i32, tag: i32, name: &str) -> i32 {
    let u = app_user::Entity::find().one(db).await.unwrap().unwrap();
    entity::vlan::ActiveModel {
        vlan_tag: Set(tag),
        name: Set(name.to_owned()),
        zone: Set(None),
        description: Set(String::new()),
        retired_at: Set(None),
        created_by: Set(u.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn ベンダー(db: &DatabaseConnection, name: &str) -> i32 {
    let u = app_user::Entity::find().one(db).await.unwrap().unwrap();
    entity::vendor::ActiveModel {
        name: Set(name.to_owned()),
        created_by: Set(u.id),
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
        全検証!(@one $用意, $属性, 同じ取込で作る機器を配置できる);
        全検証!(@one $用意, $属性, ドライランと反映の件数が一致する);
        全検証!(@one $用意, $属性, エラーがあれば何も残らない);
        全検証!(@one $用意, $属性, ネットワークをまとめて取り込める);
        全検証!(@one $用意, $属性, 費用をまとめて取り込める);
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
