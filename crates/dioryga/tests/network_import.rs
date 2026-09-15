//! ネットワーク取込の結合テスト（設計書23.5、8.3、8.5、14.2）。
//!
//! # 何を確かめているか
//!
//! **インタフェースの種別が変わったら、乗っているものも閉じること**（8.5）。
//! 閉じた行を指したままの束ね・VLAN・IPアドレスが残ると、画面の表示も
//! `dioryga check` の判定も信じられなくなる。
//!
//! **束ねる関係に循環を作らせないこと**（8.5）。辿る処理が終わらなくなる。
//!
//! **同一サブネット内のIP重複を、DBのエラーにする前に止めること**（14.2）。
//! 部分一意インデックスに当たると、利用者には直し方の分からない失敗になる。
//!
//! **二度流しても履歴が増えないこと**（23.1）。

mod support;

use chrono::{Duration, Utc};
use dioryga::import::{network, Outcome};
use dioryga::repository::{Actor, AuditedTx};
use entity::{
    app_user, device, device_assignment, interface_stack, interface_vlan, ip_address, os_interface,
    project, subnet, vlan,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

// ---------------------------------------------------------------------------
// サブネット（14.2）
// ---------------------------------------------------------------------------

/// **サブネットを登録でき、VLANに紐づくこと。**二度流しても増えない。
async fn サブネットを登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "サブネット").await;
    let rows = network::parse_subnets(
        "cidr,vlan_tag,vlan_name,zone,description\n10.0.1.0/24,100,web,DMZ,公開系\n",
    )
    .unwrap();

    let tx = 取込(db).await;
    let r = network::サブネットを取り込む(&tx, 場.project.id, &rows, 場.user_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Created), 1, "{r}");

    let s = subnet::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(s.cidr, "10.0.1.0/24");
    assert_eq!(s.vlan_id, Some(場.vlan_id));
    assert_eq!(s.zone.as_deref(), Some("DMZ"));

    // 2回目は何も書かない（23.1）
    let tx = 取込(db).await;
    let r = network::サブネットを取り込む(&tx, 場.project.id, &rows, 場.user_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Unchanged), 1, "{r}");
    assert_eq!(subnet::Entity::find().count(db).await.unwrap(), 1);
}

