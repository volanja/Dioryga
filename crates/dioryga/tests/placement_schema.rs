//! 物理設置のスキーマの結合テスト（設計書12章、13.2）。

mod support;

use chrono::Utc;
use entity::{app_user, device, device_mount, mount_container, warehouse};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

/// 什器に直接搭載できること（設計書12.2）。
async fn 什器に搭載できる(db: &DatabaseConnection) {
    let user = 利用者(db, "mount@example.com").await;
    let w = 倉庫(db, "本社倉庫", user.id).await;
    let rack = 什器(db, "Rack-01", "Rack", "Warehouse", w.id, Some(42), user.id).await;
    let d = 機器(db, "srv-01").await;

    device_mount::ActiveModel {
        device_id: Set(d.id),
        container_id: Set(Some(rack.id)),
        position: Set(Some(10)),
        horizontal_position: Set(Some("Full".to_owned())),
        depth_position: Set(Some("Front".to_owned())),
        host_device_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 現在 = 現在の搭載(db, d.id).await;
    assert_eq!(現在.container_id, Some(rack.id));
    assert_eq!(現在.position, Some(10));
    assert!(現在.host_device_id.is_none());
}

/// **他の機器の上にも載せられること**（設計書12.2）。
///
/// 棚板やハイパーバイザの上に載る場合。`container_id` は使わない。
async fn 機器の上にも載せられる(db: &DatabaseConnection) {
    let host = 機器(db, "esxi-01").await;
    let vm = 機器(db, "vm-01").await;

    device_mount::ActiveModel {
        device_id: Set(vm.id),
        container_id: Set(None),
        position: Set(None),
        horizontal_position: Set(None),
        depth_position: Set(None),
        host_device_id: Set(Some(host.id)),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 現在 = 現在の搭載(db, vm.id).await;
    assert_eq!(現在.host_device_id, Some(host.id));
    assert!(
        現在.container_id.is_none(),
        "什器と機器の両方が埋まっています"
    );
}

/// 搭載位置の変更が履歴として残ること（不変条件1）。
async fn 移設は履歴として残る(db: &DatabaseConnection) {
    let user = 利用者(db, "relocate@example.com").await;
    let w = 倉庫(db, "移設元", user.id).await;
    let 旧ラック = 什器(db, "Rack-A", "Rack", "Warehouse", w.id, Some(42), user.id).await;
    let 新ラック = 什器(db, "Rack-B", "Rack", "Warehouse", w.id, Some(42), user.id).await;
    let d = 機器(db, "move-me").await;

    let 旧 = device_mount::ActiveModel {
        device_id: Set(d.id),
        container_id: Set(Some(旧ラック.id)),
        position: Set(Some(5)),
        horizontal_position: Set(None),
        depth_position: Set(None),
        host_device_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // 閉じて開く
    let mut 閉じる: device_mount::ActiveModel = 旧.clone().into();
    閉じる.to_date = Set(Some(Utc::now()));
    閉じる.update(db).await.unwrap();

    device_mount::ActiveModel {
        device_id: Set(d.id),
        container_id: Set(Some(新ラック.id)),
        position: Set(Some(20)),
        horizontal_position: Set(None),
        depth_position: Set(None),
        host_device_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 全部 = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(d.id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(全部.len(), 2, "移設前の位置が失われています");
    assert_eq!(現在の搭載(db, d.id).await.container_id, Some(新ラック.id));
}

/// **ラック図はこの什器に載っているものを1回で引けること**（設計書12.2）。
async fn 什器から搭載機器を引ける(db: &DatabaseConnection) {
    let user = 利用者(db, "rackview@example.com").await;
    let w = 倉庫(db, "図面用", user.id).await;
    let rack = 什器(
        db,
        "Rack-View",
        "Rack",
        "Warehouse",
        w.id,
        Some(42),
        user.id,
    )
    .await;

    for (i, name) in ["sw-01", "srv-01", "srv-02"].iter().enumerate() {
        let d = 機器(db, name).await;
        device_mount::ActiveModel {
            device_id: Set(d.id),
            container_id: Set(Some(rack.id)),
            position: Set(Some(i as i32 * 2 + 1)),
            horizontal_position: Set(None),
            depth_position: Set(None),
            host_device_id: Set(None),
            work_order_id: Set(None),
            from_date: Set(Utc::now()),
            to_date: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let 搭載中 = device_mount::Entity::find()
        .filter(device_mount::Column::ContainerId.eq(rack.id))
        .filter(device_mount::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap();
    assert_eq!(搭載中.len(), 3);
}

/// 収容能力を持たない什器も登録できること（設計書12章）。
///
/// **超過はエラーではなく警告**として扱うため（不変条件6）、`capacity` は
/// 制約ではなく目安である。そもそも持たない什器もある。
async fn 収容能力なしの什器も登録できる(db: &DatabaseConnection) {
    let user = 利用者(db, "nocap@example.com").await;
    let w = 倉庫(db, "作業机置き場", user.id).await;

    let 結果 = mount_container::ActiveModel {
        name: Set("作業机".to_owned()),
        container_type: Set("Desk".to_owned()),
        location_type: Set("Warehouse".to_owned()),
        location_id: Set(w.id),
        capacity: Set(None),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;

    assert!(結果.is_ok());
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 現在の搭載(db: &DatabaseConnection, device_id: i32) -> device_mount::Model {
    device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .expect("現在の搭載位置が見つかりません")
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証".to_owned()),
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

async fn 倉庫(db: &DatabaseConnection, name: &str, user_id: i32) -> warehouse::Model {
    warehouse::ActiveModel {
        name: Set(name.to_owned()),
        address: Set(String::new()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn 什器(
    db: &DatabaseConnection,
    name: &str,
    container_type: &str,
    location_type: &str,
    location_id: i32,
    capacity: Option<i32>,
    user_id: i32,
) -> mount_container::Model {
    mount_container::ActiveModel {
        name: Set(name.to_owned()),
        container_type: Set(container_type.to_owned()),
        location_type: Set(location_type.to_owned()),
        location_id: Set(location_id),
        capacity: Set(capacity),
        created_by: Set(user_id),
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
        power_watt: Set(350),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 什器に搭載できる);
        全検証!(@one $用意, $属性, 機器の上にも載せられる);
        全検証!(@one $用意, $属性, 移設は履歴として残る);
        全検証!(@one $用意, $属性, 什器から搭載機器を引ける);
        全検証!(@one $用意, $属性, 収容能力なしの什器も登録できる);
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
