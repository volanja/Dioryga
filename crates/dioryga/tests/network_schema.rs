//! ネットワークのスキーマの結合テスト（設計書8章、14章）。
//!
//! 画面はまだ無いため、**マイグレーションが両DBで通ること**と、**8章の再設計で
//! 表現できるようにしたはずの構成が実際に入ること**を確かめる。特にボンドと
//! VLANサブインターフェースは、`INTERFACE_STACK` を中間テーブルにした理由その
//! ものなので、ここで固定しておく。

mod support;

use chrono::Utc;
use entity::{
    app_user, device, interface_stack, interface_vlan, ip_address, os_interface, project, subnet,
    vlan,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

// ---------------------------------------------------------------------------
// OS_INTERFACE（8.3）
// ---------------------------------------------------------------------------

/// **物理ポートを持たないインターフェースが登録できること**（設計書8.3）。
///
/// ボンド・VLANサブインターフェース・SVIは物理ポートに対応しない。ここが
/// 通らないと、8章の再設計（OS側から見た姿を中心に置く）が成立しない。
async fn 物理ポートを持たないインターフェースを登録できる(
    db: &DatabaseConnection,
) {
    let d = 機器(db, "sw-01").await;

    for (name, kind) in [
        ("bond0", "Bond"),
        ("bond0.100", "Vlan"),
        ("Vlan100", "Svi"),
        ("vnet0", "Virtual"),
    ] {
        let 結果 = os_interface::ActiveModel {
            device_id: Set(d.id),
            interface_type: Set(kind.to_owned()),
            // 物理ポートに紐づかない
            part_instance_id: Set(None),
            port_slot_id: Set(None),
            os_interface_name: Set(name.to_owned()),
            aggregation_mode: Set(if kind == "Bond" {
                Some("LACP".to_owned())
            } else {
                None
            }),
            work_order_id: Set(None),
            from_date: Set(Utc::now()),
            to_date: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await;

        assert!(結果.is_ok(), "{name}（{kind}）を登録できません");
    }
}

/// **ボンドとVLANサブインターフェースを同じ中間テーブルで表せること**（8.5）。
///
/// ボンドは 1 upper : N lower、VLANサブインターフェースは 1 lower : N upper。
/// 向きが逆なので、片方向の外部キーでは両方を表せない。
async fn ボンドとvlanサブインターフェースを同じ形で表せる(
    db: &DatabaseConnection,
) {
    let d = 機器(db, "srv-01").await;
    let ens1 = 物理インターフェース(db, d.id, "ens1f0").await;
    let ens2 = 物理インターフェース(db, d.id, "ens1f1").await;
    let bond = 論理インターフェース(db, d.id, "bond0", "Bond").await;
    let vlan100 = 論理インターフェース(db, d.id, "bond0.100", "Vlan").await;
    let vlan200 = 論理インターフェース(db, d.id, "bond0.200", "Vlan").await;

    // ボンド：1 upper（bond0）に 2 lower（ens1f0, ens1f1）
    積む(db, bond.id, ens1.id).await;
    積む(db, bond.id, ens2.id).await;
    // VLANサブインターフェース：1 lower（bond0）に 2 upper
    積む(db, vlan100.id, bond.id).await;
    積む(db, vlan200.id, bond.id).await;

    let bondの下 = interface_stack::Entity::find()
        .filter(interface_stack::Column::UpperInterfaceId.eq(bond.id))
        .filter(interface_stack::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap();
    assert_eq!(bondの下.len(), 2, "ボンドの構成ポートが引けません");

    let bondの上 = interface_stack::Entity::find()
        .filter(interface_stack::Column::LowerInterfaceId.eq(bond.id))
        .filter(interface_stack::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap();
    assert_eq!(bondの上.len(), 2, "VLANサブインターフェースが引けません");
}

/// ポートVLANとタグVLANの双方を表せること（設計書8.3）。
async fn ポートvlanとタグvlanを表せる(db: &DatabaseConnection) {
    let user = 利用者(db, "vlan@example.com").await;
    let d = 機器(db, "sw-02").await;
    let port = 物理インターフェース(db, d.id, "Eth1/1/1").await;

    let v10 = vlan_を作る(db, 10, "管理", user.id).await;
    let v20 = vlan_を作る(db, 20, "業務", user.id).await;

    for (v, mode) in [(&v10, "Untagged"), (&v20, "Tagged")] {
        interface_vlan::ActiveModel {
            os_interface_id: Set(port.id),
            vlan_id: Set(v.id),
            tagging_mode: Set(mode.to_owned()),
            work_order_id: Set(None),
            from_date: Set(Utc::now()),
            to_date: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let 載っているvlan = interface_vlan::Entity::find()
        .filter(interface_vlan::Column::OsInterfaceId.eq(port.id))
        .filter(interface_vlan::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap();
    assert_eq!(載っているvlan.len(), 2);
    assert!(載っているvlan.iter().any(|v| v.tagging_mode == "Untagged"));
    assert!(載っているvlan.iter().any(|v| v.tagging_mode == "Tagged"));
}

// ---------------------------------------------------------------------------
// IP_ADDRESS（14.2）
// ---------------------------------------------------------------------------

/// **同一サブネット内でIPが重複できないこと**（設計書14.2）。
///
/// 本設計では珍しく、業務ルールをDB制約で担保する。部分インデックスは両DBが
/// 対応しており、IPの重複は「あとで直せばよい」類の誤りではない。
async fn 同一サブネット内でipは重複できない(db: &DatabaseConnection) {
    let user = 利用者(db, "ip@example.com").await;
    let p = プロジェクト(db, "IP検証").await;
    let net = サブネット(db, Some(p.id), "192.168.1.0/24", user.id).await;

    let d1 = 機器(db, "srv-a").await;
    let d2 = 機器(db, "srv-b").await;
    let if1 = 物理インターフェース(db, d1.id, "eth0").await;
    let if2 = 物理インターフェース(db, d2.id, "eth0").await;

    assert!(ip(db, if1.id, "192.168.1.10", Some(net.id)).await.is_ok());

    let 重複 = ip(db, if2.id, "192.168.1.10", Some(net.id)).await;
    assert!(重複.is_err(), "同一サブネットでIPが重複できてしまいました");
}

/// **サブネットが違えば同じIPを使えること**（設計書14.2）。
///
/// プロジェクトごとに 192.168.1.0/24 を独立に使えることが、サブネットを
/// プロジェクト単位に分けた狙いである。
async fn サブネットが違えば同じipを使える(db: &DatabaseConnection) {
    let user = 利用者(db, "ip2@example.com").await;
    let a = プロジェクト(db, "プロジェクトA").await;
    let b = プロジェクト(db, "プロジェクトB").await;
    let net_a = サブネット(db, Some(a.id), "192.168.1.0/24", user.id).await;
    let net_b = サブネット(db, Some(b.id), "192.168.1.0/24", user.id).await;

    let d1 = 機器(db, "a-srv").await;
    let d2 = 機器(db, "b-srv").await;
    let if1 = 物理インターフェース(db, d1.id, "eth0").await;
    let if2 = 物理インターフェース(db, d2.id, "eth0").await;

    assert!(ip(db, if1.id, "192.168.1.10", Some(net_a.id)).await.is_ok());
    assert!(
        ip(db, if2.id, "192.168.1.10", Some(net_b.id)).await.is_ok(),
        "別プロジェクトの同じアドレス帯が使えません"
    );
}

/// **過去に使ったIPを再利用できること**（設計書14.2）。
///
/// 制約は `WHERE to_date IS NULL` に限る。閉じた行まで対象にすると、
/// 機器を入れ替えたときに同じIPを振り直せなくなる。
async fn 解放したipは再利用できる(db: &DatabaseConnection) {
    let user = 利用者(db, "ip3@example.com").await;
    let p = プロジェクト(db, "再利用検証").await;
    let net = サブネット(db, Some(p.id), "10.0.0.0/24", user.id).await;

    let 旧機 = 機器(db, "old-srv").await;
    let 新機 = 機器(db, "new-srv").await;
    let 旧if = 物理インターフェース(db, 旧機.id, "eth0").await;
    let 新if = 物理インターフェース(db, 新機.id, "eth0").await;

    let 旧 = ip(db, 旧if.id, "10.0.0.5", Some(net.id)).await.unwrap();

    // 旧機を撤去：行を閉じる
    let mut active: ip_address::ActiveModel = 旧.into();
    active.to_date = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    assert!(
        ip(db, 新if.id, "10.0.0.5", Some(net.id)).await.is_ok(),
        "解放したIPを再利用できません"
    );
}

/// VLANタグは拠点をまたいで重複しうること（設計書8.3）。
///
/// L2ドメインごとに独立しているため、一意制約を張ってはならない。
async fn vlanタグは重複できる(db: &DatabaseConnection) {
    let user = 利用者(db, "vlantag@example.com").await;
    assert!(vlan_を作れるか(db, 100, "拠点Aの管理", user.id).await);
    assert!(
        vlan_を作れるか(db, 100, "拠点Bの管理", user.id).await,
        "同じVLANタグを別拠点で使えません"
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証".to_owned()),
        email: Set(email.to_owned()),
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

async fn 機器(db: &DatabaseConnection, hostname: &str) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        external_id: Set(None),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(None),
        device_type: Set("Physical".to_owned()),
        device_category: Set(Some("Server".to_owned())),
        hostname: Set(hostname.to_owned()),
        serial_number: Set(None),
        asset_number: Set(None),
        power_watt: Set(0),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 物理インターフェース(
    db: &DatabaseConnection,
    device_id: i32,
    name: &str,
) -> os_interface::Model {
    論理インターフェース(db, device_id, name, "Physical").await
}

async fn 論理インターフェース(
    db: &DatabaseConnection,
    device_id: i32,
    name: &str,
    kind: &str,
) -> os_interface::Model {
    os_interface::ActiveModel {
        device_id: Set(device_id),
        interface_type: Set(kind.to_owned()),
        part_instance_id: Set(None),
        port_slot_id: Set(None),
        os_interface_name: Set(name.to_owned()),
        aggregation_mode: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 積む(db: &DatabaseConnection, upper: i32, lower: i32) {
    interface_stack::ActiveModel {
        upper_interface_id: Set(upper),
        lower_interface_id: Set(lower),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

async fn vlan_を作る(
    db: &DatabaseConnection,
    tag: i32,
    name: &str,
    user_id: i32,
) -> vlan::Model {
    vlan::ActiveModel {
        vlan_tag: Set(tag),
        name: Set(name.to_owned()),
        zone: Set(Some("LAN".to_owned())),
        description: Set(String::new()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn vlan_を作れるか(db: &DatabaseConnection, tag: i32, name: &str, user_id: i32) -> bool {
    vlan::ActiveModel {
        vlan_tag: Set(tag),
        name: Set(name.to_owned()),
        zone: Set(None),
        description: Set(String::new()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .is_ok()
}

async fn サブネット(
    db: &DatabaseConnection,
    project_id: Option<i32>,
    cidr: &str,
    user_id: i32,
) -> subnet::Model {
    subnet::ActiveModel {
        project_id: Set(project_id),
        vlan_id: Set(None),
        cidr: Set(cidr.to_owned()),
        zone: Set(None),
        description: Set(String::new()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn ip(
    db: &DatabaseConnection,
    os_interface_id: i32,
    address: &str,
    subnet_id: Option<i32>,
) -> Result<ip_address::Model, sea_orm::DbErr> {
    ip_address::ActiveModel {
        os_interface_id: Set(os_interface_id),
        ip_address: Set(address.to_owned()),
        prefix_length: Set(24),
        subnet_id: Set(subnet_id),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 物理ポートを持たないインターフェースを登録できる);
        全検証!(@one $用意, $属性, ボンドとvlanサブインターフェースを同じ形で表せる);
        全検証!(@one $用意, $属性, ポートvlanとタグvlanを表せる);
        全検証!(@one $用意, $属性, 同一サブネット内でipは重複できない);
        全検証!(@one $用意, $属性, サブネットが違えば同じipを使える);
        全検証!(@one $用意, $属性, 解放したipは再利用できる);
        全検証!(@one $用意, $属性, vlanタグは重複できる);
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
