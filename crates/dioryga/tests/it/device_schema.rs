//! 機器のスキーマの結合テスト（設計書6章、8.6、23章）。
//!
//! 画面はまだ無いため、**マイグレーションが両DBで通ること**と、**履歴テーブルの
//! 扱いが設計どおりであること**を確かめる。特に「閉じて開く」という書き方が
//! 実際に成立するかは、ここで固定しておかないと後続の画面がばらばらになる。

use chrono::{Duration, Utc};
use entity::{
    app_user, chassis_model, configuration, device, device_assignment, device_stack,
    firmware_version, part_catalog, part_instance, part_instance_location, vendor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};

// ---------------------------------------------------------------------------
// DEVICE
// ---------------------------------------------------------------------------

/// `uid` が一意で、`serial_number` と `asset_number` は無くても登録できること。
///
/// **採番待ちで登録できないと実務が回らない**（設計書23.2）。仮想機器には
/// シリアル番号がそもそも存在しない（6.2）。
async fn 識別子が無くても登録できる(db: &DatabaseConnection) {
    let vm = 機器(db, "vm-01", "Virtual", None).await;
    assert!(vm.serial_number.is_none());
    assert!(vm.asset_number.is_none());

    // 別の機器も同じく識別子なしで登録できる（NULL同士は重複と見なされない）
    let vm2 = 機器(db, "vm-02", "Virtual", None).await;
    assert_ne!(vm.uid, vm2.uid);

    // uid は一意
    let 重複 = device::ActiveModel {
        uid: Set(vm.uid.clone()),
        external_id: Set(None),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(None),
        device_type: Set("Virtual".to_owned()),
        device_category: Set(Some("Server".to_owned())),
        hostname: Set("dup".to_owned()),
        serial_number: Set(None),
        asset_number: Set(None),
        power_watt: Set(0),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;
    assert!(重複.is_err(), "uidが重複できてしまいました");
}

/// **重複統合は削除ではなくリダイレクトであること**（設計書23.9）。
///
/// 吸収された側の行は残り、統合先を指す。物理削除すると過去の記録が壊れる。
async fn 統合された機器は残る(db: &DatabaseConnection) {
    let 残す = 機器(db, "keep-01", "Physical", None).await;
    let 吸収 = 機器(db, "merge-01", "Physical", None).await;

    let mut active: device::ActiveModel = 吸収.clone().into();
    active.merged_into_device_id = Set(Some(残す.id));
    active.merged_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    let 後 = device::Entity::find_by_id(吸収.id)
        .one(db)
        .await
        .unwrap()
        .expect("統合された機器が物理削除されています");
    assert_eq!(後.merged_into_device_id, Some(残す.id));
    assert_eq!(device::Entity::find().count(db).await.unwrap(), 2);
}

// ---------------------------------------------------------------------------
// 履歴テーブル
// ---------------------------------------------------------------------------

/// **履歴は「閉じて開く」で書けること**（不変条件1、設計書4章）。
///
/// 既存行を `UPDATE` で書き換えず、`to_date` を入れて閉じ、新しい行を開く。
/// 現在の所在は `to_date IS NULL` の1行として引ける。
async fn 所在の履歴は閉じて開く(db: &DatabaseConnection) {
    let d = 機器(db, "move-01", "Physical", None).await;
    let 昨日 = Utc::now() - Duration::days(1);
    let 今日 = Utc::now();

    // 倉庫にあった
    let 旧 = device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Warehouse".to_owned()),
        location_id: Set(Some(1)),
        work_order_id: Set(None),
        from_date: Set(昨日),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // プロジェクトへ移した：旧行を閉じ、新行を開く
    let mut 閉じる: device_assignment::ActiveModel = 旧.clone().into();
    閉じる.to_date = Set(Some(今日));
    閉じる.update(db).await.unwrap();

    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(7)),
        work_order_id: Set(None),
        from_date: Set(今日),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // 履歴は2行残る
    let 全部 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(d.id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(全部.len(), 2, "履歴が上書きされています");

    // 現在の所在は to_date IS NULL の1行
    let 現在 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(d.id))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap();
    assert_eq!(現在.len(), 1);
    assert_eq!(現在[0].location_type, "Project");
}

