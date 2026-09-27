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
                "purchase",
                "purchases.csv",
                "item_type,item_hostname,item_serial_number,order_number,acquired_on,amount,supplier\n\
                 Device,web01,,PO-1,2026-04-01,1200000,〇〇商事\n",
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

    let item = entity::purchase::Entity::find()
        .one(db)
        .await
        .unwrap()
        .expect("購入の記録が入っていません");
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
// 什器（#139）
// ---------------------------------------------------------------------------

const 什器見出し: &str = "name,container_vendor,container_model\n";
/// 取込の什器が指す型番のベンダー（#205）。
const 什器ベンダー: &str = "什器メーカー";

/// 型番を作る。ベンダーは [`什器ベンダー`]（無ければ作る）。
async fn 型番(
    db: &DatabaseConnection,
    model_name: &str,
    container_type: &str,
    capacity: Option<i32>,
) -> i32 {
    let u = app_user::Entity::find().one(db).await.unwrap().unwrap();
    let v = match entity::vendor::Entity::find()
        .filter(entity::vendor::Column::Name.eq(什器ベンダー))
        .one(db)
        .await
        .unwrap()
    {
        Some(v) => v,
        None => entity::vendor::ActiveModel {
            name: Set(什器ベンダー.to_owned()),
            created_by: Set(u.id),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap(),
    };
    entity::container_model::ActiveModel {
        vendor_id: Set(v.id),
        model_name: Set(model_name.to_owned()),
        container_type: Set(container_type.to_owned()),
        height_u: Set((container_type == "Rack").then_some(capacity).flatten()),
        shelf_count: Set((container_type == "Shelving").then_some(capacity).flatten()),
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
const 機器見出し: &str =
    "uid,external_id,hostname,serial_number,asset_number,device_type,power_watt,status\n";
const 搭載見出し: &str =
    "uid,external_id,hostname,serial_number,container,position,horizontal_position,depth_position,host_hostname\n";

/// **什器と、そこへの搭載を1つのマニフェストで取り込めること**（#139）。
///
/// 什器を画面でしか作れないと、取込だけではラック図が空になる。**同じ取込で
/// 作る什器を搭載が名前で指せる**ことが要点で、ドライランの時点で通らなければ
/// 反映もできない（23.6）。
async fn 什器と搭載を同じ取込で入れられる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "什器同梱").await;
    let 取込者 = 場.email.replace('@', "_");
    let r42 = 型番(db, "R42", "Rack", Some(42)).await;
    let dir = 取込ファイル(
        &場,
        &[
            (
                "mount_container",
                "containers.csv",
                &format!("{什器見出し}Rack-02,{什器ベンダー},R42\n"),
            ),
            (
                "device",
                "devices.csv",
                &format!("{機器見出し},,web01,SN-C-1,,Physical,400,running\n"),
            ),
            (
                "device_mount",
                "mounts.csv",
                &format!("{搭載見出し},,web01,,Rack-02,10,Full,Front,\n"),
            ),
        ],
    );

    let 下見 = run::run(db, &dir.join("manifest.yaml"), &取込者, false)
        .await
        .unwrap();
    assert!(!下見.report.has_error(), "{}", 下見.report);
    assert_eq!(下見.report.count(Outcome::Created), 3, "{}", 下見.report);
    // **ドライランは何も残さない**（23.6）
    assert_eq!(什器の数(db, 場.project.id).await, 1);

    run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();

    let c = 什器(db, 場.project.id, "Rack-02").await;
    assert_eq!(c.container_model_id, Some(r42), "型番を指していません");
    let u = app_user::Entity::find()
        .filter(app_user::Column::Username.eq(&取込者))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c.created_by, u.id, "登録者が取込者になっていません");

    let d = device::Entity::find()
        .filter(device::Column::Hostname.eq("web01"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let m = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(d.id))
        .one(db)
        .await
        .unwrap()
        .expect("搭載されていません");
    assert_eq!(
        m.container_id,
        Some(c.id),
        "同じ取込で作った什器に載っていません"
    );
}

/// **2回流しても結果が変わらないこと**（23.1）。什器が増えない。
async fn 什器を二度流しても変わらない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "什器二度").await;
    let 取込者 = 場.email.replace('@', "_");
    型番(db, "R42", "Rack", Some(42)).await;
    型番(db, "D1", "Desk", None).await;
    型番(db, "S5", "Shelving", Some(5)).await;
    let dir = 取込ファイル(
        &場,
        &[(
            "mount_container",
            "containers.csv",
            &format!(
                "{什器見出し}Rack-02,{什器ベンダー},R42\nDesk-01,{什器ベンダー},D1\nShelf-01,{什器ベンダー},S5\n"
            ),
        )],
    );

    run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();
    let 二度目 = run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();

    assert_eq!(
        二度目.report.count(Outcome::Created),
        0,
        "{}",
        二度目.report
    );
    assert_eq!(
        二度目.report.count(Outcome::Updated),
        0,
        "{}",
        二度目.report
    );
    // 舞台の Rack-01 ＋ 3件。**ファイルに無い Rack-01 には触らない**
    assert_eq!(什器の数(db, 場.project.id).await, 4);
}

