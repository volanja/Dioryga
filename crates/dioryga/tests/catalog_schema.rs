//! 共有カタログのスキーマの結合テスト（設計書6章、8章、9章、18章）。
//!
//! 画面はまだ無いため、**マイグレーションが両DBで通ること**と、**設計上重要な
//! 制約が実際に効いていること**を確かめる。ここで固定しておかないと、後から
//! 画面を作る段階で「制約があるつもりだった」という取り違えが起きる。

mod support;

use chrono::Utc;
use dioryga::repository::{Actor, AuditedTx};
use entity::{
    app_user, cable_catalog, cable_end_slot, chassis_model, configuration, configuration_part,
    part_catalog, software_catalog, vendor,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};

// ---------------------------------------------------------------------------
// 一意制約
// ---------------------------------------------------------------------------

/// ベンダー名は重複できないこと（設計書18.3）。
///
/// 表記ゆれを防ぐためのマスタなので、同名を2件持てると存在意義が薄れる。
async fn ベンダー名は重複できない(db: &DatabaseConnection) {
    let user = 利用者(db, "vendor@example.com").await;
    ベンダー(db, "Dell", user.id).await;

    let 結果 = vendor::ActiveModel {
        name: Set("Dell".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;

    assert!(結果.is_err(), "同名のベンダーが登録できてしまいました");
}

/// 筐体モデルの自然キーは `(vendor_id, model_name)` であること（設計書6.2）。
///
/// **ベンダーが違えば同じ型番を持てる。**別のベンダーが同じ文字列の型番を
/// 使うことは実際にある。
async fn 筐体モデルはベンダーごとに型番が一意(db: &DatabaseConnection) {
    let user = 利用者(db, "chassis@example.com").await;
    let a = ベンダー(db, "VendorA", user.id).await;
    let b = ベンダー(db, "VendorB", user.id).await;

    筐体モデル(db, a.id, "R750", user.id).await;

    // 別ベンダーなら同じ型番を登録できる
    let 別ベンダー = chassis_model::ActiveModel {
        vendor_id: Set(b.id),
        model_name: Set("R750".to_owned()),
        device_category: Set("Server".to_owned()),
        height_u: Set(2),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(Some("Full".to_owned())),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;
    assert!(別ベンダー.is_ok(), "別ベンダーの同型番が登録できません");

    // 同じベンダーで同じ型番は拒否される
    let 同一 = chassis_model::ActiveModel {
        vendor_id: Set(a.id),
        model_name: Set("R750".to_owned()),
        device_category: Set("Server".to_owned()),
        height_u: Set(2),
        mount_form: Set("RackU".to_owned()),
        rack_width: Set(None),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;
    assert!(同一.is_err(), "同一ベンダーで型番が重複できてしまいました");
}

/// 部品カタログは `(vendor_id, part_number)` が一意であること（設計書7.2ケースE）。
async fn 部品はベンダーごとに型番が一意(db: &DatabaseConnection) {
    let user = 利用者(db, "part@example.com").await;
    let v = ベンダー(db, "PartVendor", user.id).await;
    部品(db, v.id, "P-001", user.id).await;

    let 結果 = part_catalog::ActiveModel {
        category: Set("CPU".to_owned()),
        vendor_id: Set(v.id),
        part_number: Set("P-001".to_owned()),
        core_count: Set(None),
        capacity_gb: Set(None),
        spec_json: Set("{}".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;

    assert!(結果.is_err());
}

/// 同じ構成に同じ部品が2行現れないこと。数量は `quantity` で表す（設計書6.2）。
async fn 構成の部品は重複しない(db: &DatabaseConnection) {
    let user = 利用者(db, "config@example.com").await;
    let v = ベンダー(db, "ConfigVendor", user.id).await;
    let model = 筐体モデル(db, v.id, "CFG-1", user.id).await;
    let part = 部品(db, v.id, "CFG-P1", user.id).await;

    let config = configuration::ActiveModel {
        chassis_model_id: Set(model.id),
        name: Set("標準構成".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 追加 = |quantity: i32| configuration_part::ActiveModel {
        configuration_id: Set(config.id),
        part_catalog_id: Set(part.id),
        quantity: Set(quantity),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    };

    assert!(追加(2).insert(db).await.is_ok());
    assert!(
        追加(1).insert(db).await.is_err(),
        "同じ部品が2行登録できてしまいました"
    );
}

/// **purl を持たない行は何件でも共存できること**（設計書9.4）。
///
/// `purl` はUNIQUEだが nullable であり、**両DBともNULL同士は重複と見なさない。**
/// 空文字で保存すると2件目から登録できなくなるため、NULLで持つ必要がある。
async fn purlなしのソフトウェアは複数登録できる(db: &DatabaseConnection) {
    let user = 利用者(db, "sw@example.com").await;

    for (name, version) in [("社内ツールA", "1.0"), ("社内ツールB", "1.0")] {
        let 結果 = software_catalog::ActiveModel {
            name: Set(name.to_owned()),
            vendor_id: Set(None),
            version: Set(version.to_owned()),
            category: Set("Application".to_owned()),
            purl: Set(None),
            license_expression: Set(String::new()),
            spec_json: Set("{}".to_owned()),
            created_by: Set(user.id),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(db)
        .await;
        assert!(結果.is_ok(), "{name} を登録できません");
    }
}

// ---------------------------------------------------------------------------
// 型の扱い（設計書24.2）
// ---------------------------------------------------------------------------

/// **ケーブル長はミリメートルの整数で往復すること**（設計書24.2.1）。
///
/// SQLiteに `DECIMAL` が無いため整数で持つ。小数で持つと丸め誤差が出る。
async fn ケーブル長はミリメートルの整数で往復する(db: &DatabaseConnection) {
    let user = 利用者(db, "cable@example.com").await;

    let cable = cable_catalog::ActiveModel {
        cable_type: Set("PowerCord".to_owned()),
        // 1.8m
        length_mm: Set(Some(1800)),
        color: Set("black".to_owned()),
        vendor_id: Set(None),
        part_number: Set(None),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 読み直し = cable_catalog::Entity::find_by_id(cable.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(読み直し.length_mm, Some(1800));
}

/// **ケーブルの両端が異なるコネクタを持てること**（設計書8.7）。
///
/// 100V電源コードは NEMA 5-15P と C13 で両端が異なる。光ファイバの LC-SC も同様。
/// 片側だけを持つ設計にすると、これらを表せない。
async fn ケーブルの両端は非対称にできる(db: &DatabaseConnection) {
    let user = 利用者(db, "asym@example.com").await;

    let cable = cable_catalog::ActiveModel {
        cable_type: Set("PowerCord".to_owned()),
        length_mm: Set(Some(1800)),
        color: Set(String::new()),
        vendor_id: Set(None),
        part_number: Set(None),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    for (label, connector) in [("A", "NEMA 5-15P"), ("B", "C13")] {
        cable_end_slot::ActiveModel {
            cable_catalog_id: Set(cable.id),
            end_label: Set(label.to_owned()),
            connector_type: Set(connector.to_owned()),
            port_speed: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }

    let ends = cable_end_slot::Entity::find().all(db).await.unwrap();
    assert_eq!(ends.len(), 2);
    assert_ne!(
        ends[0].connector_type, ends[1].connector_type,
        "両端が同じコネクタになっています"
    );
}

// ---------------------------------------------------------------------------
// 監査ログ
// ---------------------------------------------------------------------------

/// カタログの追加が監査ログに残ること（設計書24.4、18.1）。
///
/// カタログは全プロジェクトが共有するため、**誰が変えたかを追えることが要る。**
async fn カタログの追加が監査ログに残る(db: &DatabaseConnection) {
    let user = 利用者(db, "audit-catalog@example.com").await;

    let tx = AuditedTx::begin(db, Actor::User(user.id)).await.unwrap();
    tx.insert(vendor::ActiveModel {
        name: Set("監査対象ベンダー".to_owned()),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let logs = entity::audit_log::Entity::find().all(db).await.unwrap();
    let 該当 = logs
        .iter()
        .find(|l| l.table_name == "vendor")
        .expect("監査ログが記録されていません");

    assert_eq!(該当.action, "insert");
    assert_eq!(該当.user_id, user.id);
    assert!(該当
        .after_json
        .as_deref()
        .unwrap()
        .contains("監査対象ベンダー"));
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
        device_category: Set("Server".to_owned()),
        height_u: Set(2),
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

async fn 部品(
    db: &DatabaseConnection,
    vendor_id: i32,
    part_number: &str,
    user_id: i32,
) -> part_catalog::Model {
    part_catalog::ActiveModel {
        category: Set("CPU".to_owned()),
        vendor_id: Set(vendor_id),
        part_number: Set(part_number.to_owned()),
        core_count: Set(Some(32)),
        capacity_gb: Set(None),
        spec_json: Set(r#"{"base_clock_ghz":2.1}"#.to_owned()),
        created_by: Set(user_id),
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
        全検証!(@one $用意, $属性, ベンダー名は重複できない);
        全検証!(@one $用意, $属性, 筐体モデルはベンダーごとに型番が一意);
        全検証!(@one $用意, $属性, 部品はベンダーごとに型番が一意);
        全検証!(@one $用意, $属性, 構成の部品は重複しない);
        全検証!(@one $用意, $属性, purlなしのソフトウェアは複数登録できる);
        全検証!(@one $用意, $属性, ケーブル長はミリメートルの整数で往復する);
        全検証!(@one $用意, $属性, ケーブルの両端は非対称にできる);
        全検証!(@one $用意, $属性, カタログの追加が監査ログに残る);
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