/// **cidrの形が違えばエラー。**
async fn 不正なcidrはエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "不正cidr").await;
    let rows =
        network::parse_subnets("cidr,vlan_tag,vlan_name,zone,description\n10.0.1.0,,,,\n").unwrap();

    let tx = 取込(db).await;
    let r = network::サブネットを取り込む(&tx, 場.project.id, &rows, 場.user_id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **ゾーンの語彙外は拒否すること**（8.6）。
async fn 語彙外のゾーンは拒否(db: &DatabaseConnection) {
    let 場 = 舞台(db, "ゾーン語彙").await;
    let rows = network::parse_subnets(
        "cidr,vlan_tag,vlan_name,zone,description\n10.0.1.0/24,,,まんなか,\n",
    )
    .unwrap();

    let tx = 取込(db).await;
    let r = network::サブネットを取り込む(&tx, 場.project.id, &rows, 場.user_id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// インタフェース（8.3）
// ---------------------------------------------------------------------------

/// **物理インタフェースとbondを登録できること。**
async fn インタフェースを登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "IF登録").await;
    let tx = 取込(db).await;
    let r = network::インタフェースを取り込む(
        &tx,
        場.project.id,
        &インタフェース(),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(r.count(Outcome::Created), 3, "{r}");
    let bond = 現在のif(db, 場.device.id, "bond0").await.unwrap();
    assert_eq!(bond.interface_type, "Bond");
    assert_eq!(bond.aggregation_mode.as_deref(), Some("LACP"));
    // **物理ポートとの紐付けは取込では扱わない**（23.5）
    assert!(bond.part_instance_id.is_none());
    assert!(bond.port_slot_id.is_none());
}

/// **Bond以外に集約モードを書いたらエラー**（8.3）。種別かモードが誤っている。
async fn bond以外の集約モードは拒否(db: &DatabaseConnection) {
    let 場 = 舞台(db, "集約モード").await;
    let rows = network::parse_interfaces(
        "hostname,os_interface_name,interface_type,aggregation_mode\nweb01,ens1f0,Physical,LACP\n",
    )
    .unwrap();

    let tx = 取込(db).await;
    let r = network::インタフェースを取り込む(&tx, 場.project.id, &rows, Utc::now())
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **種別が変わったら閉じて開き、乗っているものも閉じること**（8.5）。
///
/// 閉じた件数を警告に出す。書き直さなければ消えるためで、黙って消さない。
async fn 種別が変わると乗っているものも閉じる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "種別変更").await;
    let as_of = Utc::now() - Duration::days(1);

    let tx = 取込(db).await;
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    network::ipアドレスを取り込む(
        &tx,
        場.project.id,
        &network::parse_ips(
            "hostname,os_interface_name,ip_address,prefix_length,subnet_cidr\nweb01,ens1f0,10.0.1.10,24,\n",
        )
        .unwrap(),
        as_of,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let 元 = 現在のif(db, 場.device.id, "ens1f0").await.unwrap();

    // ens1f0 を Physical から Bridge へ
    let 変更 = network::parse_interfaces(
        "hostname,os_interface_name,interface_type,aggregation_mode\nweb01,ens1f0,Bridge,\n",
    )
    .unwrap();
    let tx = 取込(db).await;
    let r = network::インタフェースを取り込む(&tx, 場.project.id, &変更, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        r.count(Outcome::Warning),
        1,
        "閉じた件数を警告に出していません: {r}"
    );

    let 後 = 現在のif(db, 場.device.id, "ens1f0").await.unwrap();
    assert_ne!(後.id, 元.id, "閉じて開いていません");
    assert_eq!(後.interface_type, "Bridge");

    let 旧 = os_interface::Entity::find_by_id(元.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(旧.to_date.is_some(), "前の行を閉じていません");

    // 乗っていたIPも閉じている
    let 生きているip = ip_address::Entity::find()
        .filter(ip_address::Column::ToDate.is_null())
        .count(db)
        .await
        .unwrap();
    assert_eq!(生きているip, 0, "閉じた行を指したままのIPが残っています");
}

// ---------------------------------------------------------------------------
// 束ね（8.5）
// ---------------------------------------------------------------------------

/// **束ねを登録でき、二度流しても増えないこと。**
async fn 束ねを登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "束ね").await;
    let as_of = Utc::now();
    let rows = network::parse_stacks(
        "hostname,upper_interface,lower_interface\nweb01,bond0,ens1f0\nweb01,bond0.100,bond0\n",
    )
    .unwrap();

    let tx = 取込(db).await;
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    let r = network::束ねを取り込む(&tx, 場.project.id, &rows, as_of)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Created), 2, "{r}");

    let tx = 取込(db).await;
    let r = network::束ねを取り込む(&tx, 場.project.id, &rows, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Unchanged), 2, "{r}");
    assert_eq!(interface_stack::Entity::find().count(db).await.unwrap(), 2);
}