/// **値が違えば、同じ行を更新すること。**什器は履歴を持たない。
async fn 什器の値を変えると同じ行が更新される(db: &DatabaseConnection) {
    let 場 = 舞台(db, "什器更新").await;
    let 取込者 = 場.email.replace('@', "_");
    let 前 = 什器(db, 場.project.id, "Rack-01").await;
    let r48 = 型番(db, "R48", "Rack", Some(48)).await;

    let dir = 取込ファイル(
        &場,
        &[(
            "mount_container",
            "containers.csv",
            &format!("{什器見出し}Rack-01,{什器ベンダー},R48\n"),
        )],
    );
    let 実行 = run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();

    assert_eq!(実行.report.count(Outcome::Updated), 1, "{}", 実行.report);
    let 後 = 什器(db, 場.project.id, "Rack-01").await;
    assert_eq!(後.id, 前.id, "行が作り直されています");
    assert_eq!(後.container_model_id, Some(r48));
    assert_eq!(什器の数(db, 場.project.id).await, 1);
}

/// **見つからない型番・廃番・空の名前・ファイル内の重複を拒否すること**（Q-21、18.5）。
async fn 什器の誤りは取り込まない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "什器誤り").await;
    let 取込者 = 場.email.replace('@', "_");
    型番(db, "D1", "Desk", None).await;
    let 廃番 = 型番(db, "OLD", "Rack", Some(42)).await;
    let m = entity::container_model::Entity::find_by_id(廃番)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut active: entity::container_model::ActiveModel = m.into();
    active.retired_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
    let dir = 取込ファイル(
        &場,
        &[(
            "mount_container",
            "containers.csv",
            &format!(
                "{什器見出し}Cab-01,無いメーカー,X\nRack-09,{什器ベンダー},無い型番\n,{什器ベンダー},D1\nRack-10,,\nRack-11,{什器ベンダー},OLD\nDesk-01,{什器ベンダー},D1\nDesk-01,{什器ベンダー},D1\n"
            ),
        )],
    );

    let 下見 = run::run(db, &dir.join("manifest.yaml"), &取込者, false)
        .await
        .unwrap();
    // ベンダーが無い・型番が無い・空の名前・型番の列が空・廃番・2つ目の Desk-01
    assert_eq!(下見.report.count(Outcome::Error), 6, "{}", 下見.report);
    assert!(run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .is_err());
    assert_eq!(
        什器の数(db, 場.project.id).await,
        1,
        "誤りがあるのに書かれています"
    );
}

/// **名前は大文字小文字を区別せずに突き合わせること**（12.10、#204）。
/// 「rack-01」は既存の「Rack-01」を指し、2つ目を作らない。
async fn 什器の名前は大文字小文字を区別しない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "什器大小").await;
    let 取込者 = 場.email.replace('@', "_");
    型番(db, "R48", "Rack", Some(48)).await;
    let dir = 取込ファイル(
        &場,
        &[(
            "mount_container",
            "containers.csv",
            &format!("{什器見出し}rack-01,{什器ベンダー},R48\n"),
        )],
    );
    let 実行 = run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();

    assert_eq!(実行.report.count(Outcome::Updated), 1, "{}", 実行.report);
    assert_eq!(什器の数(db, 場.project.id).await, 1);
}