/// 廃棄は `location_id` が null になること（設計書6.2）。
///
/// **`status` には `disposed` を持たない。**所在から導出する（旧B-1）。
async fn 廃棄は所在で表す(db: &DatabaseConnection) {
    let d = 機器(db, "dispose-01", "Physical", None).await;

    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Disposed".to_owned()),
        location_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 現在 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(d.id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(現在.location_type, "Disposed");
    assert!(現在.location_id.is_none());

    // 機器側の status は所在と独立している
    let 機器の状態 = device::Entity::find_by_id(d.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(機器の状態.status, "running");
}

/// 部品はスロットを指定しなくても機器に載せられること（設計書6.2）。
///
/// `chassis_slot_id` は**任意項目**であり、どのスロットに挿さっているかまでは
/// 求めない。求めると取込のたびに実機を開けることになる。
async fn 部品はスロット未指定で載せられる(db: &DatabaseConnection) {
    let user = 利用者(db, "part-loc@example.com").await;
    let v = ベンダー(db, "PartVendor", user.id).await;
    let catalog = part_catalog::ActiveModel {
        category: Set("Memory".to_owned()),
        vendor_id: Set(v.id),
        part_number: Set("M-64G".to_owned()),
        core_count: Set(None),
        capacity_gb: Set(Some(64)),
        spec_json: Set("{}".to_owned()),
        created_by: Set(user.id),
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

    let d = 機器(db, "host-01", "Physical", None).await;

    let 結果 = part_instance_location::ActiveModel {
        part_instance_id: Set(instance.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(d.id)),
        // スロットは指定しない
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await;

    assert!(結果.is_ok(), "スロット未指定で載せられません");
}

/// ファームウェアの履歴が機器と部品の双方を対象にできること（設計書6.2）。
async fn ファームウェアは機器と部品の双方を指せる(db: &DatabaseConnection) {
    let user = 利用者(db, "fw@example.com").await;
    let d = 機器(db, "fw-01", "Physical", None).await;

    for (item_type, item_id, component) in [("Device", d.id, "BIOS"), ("PartInstance", 1, "NIC")] {
        firmware_version::ActiveModel {
            item_type: Set(item_type.to_owned()),
            item_id: Set(item_id),
            component: Set(component.to_owned()),
            version: Set("1.0.0".to_owned()),
            work_order_id: Set(None),
            changed_by: Set(user.id),
            from_date: Set(Utc::now()),
            to_date: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    assert_eq!(firmware_version::Entity::find().count(db).await.unwrap(), 2);
}

/// **スタックは論理DEVICEと物理DEVICEを結ぶこと**（設計書8.6）。
///
/// スタック全体を `Logical` の機器として登録し、筐体は `Physical` として別に持つ。
/// 論理側は構成もシリアル番号も持たない。
async fn スタックは論理と物理を結ぶ(db: &DatabaseConnection) {
    let user = 利用者(db, "stack@example.com").await;
    let v = ベンダー(db, "SwitchVendor", user.id).await;
    let model = 筐体モデル(db, v.id, "SW-48P", user.id).await;
    let config = configuration::ActiveModel {
        chassis_model_id: Set(model.id),
        name: Set("標準".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 論理 = 機器(db, "stack-01", "Logical", None).await;
    let 筐体1 = 機器(db, "stack-01-member1", "Physical", Some(config.id)).await;
    let 筐体2 = 機器(db, "stack-01-member2", "Physical", Some(config.id)).await;

    // 論理側は構成を持たない（8.6）
    assert!(論理.configuration_id.is_none());
    assert!(筐体1.configuration_id.is_some());

    for (i, member) in [筐体1, 筐体2].iter().enumerate() {
        device_stack::ActiveModel {
            logical_device_id: Set(論理.id),
            member_device_id: Set(member.id),
            member_number: Set(i as i32 + 1),
            from_date: Set(Utc::now()),
            to_date: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let 構成筐体 = device_stack::Entity::find()
        .filter(device_stack::Column::LogicalDeviceId.eq(論理.id))
        .filter(device_stack::Column::ToDate.is_null())
        .all(db)
        .await
        .unwrap();
    assert_eq!(構成筐体.len(), 2);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

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

async fn ベンダー(db: &DatabaseConnection, name: &str, user_id: i32) -> vendor::Model {
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
}

async fn 筐体モデル(
    db: &DatabaseConnection,
    vendor_id: i32,
    model_name: &str,
    user_id: i32,
) -> chassis_model::Model {
    chassis_model::ActiveModel {
        vendor_id: Set(vendor_id),
        model_name: Set(model_name.to_owned()),
        device_category: Set("Switch".to_owned()),
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
    .unwrap()
}

async fn 機器(
    db: &DatabaseConnection,
    hostname: &str,
    device_type: &str,
    configuration_id: Option<i32>,
) -> device::Model {
    device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        external_id: Set(None),
        merged_into_device_id: Set(None),
        merged_at: Set(None),
        configuration_id: Set(configuration_id),
        device_type: Set(device_type.to_owned()),
        device_category: Set(configuration_id.is_none().then(|| "Server".to_owned())),
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

/// **機器・部品の状態の語彙が書き換わり、戻せること**（#173）。
///
/// 語彙はDB制約にせずアプリケーションで検証する（設計書8.6）ため、表を改めた
/// だけでは既存の行が語彙外の値として残る。
async fn 状態の語彙を書き換える(db: &DatabaseConnection) {
    use migration::m20260922_000001_rename_device_status::Migration as 語彙変更;
    use migration::{MigrationTrait, SchemaManager};

    // **このマイグレーションだけを名指しで戻す。**後から足しても対象がずれない
    let manager = SchemaManager::new(db);
    語彙変更.down(&manager).await.unwrap();

    let mut ids = Vec::new();
    for (name, status) in [
        ("old-plan", "plan"),
        ("old-building", "building"),
        ("old-repair", "repair"),
        ("old-broken", "broken"),
        ("old-running", "running"),
    ] {
        let d = 機器(db, name, "Physical", None).await;
        let mut active: device::ActiveModel = d.clone().into();
        active.status = Set(status.to_owned());
        ids.push(active.update(db).await.unwrap().id);
    }

    let 状態 = |db: &DatabaseConnection| {
        let ids = ids.clone();
        let db = db.clone();
        async move {
            let mut 結果 = Vec::new();
            for id in ids {
                let d = device::Entity::find_by_id(id)
                    .one(&db)
                    .await
                    .unwrap()
                    .unwrap();
                結果.push(d.status);
            }
            結果
        }
    };

    語彙変更.up(&manager).await.unwrap();
    assert_eq!(
        状態(db).await,
        ["planned", "provisioning", "repairing", "failed", "running"]
    );

    語彙変更.down(&manager).await.unwrap();
    assert_eq!(
        状態(db).await,
        ["plan", "building", "repair", "broken", "running"]
    );

    // 試験の後始末。以降の検証は新しい語彙で動く
    語彙変更.up(&manager).await.unwrap();
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 識別子が無くても登録できる);
        全検証!(@one $用意, $属性, 状態の語彙を書き換える);
        全検証!(@one $用意, $属性, 統合された機器は残る);
        全検証!(@one $用意, $属性, 所在の履歴は閉じて開く);
        全検証!(@one $用意, $属性, 廃棄は所在で表す);
        全検証!(@one $用意, $属性, 部品はスロット未指定で載せられる);
        全検証!(@one $用意, $属性, ファームウェアは機器と部品の双方を指せる);
        全検証!(@one $用意, $属性, スタックは論理と物理を結ぶ);
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
