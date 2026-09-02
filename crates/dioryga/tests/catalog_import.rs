//! カタログYAML取込の結合テスト（設計書23章）。

mod support;

use chrono::Utc;
use dioryga::import::{catalog, Outcome};
use entity::{app_user, chassis_model, chassis_slot, configuration_part, part_catalog, vendor};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

const 基本: &str = r#"
format_version: 1
kind: catalog
vendors:
  - name: Fujitsu
chassis_models:
  - vendor: Fujitsu
    model_name: PRIMERGY RX4770 M8
    device_category: Server
    height_u: 2
    mount_form: RackU
    rack_width: Full
    slots:
      - { slot_type: CPU_SOCKET, count: 4,  label_format: "CPU{n}" }
      - { slot_type: DIMM,       count: 64, label_format: "DIMM{n}" }
      - { slot_type: PCIE,       labels: [Slot1, Slot2] }
part_catalogs:
  - &dimm64
    vendor: Fujitsu
    part_number: UCS-MRX64G2RE5
    category: Memory
    capacity_gb: 64
    spec_json: { speed: "DDR5-6400" }
configurations:
  - chassis_model: { vendor: Fujitsu, model_name: PRIMERGY RX4770 M8 }
    name: 標準構成
    parts:
      - { part: *dimm64, quantity: 16 }
"#;

// ---------------------------------------------------------------------------
// ドライラン（設計書23.6）
// ---------------------------------------------------------------------------

/// **ドライランはDBを書き換えないこと。**
///
/// 18.2により参照されたカタログ行は編集できず、誤った取込は事後修正が困難。
/// 差分を見てから実行する2段階が成立するには、まずここが要件になる。
async fn ドライランは書き換えない(db: &DatabaseConnection) {
    let file = catalog::parse(基本).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();

    assert_eq!(report.count(Outcome::Created), 4, "{report}");
    assert!(!report.has_error());

    assert_eq!(vendor::Entity::find().all(db).await.unwrap().len(), 0);
    assert_eq!(
        chassis_model::Entity::find().all(db).await.unwrap().len(),
        0
    );
}

/// 参照先が解決できなければエラーになること（設計書23.6）。
async fn 解決できない参照はエラー(db: &DatabaseConnection) {
    let yaml = r#"
format_version: 1
kind: catalog
chassis_models:
  - vendor: 存在しないベンダー
    model_name: M1
    device_category: Server
    mount_form: RackU
"#;
    let file = catalog::parse(yaml).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();

    assert!(report.has_error());
    assert!(report
        .errors()
        .any(|e| e.detail.contains("存在しないベンダー")));
}

/// スロットの記述が曖昧ならエラーになること（設計書23.4）。
async fn 曖昧なスロット記述はエラー(db: &DatabaseConnection) {
    let yaml = r#"
format_version: 1
kind: catalog
vendors:
  - name: V
chassis_models:
  - vendor: V
    model_name: M1
    device_category: Server
    mount_form: RackU
    slots:
      - { slot_type: DIMM, count: 4, labels: [A, B] }
"#;
    let file = catalog::parse(yaml).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();

    assert!(report.has_error());
    assert!(report.errors().any(|e| e.detail.contains("同時に指定")));
}

// ---------------------------------------------------------------------------
// 反映
// ---------------------------------------------------------------------------

/// 展開記法どおりにスロットが作られること（設計書23.4）。
async fn 取り込むとスロットが展開される(db: &DatabaseConnection) {
    let user = 利用者(db, "import@example.com").await;
    let file = catalog::parse(基本).unwrap();
    catalog::apply(db, &file, user.id, 1).await.unwrap();

    let model = chassis_model::Entity::find()
        .filter(chassis_model::Column::ModelName.eq("PRIMERGY RX4770 M8"))
        .one(db)
        .await
        .unwrap()
        .expect("筐体モデルが作られていません");

    let slots = chassis_slot::Entity::find()
        .filter(chassis_slot::Column::ChassisModelId.eq(model.id))
        .all(db)
        .await
        .unwrap();

    // CPU 4 + DIMM 64 + PCIE 2
    assert_eq!(slots.len(), 70);
    assert!(slots.iter().any(|s| s.slot_label == "DIMM64"));
    assert!(slots.iter().any(|s| s.slot_label == "CPU1"));
    assert!(slots.iter().any(|s| s.slot_label == "Slot2"));
    // **1始まりであること。**実機のラベルに合わせる
    assert!(!slots.iter().any(|s| s.slot_label == "DIMM0"));
}