/// **撤去した設備は突き合わせの対象にしないこと。**同じ名前の行は新しい
/// 設備になり、撤去した設備だけが持つ名前へは載せられない。
async fn 撤去した什器は突き合わせない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "什器撤去").await;
    let 取込者 = 場.email.replace('@', "_");
    型番(db, "R42", "Rack", Some(42)).await;
    let 旧 = 什器(db, 場.project.id, "Rack-01").await;
    let mut active: mount_container::ActiveModel = 旧.clone().into();
    active.retired_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    let dir = 取込ファイル(
        &場,
        &[
            (
                "mount_container",
                "containers.csv",
                &format!("{什器見出し}Rack-01,{什器ベンダー},R42\n"),
            ),
            (
                "device",
                "devices.csv",
                &format!("{機器見出し},,web01,SN-R-1,,Physical,400,running\n"),
            ),
        ],
    );
    let 実行 = run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();
    assert_eq!(実行.report.count(Outcome::Created), 2, "{}", 実行.report);
    assert_eq!(
        什器の数(db, 場.project.id).await,
        2,
        "撤去した設備を書き換えています"
    );

    // 撤去した設備にしか無い名前へは載せられない
    let 古い = mount_container::ActiveModel {
        name: Set("Old-01".to_owned()),
        container_model_id: Set(Some(
            crate::support::設備の型番(db, 旧.created_by, "Rack".to_owned(), Some(42)).await,
        )),
        location_type: Set("Project".to_owned()),
        location_id: Set(場.project.id),
        retired_at: Set(Some(Utc::now())),
        created_by: Set(旧.created_by),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    };
    古い.insert(db).await.unwrap();
    let dir = 取込ファイル(
        &場,
        &[(
            "device_mount",
            "mounts.csv",
            &format!("{搭載見出し},,web01,,Old-01,10,Full,Front,\n"),
        )],
    );
    let 下見 = run::run(db, &dir.join("manifest.yaml"), &取込者, false)
        .await
        .unwrap();
    assert_eq!(下見.report.count(Outcome::Error), 1, "{}", 下見.report);
}

/// **回路を取り込めること**（#206）。同じ取込で作る設備・什器を名前で指せ、
/// 二度流しても変わらず、値を変えると閉じて開く。
async fn 回路を取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "回路取込").await;
    let 取込者 = 場.email.replace('@', "_");
    型番(db, "R42", "Rack", Some(42)).await;
    let 回路見出し = "container,circuit_label,voltage,phase,breaker_current_a,connector_type\n";
    let files = |a系: &str| {
        vec![
            (
                "mount_container",
                "containers.csv".to_owned(),
                format!("{什器見出し}Rack-02,{什器ベンダー},R42\n"),
            ),
            (
                "power_circuit",
                "circuits.csv".to_owned(),
                format!(
                    "{回路見出し}rack-02,A系,200,Single,{a系},NEMA L6-30R\nRack-02,B系,200,Single,20,NEMA L6-30R\n"
                ),
            ),
        ]
    };
    let 流す = |a系: &'static str| {
        let files = files(a系);
        let borrowed: Vec<(&str, &str, &str)> = files
            .iter()
            .map(|(e, n, b)| (*e, n.as_str(), b.as_str()))
            .collect();
        取込ファイル(&場, &borrowed)
    };

    let 実行 = run::run(db, &流す("20").join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();
    assert_eq!(実行.report.count(Outcome::Created), 3, "{}", 実行.report);

    let 二度目 = run::run(db, &流す("20").join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();
    assert_eq!(
        二度目.report.count(Outcome::Created),
        0,
        "{}",
        二度目.report
    );
    assert_eq!(
        二度目.report.count(Outcome::Updated),
        0,
        "{}",
        二度目.report
    );

    let 変更 = run::run(db, &流す("30").join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();
    assert_eq!(変更.report.count(Outcome::Updated), 1, "{}", 変更.report);
    let 回路 = entity::power_circuit::Entity::find().all(db).await.unwrap();
    assert_eq!(回路.len(), 3, "閉じて開いていません");
    assert_eq!(回路.iter().filter(|r| r.to_date.is_none()).count(), 2);
    assert!(回路
        .iter()
        .any(|r| r.to_date.is_none() && r.breaker_current_ma == 30_000));
}

/// **回路の誤りを拒否すること。**画面と同じ規則を通す。
async fn 回路の誤りは取り込まない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "回路誤り").await;
    let 取込者 = 場.email.replace('@', "_");
    let dir = 取込ファイル(
        &場,
        &[(
            "power_circuit",
            "circuits.csv",
            "container,circuit_label,voltage,phase,breaker_current_a,connector_type\n\
             無いラック,A系,200,Single,20,\n\
             Rack-01,A系,200,Triple,20,\n\
             Rack-01,B系,abc,Single,20,\n\
             Rack-01,C系,200,Single,0,\n\
             Rack-01,D系,200,Single,20,\n\
             Rack-01,D系,100,Single,15,\n",
        )],
    );
    let 下見 = run::run(db, &dir.join("manifest.yaml"), &取込者, false)
        .await
        .unwrap();
    // 無いラック・語彙外の相・読めない電圧・0A・ファイル内の重複
    assert_eq!(下見.report.count(Outcome::Error), 5, "{}", 下見.report);
}

