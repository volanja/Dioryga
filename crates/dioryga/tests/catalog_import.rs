//! カタログYAML取込の結合テスト（設計書23章）。

mod support;

use chrono::Utc;
use dioryga::import::{catalog, Outcome};
use entity::{
    app_user, chassis_model, chassis_slot, configuration_part, part_catalog, part_port_slot,
    port_power_rating, vendor, vlan,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};

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

/// **種別の略語は大文字だけを受けること**（設計書8.6、#125）。
///
/// 旧表記の `Vpn` を黙って `VPN` に直すと、取込ファイルの誤りに気付けない。
/// 語彙外の値と同じく拒否する。
async fn 種別の旧表記はエラー(db: &DatabaseConnection) {
    let yaml = |category: &str| {
        format!(
            r#"
format_version: 1
kind: catalog
vendors:
  - name: V
chassis_models:
  - vendor: V
    model_name: M1
    device_category: {category}
    mount_form: RackU
"#
        )
    };

    let file = catalog::parse(&yaml("Vpn")).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();
    assert!(
        report.errors().any(|e| e.detail.contains("「Vpn」")),
        "{report}"
    );

    let file = catalog::parse(&yaml("VPN")).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();
    assert!(!report.has_error(), "{report}");
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
// ポート（設計書8.3、23.4）
// ---------------------------------------------------------------------------

const ポートつき: &str = r#"
format_version: 1
kind: catalog
vendors:
  - name: Intel
  - name: Fujitsu
part_catalogs:
  - vendor: Intel
    part_number: E810-XXVDA2
    category: NIC
    ports:
      - { port_kind: Network, count: 2, label_format: "Port{n}", connector_type: SFP28ケージ, port_speed: 25G }
  - vendor: Fujitsu
    part_number: PYBPS1600
    category: PSU
    ports:
      - port_kind: Power
        count: 2
        label_format: "Inlet{n}"
        connector_type: "IEC C14"
        power_ratings:
          - { current_type: AC, voltage_min: 100, voltage_max: 240 }
          - { current_type: DC, voltage_min: 240, voltage_max: 240 }
"#;

/// **ポートがスロットと同じ記法で展開されること**（設計書23.4）。
async fn 取り込むとポートが展開される(db: &DatabaseConnection) {
    let user = 利用者(db, "ports@example.com").await;
    let file = catalog::parse(ポートつき).unwrap();
    catalog::apply(db, &file, user.id, 1).await.unwrap();

    let nic = 部品を引く(db, "E810-XXVDA2").await;
    let ports = ポート一覧(db, nic.id).await;
    assert_eq!(ports.len(), 2);
    // **1始まりであること。**実機のラベルに合わせる
    assert!(ports.iter().any(|p| p.port_label == "Port1"));
    assert!(ports.iter().any(|p| p.port_label == "Port2"));
    assert!(!ports.iter().any(|p| p.port_label == "Port0"));
    assert_eq!(ports[0].port_speed.as_deref(), Some("25G"));
}

/// **展開したポートすべてが同じ定格を持つこと**（設計書23.4）。
///
/// 同じ型番のインレットが2口あって片方だけ定格が違う、ということは起こらない。
/// **交流と直流の双方を持てること**も併せて確かめる（12.7）。
async fn 電源定格は展開したポートすべてに付く(db: &DatabaseConnection) {
    let user = 利用者(db, "ratings@example.com").await;
    catalog::apply(db, &catalog::parse(ポートつき).unwrap(), user.id, 1)
        .await
        .unwrap();

    let psu = 部品を引く(db, "PYBPS1600").await;
    let ports = ポート一覧(db, psu.id).await;
    assert_eq!(ports.len(), 2);

    for port in &ports {
        let ratings = 定格一覧(db, port.id).await;
        assert_eq!(
            ratings.len(),
            2,
            "{} に2方式が入っていない",
            port.port_label
        );
        let ac = ratings.iter().find(|r| r.current_type == "AC").unwrap();
        assert_eq!((ac.voltage_min, ac.voltage_max), (100, 240));
        assert!(ratings.iter().any(|r| r.current_type == "DC"));
    }
}

/// **閉じた語彙は既定へ寄せず拒否すること**（設計書8.6、Q-21）。
async fn 語彙外のポート種別はエラー(db: &DatabaseConnection) {
    let 誤り = ポートつき.replace("port_kind: Network", "port_kind: ネットワーク");
    let file = catalog::parse(&誤り).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();

    assert_eq!(report.count(Outcome::Error), 1);
    assert!(report
        .errors()
        .any(|e| e.detail.contains("語彙にありません")));

    // 反映は全体が止まる
    let user = 利用者(db, "badkind@example.com").await;
    assert!(catalog::apply(db, &file, user.id, 1).await.is_err());
    assert_eq!(ポート総数(db).await, 0);
}

/// **開いた語彙は語彙外でも取り込むこと**（設計書8.6）。
///
/// `connector_type` / `port_speed` を閉じると、**コネクタ形状も速度表記も
/// ベンダーと世代で増え続けるため「表に無いから取り込めない」が常態になる。**
/// 取り込めなかったものは台帳に載らない。
async fn 未知のコネクタでも取り込む(db: &DatabaseConnection) {
    let user = 利用者(db, "newconnector@example.com").await;
    let 新型 = ポートつき
        .replace("connector_type: SFP28ケージ", "connector_type: OSFP-XD")
        .replace("port_speed: 25G", "port_speed: 1.6T");
    let file = catalog::parse(&新型).unwrap();

    let report = catalog::dry_run(db, &file).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 0, "開いた語彙で拒否している");

    catalog::apply(db, &file, user.id, 1).await.unwrap();
    let ports = ポート一覧(db, 部品を引く(db, "E810-XXVDA2").await.id).await;
    assert_eq!(ports[0].connector_type, "OSFP-XD");
    assert_eq!(ports[0].port_speed.as_deref(), Some("1.6T"));
}