/// **循環を作らせないこと**（8.5）。辿る処理が終わらなくなる。
async fn 循環はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "循環").await;
    let as_of = Utc::now();

    let tx = 取込(db).await;
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    network::束ねを取り込む(
        &tx,
        場.project.id,
        &network::parse_stacks("hostname,upper_interface,lower_interface\nweb01,bond0,ens1f0\n")
            .unwrap(),
        as_of,
    )
    .await
    .unwrap();
    // ens1f0 の下に bond0 を入れると循環する
    let r = network::束ねを取り込む(
        &tx,
        場.project.id,
        &network::parse_stacks("hostname,upper_interface,lower_interface\nweb01,ens1f0,bond0\n")
            .unwrap(),
        as_of,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **自分自身は束ねられないこと。**
async fn 自分自身は束ねられない(db: &DatabaseConnection) {
    let 場 = 舞台(db, "自己束ね").await;
    let as_of = Utc::now();

    let tx = 取込(db).await;
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    let r = network::束ねを取り込む(
        &tx,
        場.project.id,
        &network::parse_stacks("hostname,upper_interface,lower_interface\nweb01,bond0,bond0\n")
            .unwrap(),
        as_of,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **存在しないインタフェースを指したらエラー。**
async fn 知らないインタフェースはエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "未知IF").await;
    let tx = 取込(db).await;
    let r = network::束ねを取り込む(
        &tx,
        場.project.id,
        &network::parse_stacks("hostname,upper_interface,lower_interface\nweb01,bond9,ens9\n")
            .unwrap(),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// インタフェースのVLAN（8.5）
// ---------------------------------------------------------------------------

/// **VLANを結び、タグ付けの変更は閉じて開くこと**（4章）。
async fn vlanの結びは閉じて開く(db: &DatabaseConnection) {
    let 場 = 舞台(db, "IFVLAN").await;
    let as_of = Utc::now() - Duration::days(1);

    let tx = 取込(db).await;
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    let r = network::インタフェースのvlanを取り込む(
        &tx,
        場.project.id,
        &network::parse_interface_vlans(
            "hostname,os_interface_name,vlan_tag,vlan_name,tagging_mode\nweb01,bond0,100,web,Tagged\n",
        )
        .unwrap(),
        as_of,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Created), 1, "{r}");

    let tx = 取込(db).await;
    let r = network::インタフェースのvlanを取り込む(
        &tx,
        場.project.id,
        &network::parse_interface_vlans(
            "hostname,os_interface_name,vlan_tag,vlan_name,tagging_mode\nweb01,bond0,100,web,Untagged\n",
        )
        .unwrap(),
        Utc::now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Updated), 1, "{r}");

    let 全部 = interface_vlan::Entity::find()
        .order_by_asc(interface_vlan::Column::Id)
        .all(db)
        .await
        .unwrap();
    assert_eq!(全部.len(), 2, "閉じて開いていません");
    assert!(全部[0].to_date.is_some());
    assert_eq!(全部[1].tagging_mode, "Untagged");
}

/// **同じタグのVLANが複数あるとき、名前が無ければエラー**（8.5）。
///
/// 黙って1つ目を選ぶと、別の拠点のVLANに繋ぎ込む。
async fn 同じタグが複数ならエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "タグ重複").await;
    vlan作る(db, 場.user_id, 100, "別拠点web").await;
    let as_of = Utc::now();

    let tx = 取込(db).await;
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    let r = network::インタフェースのvlanを取り込む(
        &tx,
        場.project.id,
        &network::parse_interface_vlans(
            "hostname,os_interface_name,vlan_tag,vlan_name,tagging_mode\nweb01,bond0,100,,Tagged\n",
        )
        .unwrap(),
        as_of,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

// ---------------------------------------------------------------------------
// IPアドレス（14.2）
// ---------------------------------------------------------------------------

/// **IPを登録でき、二度流しても履歴が増えないこと。**
async fn ipを登録できる(db: &DatabaseConnection) {
    let 場 = 舞台(db, "IP登録").await;
    let as_of = Utc::now() - Duration::days(1);
    let rows = network::parse_ips(
        "hostname,os_interface_name,ip_address,prefix_length,subnet_cidr\nweb01,bond0,10.0.1.10,24,10.0.1.0/24\n",
    )
    .unwrap();

    let tx = 取込(db).await;
    network::サブネットを取り込む(&tx, 場.project.id, &サブネット(), 場.user_id)
        .await
        .unwrap();
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    let r = network::ipアドレスを取り込む(&tx, 場.project.id, &rows, as_of)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Created), 1, "{r}");

    let tx = 取込(db).await;
    let r = network::ipアドレスを取り込む(&tx, 場.project.id, &rows, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(r.count(Outcome::Unchanged), 1, "{r}");
    assert_eq!(ip_address::Entity::find().count(db).await.unwrap(), 1);
}