/// **設置場所を取り込めること**（#206）。値を変えると同じ行を更新する。
async fn 設置場所を取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "設置場所").await;
    let 取込者 = 場.email.replace('@', "_");
    型番(db, "R42", "Rack", Some(42)).await;
    for (site, 期待) in [
        ("第1DC 3F", Outcome::Created),
        ("第1DC 4F", Outcome::Updated),
    ] {
        let dir = 取込ファイル(
            &場,
            &[(
                "mount_container",
                "containers.csv",
                &format!(
                    "name,container_vendor,container_model,installation_site\nRack-02,{什器ベンダー},R42,{site}\n"
                ),
            )],
        );
        let 実行 = run::run(db, &dir.join("manifest.yaml"), &取込者, true)
            .await
            .unwrap();
        assert_eq!(実行.report.count(期待), 1, "{}", 実行.report);
    }
    let c = 什器(db, 場.project.id, "Rack-02").await;
    assert_eq!(c.installation_site.as_deref(), Some("第1DC 4F"));
}

/// **購入と固定資産の取込で、設備・什器を名前で指せること**（#206）。
/// 保守契約は設備・什器を指せない。
async fn 設備の費用を取り込める(db: &DatabaseConnection) {
    let 場 = 舞台(db, "設備費用").await;
    let 取込者 = 場.email.replace('@', "_");
    ベンダー(db, "保守会社").await;
    let dir = 取込ファイル(
        &場,
        &[
            (
                "purchase",
                "purchases.csv",
                "item_type,item_hostname,item_serial_number,item_container,order_number,acquired_on,amount,supplier\n\
                 MountContainer,,,rack-01,PO-R-1,2026-05-01,300000,リンドウ商事\n",
            ),
            (
                "fixed_asset",
                "assets.csv",
                "item_type,item_hostname,item_serial_number,item_container,acquisition_cost,depreciation_method,useful_life_years,acquisition_date\n\
                 MountContainer,,,Rack-01,300000,straight_line,5,2026-05-01\n",
            ),
        ],
    );
    let 実行 = run::run(db, &dir.join("manifest.yaml"), &取込者, true)
        .await
        .unwrap();
    assert!(!実行.report.has_error(), "{}", 実行.report);
    let rack = 什器(db, 場.project.id, "Rack-01").await;
    let p = entity::purchase::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (p.item_type.as_str(), p.item_id),
        ("MountContainer", rack.id)
    );
    let a = entity::fixed_asset::Entity::find()
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (a.item_type.as_str(), a.item_id),
        ("MountContainer", rack.id)
    );

    // 保守契約は指せない
    let dir = 取込ファイル(
        &場,
        &[(
            "maintenance_contract",
            "contracts.csv",
            "contract_number,vendor,start_date,end_date,amount,quote_contact,failure_contact,item_type,item_hostname,item_serial_number\n\
             MC-R-1,保守会社,2026-04-01,2027-03-31,100000,,,MountContainer,,\n",
        )],
    );
    let 下見 = run::run(db, &dir.join("manifest.yaml"), &取込者, false)
        .await
        .unwrap();
    assert_eq!(下見.report.count(Outcome::Error), 1, "{}", 下見.report);
}

async fn 什器の数(db: &DatabaseConnection, project_id: i32) -> u64 {
    mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq("Project"))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .count(db)
        .await
        .unwrap()
}

async fn 什器(db: &DatabaseConnection, project_id: i32, name: &str) -> mount_container::Model {
    mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq("Project"))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .filter(mount_container::Column::Name.eq(name))
        .one(db)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("什器「{name}」がありません"))
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
        container_model_id: Set(Some(
            crate::support::設備の型番(db, u.id, "Rack".to_owned(), Some(42)).await,
        )),
        location_type: Set("Project".to_owned()),
        location_id: Set(p.id),
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
        全検証!(@one $用意, $属性, 什器と搭載を同じ取込で入れられる);
        全検証!(@one $用意, $属性, 什器を二度流しても変わらない);
        全検証!(@one $用意, $属性, 什器の値を変えると同じ行が更新される);
        全検証!(@one $用意, $属性, 什器の誤りは取り込まない);
        全検証!(@one $用意, $属性, 什器の名前は大文字小文字を区別しない);
        全検証!(@one $用意, $属性, 撤去した什器は突き合わせない);
        全検証!(@one $用意, $属性, 回路を取り込める);
        全検証!(@one $用意, $属性, 回路の誤りは取り込まない);
        全検証!(@one $用意, $属性, 設置場所を取り込める);
        全検証!(@one $用意, $属性, 設備の費用を取り込める);
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