/// **意味を持たない列は取り込むが記録すること**（設計書23.6、12.7）。
///
/// 語彙外の値（拒否）とは別の話である。値そのものは読めるが、その `port_kind`
/// では意味を持たない。**ポート自体は作る。**
async fn 意味を持たない列は警告して捨てる(db: &DatabaseConnection) {
    let user = 利用者(db, "unused@example.com").await;
    // Power のポートに port_speed を書いてある
    let 誤り = ポートつき.replace(
        r#"connector_type: "IEC C14""#,
        r#"connector_type: "IEC C14"
        port_speed: 25G"#,
    );
    let file = catalog::parse(&誤り).unwrap();

    let report = catalog::dry_run(db, &file).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 0);
    assert_eq!(report.count(Outcome::Warning), 1);
    assert!(report
        .warnings()
        .any(|w| w.detail.contains("意味を持たない")));

    catalog::apply(db, &file, user.id, 1).await.unwrap();
    let ports = ポート一覧(db, 部品を引く(db, "PYBPS1600").await.id).await;
    assert_eq!(ports.len(), 2, "ポート自体は作られるべき");
    assert!(ports[0].port_speed.is_none(), "意味を持たない値が入った");
}

/// **DCの電圧を絶対値で比べないこと**（設計書12.7）。
///
/// Ciscoの`-48V`電源は許容範囲が`-40 to -72`。-72が下限、-40が上限になる。
async fn 直流の負電圧を受け付ける(db: &DatabaseConnection) {
    let user = 利用者(db, "dcimport@example.com").await;
    let dc = ポートつき.replace(
        "- { current_type: DC, voltage_min: 240, voltage_max: 240 }",
        "- { current_type: DC, voltage_min: -72, voltage_max: -40 }",
    );
    let file = catalog::parse(&dc).unwrap();

    let report = catalog::dry_run(db, &file).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 0, "負の範囲が拒否された");

    catalog::apply(db, &file, user.id, 1).await.unwrap();
    let port = ポート一覧(db, 部品を引く(db, "PYBPS1600").await.id)
        .await
        .remove(0);
    let dc = 定格一覧(db, port.id)
        .await
        .into_iter()
        .find(|r| r.current_type == "DC")
        .unwrap();
    assert_eq!((dc.voltage_min, dc.voltage_max), (-72, -40));
}

/// 上下が逆転していれば拒否すること（設計書12.7）。
async fn 逆転した電圧範囲はエラー(db: &DatabaseConnection) {
    let 誤り = ポートつき.replace(
        "- { current_type: AC, voltage_min: 100, voltage_max: 240 }",
        "- { current_type: AC, voltage_min: 240, voltage_max: 100 }",
    );
    let file = catalog::parse(&誤り).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();

    assert_eq!(report.count(Outcome::Error), 1);
    assert!(report.errors().any(|e| e.detail.contains("上限")));
}