/// アンカーで参照した部品が構成に結び付くこと（設計書23.4）。
async fn アンカー参照が構成に結び付く(db: &DatabaseConnection) {
    let user = 利用者(db, "anchor@example.com").await;
    let file = catalog::parse(基本).unwrap();
    catalog::apply(db, &file, user.id, 1).await.unwrap();

    let part = part_catalog::Entity::find()
        .filter(part_catalog::Column::PartNumber.eq("UCS-MRX64G2RE5"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(part.capacity_gb, Some(64));
    // JSONは文字列で持つ（24.2.3）
    assert!(part.spec_json.contains("DDR5-6400"));

    let link = configuration_part::Entity::find()
        .filter(configuration_part::Column::PartCatalogId.eq(part.id))
        .one(db)
        .await
        .unwrap()
        .expect("構成に部品が結び付いていません");
    assert_eq!(link.quantity, 16);
}

/// **同じファイルを2回流しても結果が変わらないこと**（設計書23.1）。
///
/// 宣言的な取込の核心。命令形にすると履歴行が重複して事実が壊れる。
/// 「1行直して再取込」という運用が成立するかどうかが、ここで決まる。
async fn 二度流しても結果が変わらない(db: &DatabaseConnection) {
    let user = 利用者(db, "twice@example.com").await;
    let file = catalog::parse(基本).unwrap();

    catalog::apply(db, &file, user.id, 1).await.unwrap();
    let 一回目 = 件数(db).await;

    let 二回目のレポート = catalog::apply(db, &file, user.id, 2).await.unwrap();
    let 二回目 = 件数(db).await;

    assert_eq!(一回目, 二回目, "二度目の取込で行が増えています");
    // 二度目はすべて「変更なし」になる
    assert_eq!(二回目のレポート.count(Outcome::Created), 0);
    assert_eq!(二回目のレポート.count(Outcome::Unchanged), 4);
}

/// **既存のカタログを上書きしないこと**（設計書18.2）。
///
/// 参照されたカタログ行は編集できない。黙って上書きすると、参照している
/// 機器の仕様が知らないうちに変わる。
async fn 既存のカタログを上書きしない(db: &DatabaseConnection) {
    let user = 利用者(db, "overwrite@example.com").await;
    catalog::apply(db, &catalog::parse(基本).unwrap(), user.id, 1)
        .await
        .unwrap();

    // 同じ自然キーで、高さと分類を変えたファイル
    let 書き換え = 基本
        .replace("height_u: 2", "height_u: 4")
        .replace("device_category: Server", "device_category: Storage");
    let file = catalog::parse(&書き換え).unwrap();
    catalog::apply(db, &file, user.id, 2).await.unwrap();

    let model = chassis_model::Entity::find()
        .filter(chassis_model::Column::ModelName.eq("PRIMERGY RX4770 M8"))
        .one(db)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(model.height_u, 2, "既存のカタログが上書きされました");
    assert_eq!(model.device_category, "Server");
}

/// **取込では行ごとの監査ログを書かないこと**（設計書24.4）。
///
/// 25万行の取込で25万行の監査ログが生まれると肥大する。すべて同じ主体・
/// 時刻・`import_run_id` を持つため、1行ごとに複製しても情報が増えない。
async fn 取込は行ごとの監査ログを書かない(db: &DatabaseConnection) {
    let user = 利用者(db, "audit-import@example.com").await;
    let file = catalog::parse(基本).unwrap();
    catalog::apply(db, &file, user.id, 1).await.unwrap();

    // ベンダー1 + モデル1 + スロット70 + 部品1 + 構成1 + 構成部品1 = 75行
    assert!(件数(db).await >= 75);

    let logs = entity::audit_log::Entity::find().all(db).await.unwrap();
    assert!(
        logs.is_empty(),
        "取込で監査ログが {} 件書かれています",
        logs.len()
    );
}

/// エラーがあれば1件も反映しないこと（設計書23.6）。
///
/// 部分的に入ると、どこまで入ったのかを利用者が把握できない。
async fn エラーがあれば何も反映しない(db: &DatabaseConnection) {
    let user = 利用者(db, "partial@example.com").await;
    let yaml = r#"
format_version: 1
kind: catalog
vendors:
  - name: 正しいベンダー
chassis_models:
  - vendor: 存在しないベンダー
    model_name: M1
    device_category: Server
    mount_form: RackU
"#;
    let file = catalog::parse(yaml).unwrap();
    let 結果 = catalog::apply(db, &file, user.id, 1).await;

    assert!(結果.is_err());
    assert_eq!(
        vendor::Entity::find().all(db).await.unwrap().len(),
        0,
        "エラーがあるのに一部が反映されています"
    );
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 件数(db: &DatabaseConnection) -> usize {
    vendor::Entity::find().all(db).await.unwrap().len()
        + chassis_model::Entity::find().all(db).await.unwrap().len()
        + chassis_slot::Entity::find().all(db).await.unwrap().len()
        + part_catalog::Entity::find().all(db).await.unwrap().len()
        + entity::configuration::Entity::find()
            .all(db)
            .await
            .unwrap()
            .len()
        + configuration_part::Entity::find()
            .all(db)
            .await
            .unwrap()
            .len()
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("取込担当".to_owned()),
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, ドライランは書き換えない);
        全検証!(@one $用意, $属性, 解決できない参照はエラー);
        全検証!(@one $用意, $属性, 曖昧なスロット記述はエラー);
        全検証!(@one $用意, $属性, 取り込むとスロットが展開される);
        全検証!(@one $用意, $属性, アンカー参照が構成に結び付く);
        全検証!(@one $用意, $属性, 二度流しても結果が変わらない);
        全検証!(@one $用意, $属性, 既存のカタログを上書きしない);
        全検証!(@one $用意, $属性, 取込は行ごとの監査ログを書かない);
        全検証!(@one $用意, $属性, エラーがあれば何も反映しない);
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