/// **同一サブネット内でIPが重複したらエラー**（14.2）。
///
/// DBの部分一意インデックスに当たる前に、直せる言葉で止める。
async fn 同一サブネットのip重複はエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "IP重複").await;
    let as_of = Utc::now();
    let rows = network::parse_ips(
        "hostname,os_interface_name,ip_address,prefix_length,subnet_cidr\n\
         web01,bond0,10.0.1.10,24,10.0.1.0/24\n\
         web01,ens1f0,10.0.1.10,24,10.0.1.0/24\n",
    )
    .unwrap();

    let tx = 取込(db).await;
    network::サブネットを取り込む(&tx, 場.project.id, &サブネット(), 場.user_id)
        .await
        .unwrap();
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    let r = network::ipアドレスを取り込む(&tx, 場.project.id, &rows, as_of)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(r.count(Outcome::Error), 1, "{r}");
}

/// **知らないサブネットを指したらエラー。**
async fn 知らないサブネットはエラー(db: &DatabaseConnection) {
    let 場 = 舞台(db, "未知サブネット").await;
    let as_of = Utc::now();

    let tx = 取込(db).await;
    network::インタフェースを取り込む(&tx, 場.project.id, &インタフェース(), as_of)
        .await
        .unwrap();
    let r = network::ipアドレスを取り込む(
        &tx,
        場.project.id,
        &network::parse_ips(
            "hostname,os_interface_name,ip_address,prefix_length,subnet_cidr\nweb01,bond0,10.9.9.9,24,10.9.9.0/24\n",
        )
        .unwrap(),
        as_of,
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
    user_id: i32,
    vlan_id: i32,
}

fn インタフェース() -> Vec<network::InterfaceRow> {
    network::parse_interfaces(
        "hostname,os_interface_name,interface_type,aggregation_mode\n\
         web01,ens1f0,Physical,\n\
         web01,bond0,Bond,LACP\n\
         web01,bond0.100,Vlan,\n",
    )
    .unwrap()
}

fn サブネット() -> Vec<network::SubnetRow> {
    network::parse_subnets("cidr,vlan_tag,vlan_name,zone,description\n10.0.1.0/24,,,LAN,\n")
        .unwrap()
}

async fn 取込(db: &DatabaseConnection) -> AuditedTx {
    AuditedTx::begin(db, Actor::Import { import_run_id: 1 })
        .await
        .unwrap()
}

async fn 現在のif(
    db: &DatabaseConnection,
    device_id: i32,
    name: &str,
) -> Option<os_interface::Model> {
    os_interface::Entity::find()
        .filter(os_interface::Column::DeviceId.eq(device_id))
        .filter(os_interface::Column::OsInterfaceName.eq(name))
        .filter(os_interface::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
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

    let u = app_user::ActiveModel {
        name: Set("網".to_owned()),
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

    let vlan_id = vlan作る(db, u.id, 100, "web").await;

    舞台情報 {
        project: p,
        device: d,
        user_id: u.id,
        vlan_id,
    }
}

async fn vlan作る(db: &DatabaseConnection, user_id: i32, tag: i32, name: &str) -> i32 {
    vlan::ActiveModel {
        vlan_tag: Set(tag),
        name: Set(name.to_owned()),
        zone: Set(None),
        description: Set(String::new()),
        retired_at: Set(None),
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, サブネットを登録できる);
        全検証!(@one $用意, $属性, 不正なcidrはエラー);
        全検証!(@one $用意, $属性, 語彙外のゾーンは拒否);
        全検証!(@one $用意, $属性, インタフェースを登録できる);
        全検証!(@one $用意, $属性, bond以外の集約モードは拒否);
        全検証!(@one $用意, $属性, 種別が変わると乗っているものも閉じる);
        全検証!(@one $用意, $属性, 束ねを登録できる);
        全検証!(@one $用意, $属性, 循環はエラー);
        全検証!(@one $用意, $属性, 自分自身は束ねられない);
        全検証!(@one $用意, $属性, 知らないインタフェースはエラー);
        全検証!(@one $用意, $属性, vlanの結びは閉じて開く);
        全検証!(@one $用意, $属性, 同じタグが複数ならエラー);
        全検証!(@one $用意, $属性, ipを登録できる);
        全検証!(@one $用意, $属性, 同一サブネットのip重複はエラー);
        全検証!(@one $用意, $属性, 知らないサブネットはエラー);
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