/// **同じポートに同じ方式を2行書けないこと**（設計書12.7）。
async fn 方式の重複はエラー(db: &DatabaseConnection) {
    let 誤り = ポートつき.replace(
        "- { current_type: DC, voltage_min: 240, voltage_max: 240 }",
        "- { current_type: AC, voltage_min: 200, voltage_max: 200 }",
    );
    let file = catalog::parse(&誤り).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();

    assert_eq!(report.count(Outcome::Error), 1);
    assert!(report.errors().any(|e| e.detail.contains("重複")));
}

/// 上の `ポートつき` から `ports` を落としたもの。
const ポート無し: &str = r#"
format_version: 1
kind: catalog
vendors:
  - name: Intel
  - name: Fujitsu
part_catalogs:
  - { vendor: Intel,    part_number: E810-XXVDA2, category: NIC }
  - { vendor: Fujitsu,  part_number: PYBPS1600,   category: PSU }
"#;

/// **子行が1件も無い既存の部品には、ポートを後から足せること**（設計書23.4）。
///
/// 構成図パーサ（25章）は情報を段階的に供給する。「既存は触らない」をそのまま
/// 適用すると、**初回にポートを含めなかった部品は永久に空のまま**になる。
async fn ポートの無い既存部品には後から足せる(db: &DatabaseConnection) {
    let user = 利用者(db, "backfill@example.com").await;

    // 1回目：ports を書かずに取り込む
    catalog::apply(db, &catalog::parse(ポート無し).unwrap(), user.id, 1)
        .await
        .unwrap();
    assert_eq!(ポート総数(db).await, 0);

    // 2回目：同じ部品に ports を付けて流す
    let file = catalog::parse(ポートつき).unwrap();
    let report = catalog::dry_run(db, &file).await.unwrap();
    assert_eq!(report.count(Outcome::Updated), 2, "追加として出ていない");

    catalog::apply(db, &file, user.id, 2).await.unwrap();
    assert_eq!(ポート総数(db).await, 4);
}

/// **既にポートがある部品には触らないこと**（設計書18.2、23.4）。
///
/// 足すのは子行が0件のときだけであり、**上書きは起こらない。**
async fn ポートのある部品は上書きしない(db: &DatabaseConnection) {
    let user = 利用者(db, "nooverwrite@example.com").await;
    catalog::apply(db, &catalog::parse(ポートつき).unwrap(), user.id, 1)
        .await
        .unwrap();

    // ラベルもコネクタも変えたファイルを流す
    let 書き換え = ポートつき
        .replace(r#"label_format: "Port{n}""#, r#"label_format: "eth{n}""#)
        .replace("count: 2, label_format", "count: 4, label_format");
    let file = catalog::parse(&書き換え).unwrap();
    let report = catalog::apply(db, &file, user.id, 2).await.unwrap();

    assert_eq!(report.count(Outcome::Updated), 0);
    let ports = ポート一覧(db, 部品を引く(db, "E810-XXVDA2").await.id).await;
    assert_eq!(ports.len(), 2, "ポートが上書きされました");
    assert!(ports.iter().any(|p| p.port_label == "Port1"));
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 部品を引く(db: &DatabaseConnection, part_number: &str) -> part_catalog::Model {
    part_catalog::Entity::find()
        .filter(part_catalog::Column::PartNumber.eq(part_number))
        .one(db)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{part_number} が見つかりません"))
}

async fn ポート一覧(db: &DatabaseConnection, part_id: i32) -> Vec<part_port_slot::Model> {
    part_port_slot::Entity::find()
        .filter(part_port_slot::Column::PartCatalogId.eq(part_id))
        .order_by_asc(part_port_slot::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 定格一覧(db: &DatabaseConnection, port_id: i32) -> Vec<port_power_rating::Model> {
    port_power_rating::Entity::find()
        .filter(port_power_rating::Column::PartPortSlotId.eq(port_id))
        .all(db)
        .await
        .unwrap()
}

async fn ポート総数(db: &DatabaseConnection) -> usize {
    part_port_slot::Entity::find().all(db).await.unwrap().len()
}

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

// ---------------------------------------------------------------------------
// VLAN（設計書23.5、8.5）
// ---------------------------------------------------------------------------

const VLAN定義: &str = r#"
format_version: 1
kind: catalog
vlans:
  - { vlan_tag: 100, name: web, zone: DMZ, description: 公開系 }
"#;

/// **VLANをカタログYAMLから取り込めること**（23.5）。
///
/// VLANはプロジェクトを横断するマスタなので、インスタンスCSVではなく
/// カタログ側に置く。
async fn vlanを取り込める(db: &DatabaseConnection) {
    let user = 利用者(db, "vlan@example.com").await;
    let file = catalog::parse(VLAN定義).unwrap();

    let report = catalog::dry_run(db, &file).await.unwrap();
    assert_eq!(report.count(Outcome::Created), 1, "{report}");

    catalog::apply(db, &file, user.id, 1).await.unwrap();
    let v = vlan::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(v.vlan_tag, 100);
    assert_eq!(v.zone.as_deref(), Some("DMZ"));

    // 2回目は何も足さない（23.1）
    let report = catalog::apply(db, &file, user.id, 2).await.unwrap();
    assert_eq!(report.count(Outcome::Unchanged), 1, "{report}");
    assert_eq!(vlan::Entity::find().all(db).await.unwrap().len(), 1);
}

/// **同じタグの別名は禁じず、警告に留めること**（8.5、不変条件6）。
///
/// VLANタグはL2ドメインごとに独立しており、**拠点が違えば同じ100番が
/// 別物として存在する。**禁止すると複数拠点を1つの台帳で扱えなくなる。
async fn 同じタグの別名は警告(db: &DatabaseConnection) {
    let user = 利用者(db, "vlan-dup@example.com").await;
    catalog::apply(db, &catalog::parse(VLAN定義).unwrap(), user.id, 1)
        .await
        .unwrap();

    let 別拠点 = catalog::parse(
        r#"
format_version: 1
kind: catalog
vlans:
  - { vlan_tag: 100, name: 大阪web }
"#,
    )
    .unwrap();

    let report = catalog::dry_run(db, &別拠点).await.unwrap();
    assert_eq!(report.count(Outcome::Warning), 1, "{report}");
    assert!(!report.has_error(), "警告で止めてはなりません");

    catalog::apply(db, &別拠点, user.id, 2).await.unwrap();
    assert_eq!(vlan::Entity::find().all(db).await.unwrap().len(), 2);
}

/// **802.1Qの範囲外は拒否すること。**0と4095は予約されており実機に入らない。
async fn vlanタグの範囲外は拒否(db: &DatabaseConnection) {
    let file = catalog::parse(
        r#"
format_version: 1
kind: catalog
vlans:
  - { vlan_tag: 4095, name: よくない }
"#,
    )
    .unwrap();

    let report = catalog::dry_run(db, &file).await.unwrap();
    assert_eq!(report.count(Outcome::Error), 1, "{report}");
    assert_eq!(vlan::Entity::find().all(db).await.unwrap().len(), 0);
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("取込担当".to_owned()),
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

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, ドライランは書き換えない);
        全検証!(@one $用意, $属性, 解決できない参照はエラー);
        全検証!(@one $用意, $属性, 曖昧なスロット記述はエラー);
        全検証!(@one $用意, $属性, 種別の旧表記はエラー);
        全検証!(@one $用意, $属性, 取り込むとスロットが展開される);
        全検証!(@one $用意, $属性, アンカー参照が構成に結び付く);
        全検証!(@one $用意, $属性, 二度流しても結果が変わらない);
        全検証!(@one $用意, $属性, 既存のカタログを上書きしない);
        全検証!(@one $用意, $属性, 取込は行ごとの監査ログを書かない);
        全検証!(@one $用意, $属性, エラーがあれば何も反映しない);
        全検証!(@one $用意, $属性, 取り込むとポートが展開される);
        全検証!(@one $用意, $属性, 電源定格は展開したポートすべてに付く);
        全検証!(@one $用意, $属性, 語彙外のポート種別はエラー);
        全検証!(@one $用意, $属性, 未知のコネクタでも取り込む);
        全検証!(@one $用意, $属性, 意味を持たない列は警告して捨てる);
        全検証!(@one $用意, $属性, 直流の負電圧を受け付ける);
        全検証!(@one $用意, $属性, 逆転した電圧範囲はエラー);
        全検証!(@one $用意, $属性, 方式の重複はエラー);
        全検証!(@one $用意, $属性, ポートの無い既存部品には後から足せる);
        全検証!(@one $用意, $属性, ポートのある部品は上書きしない);
        全検証!(@one $用意, $属性, vlanを取り込める);
        全検証!(@one $用意, $属性, 同じタグの別名は警告);
        全検証!(@one $用意, $属性, vlanタグの範囲外は拒否);
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
