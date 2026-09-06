//! カタログYAMLの取込（設計書23.3、23.4）。
//!
//! # なぜカタログだけYAMLか
//!
//! `CHASSIS_MODEL` は「DIMM×64、ベイ×24、PCIe×10」という木構造であり、CSVでは
//! 表現が苦しい。逆に25万行の `PART_INSTANCE` をYAMLで書くのは非現実的である。
//! **量と構造が違うものに同じ形式を強いない**（23.3）。
//!
//! # 展開記法（23.4）
//!
//! 64個のDIMMスロットを手書きさせない。`count` ＋ `label_format` で機械的な
//! 連番、`labels` で明示列挙とする。
//!
//! ```yaml
//! slots:
//!   - { slot_type: DIMM, count: 64, label_format: "DIMM{n}" }
//!   - { slot_type: PCIE, labels: [Slot1, Slot2] }
//! ```
//!
//! YAMLのアンカー（`&dimm64` / `*dimm64`）はパーサが解決するため、こちらで
//! 扱う必要はない。
//!
//! # 参照済みカタログの更新は常にエラー（18.2、23.6）
//!
//! **黙って上書きさせない。**修正が必要な場合は新しいカタログ行を作る運用と
//! する。取り返しのつかない変更を未然に防ぐのが2段階取込の狙いである。
//!
//! # ただし、子行が0件なら足す（23.4）
//!
//! 構成図パーサ（25章）は情報を段階的に供給する。**初回の取込にポート定義が
//! 含まれていなかった部品は、「既存は触らない」をそのまま適用すると永久に
//! 空のまま**になり、手入力に頼ることになる——10,000台規模でそれは実質
//! 「入らない」を意味する（22.2）。
//!
//! **足すのは子行が1件も無いときだけ。**既にある定義には触れないため、
//! **上書きは起こらない。**18.2が禁じているのはスペックの*編集*であり、
//! 無かった定義が増えることは編集にあたらない。参照の有無は問わない。
//!
//! # 閉じた語彙と開いた語彙（8.6）
//!
//! 拒否するのは**閉じた語彙**——`port_kind` と `current_type`——だけである。
//! `connector_type` と `port_speed` は `vocabularies.md` でも末尾が `...` の
//! 開いた列挙であり、**閉じると「表に無いから取り込めない」が常態になる。**
//! コネクタ形状も速度表記もベンダーと世代で増え続けるためで、正規化した
//! 自由入力として受ける。

use chrono::Utc;
use entity::{
    chassis_model, chassis_slot, configuration, configuration_part, part_catalog, part_port_slot,
    port_power_rating, vendor,
};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set};
use std::collections::HashMap;

use super::{Entry, ImportError, Outcome, Report};
// **入力型と展開記法は別crateにある**（#77）。構成図パーサ（Krounos）が同じ型を
// 使うため、DBに触れない部分だけを切り出してある
use crate::repository::{Actor, AuditedTx};
use dioryga_catalog_format::ポートを検証する;
pub use dioryga_catalog_format::{
    parse, CatalogFile, ChassisModelInput, ChassisModelRef, ConfigurationInput,
    ConfigurationPartInput, PartCatalogInput, PartCatalogRef, PortInput, PowerRatingInput,
    SlotInput, VendorInput,
};

// ---------------------------------------------------------------------------
// 取込
// ---------------------------------------------------------------------------

/// 現在の事実と突き合わせ、差分レポートを作る（ドライラン、23.6）。
///
/// **DBを書き換えない。**呼び出し側はこのレポートを見てから [`apply`] を呼ぶ。
pub async fn dry_run<C: ConnectionTrait>(
    db: &C,
    file: &CatalogFile,
) -> Result<Report, ImportError> {
    let mut report = Report::default();

    for v in &file.vendors {
        let 既存 = 既存ベンダー(db, &v.name).await?;
        report.push(Entry::new(
            if 既存.is_some() {
                Outcome::Unchanged
            } else {
                Outcome::Created
            },
            format!("VENDOR {}", v.name),
            "",
        ));
    }

    // ファイル内で定義されたベンダーも参照先として認める。
    // **同じファイルで定義したものを参照できないと、初回取込が成立しない。**
    let 宣言済みベンダー: Vec<&str> = file.vendors.iter().map(|v| v.name.as_str()).collect();

    for m in &file.chassis_models {
        let target = format!("CHASSIS_MODEL {} {}", m.vendor, m.model_name);

        if !ベンダーを解決できる(db, &m.vendor, &宣言済みベンダー).await? {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!("ベンダー「{}」が見つかりません", m.vendor),
            ));
            continue;
        }

        // スロットの展開を先に確かめる。ここで落ちるのは記述の誤り
        let mut 展開エラー = None;
        let mut ラベル数 = 0usize;
        for slot in &m.slots {
            match slot.expand() {
                Ok(labels) => ラベル数 += labels.len(),
                Err(e) => {
                    展開エラー = Some(format!("{}: {e}", slot.slot_type));
                    break;
                }
            }
        }
        if let Some(e) = 展開エラー {
            report.push(Entry::new(Outcome::Error, target, e));
            continue;
        }

        match 既存モデル(db, &m.vendor, &m.model_name).await? {
            // **既存のスペックは上書きしない**（18.2）。ただし子行が1件も無ければ
            // 足す（23.4）——上書きではなく、無かった定義が増えるだけである
            Some(existing) => {
                let 既存の本数 = スロット数(db, existing.id).await?;
                if 既存の本数 == 0 && ラベル数 > 0 {
                    report.push(Entry::new(
                        Outcome::Updated,
                        target,
                        format!("スロット {ラベル数} 本を追加します"),
                    ));
                } else {
                    report.push(Entry::new(
                        Outcome::Unchanged,
                        target,
                        "既に登録されています（上書きしません）",
                    ));
                }
            }
            None => report.push(Entry::new(
                Outcome::Created,
                target,
                format!("スロット {ラベル数} 本"),
            )),
        }
    }

    for p in &file.part_catalogs {
        let target = format!("PART_CATALOG {} {}", p.vendor, p.part_number);
        if !ベンダーを解決できる(db, &p.vendor, &宣言済みベンダー).await? {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!("ベンダー「{}」が見つかりません", p.vendor),
            ));
            continue;
        }
        // ポートの展開と語彙をここで確かめる。落ちるのは記述の誤り
        let (ports, 警告) = match ポートを検証する(&p.ports) {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, e));
                continue;
            }
        };

        match 既存部品(db, &p.vendor, &p.part_number).await? {
            // 子行が1件も無ければ足す（23.4）
            Some(existing) => {
                let 既存の本数 = ポート数(db, existing.id).await?;
                if 既存の本数 == 0 && !ports.is_empty() {
                    report.push(Entry::new(
                        Outcome::Updated,
                        target.clone(),
                        format!("ポート {} 本を追加します", ports.len()),
                    ));
                } else {
                    report.push(Entry::new(
                        Outcome::Unchanged,
                        target.clone(),
                        "既に登録されています（上書きしません）",
                    ));
                }
            }
            None => report.push(Entry::new(
                Outcome::Created,
                target.clone(),
                if ports.is_empty() {
                    String::new()
                } else {
                    format!("ポート {} 本", ports.len())
                },
            )),
        }

        // **取り込むが記録する**（23.6）。ポート自体は作る
        for w in 警告 {
            report.push(Entry::new(Outcome::Warning, target.clone(), w));
        }
    }

    let 宣言済みモデル: Vec<(&str, &str)> = file
        .chassis_models
        .iter()
        .map(|m| (m.vendor.as_str(), m.model_name.as_str()))
        .collect();
    let 宣言済み部品: Vec<(&str, &str)> = file
        .part_catalogs
        .iter()
        .map(|p| (p.vendor.as_str(), p.part_number.as_str()))
        .collect();

    for c in &file.configurations {
        let target = format!(
            "CONFIGURATION {} {} / {}",
            c.chassis_model.vendor, c.chassis_model.model_name, c.name
        );

        let モデルあり =
            宣言済みモデル.contains(&(
                c.chassis_model.vendor.as_str(),
                c.chassis_model.model_name.as_str(),
            )) || 既存モデル(db, &c.chassis_model.vendor, &c.chassis_model.model_name)
                .await?
                .is_some();
        if !モデルあり {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "参照している筐体モデルが見つかりません",
            ));
            continue;
        }

        let mut 未解決 = Vec::new();
        for part in &c.parts {
            let ある = 宣言済み部品
                .contains(&(part.part.vendor.as_str(), part.part.part_number.as_str()))
                || 既存部品(db, &part.part.vendor, &part.part.part_number)
                    .await?
                    .is_some();
            if !ある {
                未解決.push(format!("{} {}", part.part.vendor, part.part.part_number));
            }
            if part.quantity < 1 {
                未解決.push(format!(
                    "{} の数量が {}",
                    part.part.part_number, part.quantity
                ));
            }
        }
        if !未解決.is_empty() {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!("解決できない部品: {}", 未解決.join(", ")),
            ));
            continue;
        }

        report.push(Entry::new(
            if 既存構成(db, &c.chassis_model, &c.name).await?.is_some() {
                Outcome::Unchanged
            } else {
                Outcome::Created
            },
            target,
            format!("部品 {} 種", c.parts.len()),
        ));
    }

    Ok(report)
}

/// 差分を反映する（設計書23.1）。
///
/// **既存のものは触らない。**宣言的な取込であり、同じファイルを何度流しても
/// 結果が変わらないことが要点である。18.2により参照済みカタログは編集できず、
/// 参照の有無を取込のたびに調べるより「既存は常に据え置く」ほうが単純で安全。
pub async fn apply(
    db: &sea_orm::DatabaseConnection,
    file: &CatalogFile,
    actor: i32,
    import_run_id: i32,
) -> Result<Report, ImportError> {
    let report = dry_run(db, file).await?;
    if report.has_error() {
        return Err(ImportError::HasErrors(report.count(Outcome::Error)));
    }

    let now = Utc::now();
    // **行ごとの監査ログを書かない**（24.4）。追跡は IMPORT_RUN が担う
    let tx = AuditedTx::begin(db, Actor::Import { import_run_id }).await?;

    let mut vendor_ids: HashMap<String, i32> = HashMap::new();
    for v in &file.vendors {
        let id = match 既存ベンダー(tx.reader(), &v.name).await? {
            Some(existing) => existing.id,
            None => {
                tx.insert(vendor::ActiveModel {
                    name: Set(v.name.clone()),
                    created_by: Set(actor),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?
                .id
            }
        };
        vendor_ids.insert(v.name.clone(), id);
    }

    // ファイルで宣言されていないベンダーは既存から引く
    let ベンダーid = |name: &str| -> Option<i32> { vendor_ids.get(name).copied() };

    for m in &file.chassis_models {
        // **既存でも子行が0件なら足す**（23.4）。上書きは起こらない
        if let Some(existing) = 既存モデル(tx.reader(), &m.vendor, &m.model_name).await? {
            if スロット数(tx.reader(), existing.id).await? == 0 {
                スロットを足す(&tx, existing.id, &m.slots, now).await?;
            }
            continue;
        }
        let vendor_id = match ベンダーid(&m.vendor) {
            Some(id) => id,
            None => {
                既存ベンダー(tx.reader(), &m.vendor)
                    .await?
                    .expect("ドライランで解決済み")
                    .id
            }
        };

        let model = tx
            .insert(chassis_model::ActiveModel {
                vendor_id: Set(vendor_id),
                model_name: Set(m.model_name.clone()),
                device_category: Set(m.device_category.clone()),
                height_u: Set(m.height_u),
                mount_form: Set(m.mount_form.clone()),
                rack_width: Set(m.rack_width.clone()),
                created_by: Set(actor),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;

        スロットを足す(&tx, model.id, &m.slots, now).await?;
    }

    for p in &file.part_catalogs {
        // **既存でも子行が0件なら足す**（23.4）
        if let Some(existing) = 既存部品(tx.reader(), &p.vendor, &p.part_number).await? {
            if ポート数(tx.reader(), existing.id).await? == 0 {
                ポートを足す(&tx, existing.id, &p.ports, now).await?;
            }
            continue;
        }
        let vendor_id = match ベンダーid(&p.vendor) {
            Some(id) => id,
            None => {
                既存ベンダー(tx.reader(), &p.vendor)
                    .await?
                    .expect("ドライランで解決済み")
                    .id
            }
        };

        let part = tx
            .insert(part_catalog::ActiveModel {
                category: Set(p.category.clone()),
                vendor_id: Set(vendor_id),
                part_number: Set(p.part_number.clone()),
                core_count: Set(p.core_count),
                capacity_gb: Set(p.capacity_gb),
                // JSONは文字列で持つ（24.2.3）
                spec_json: Set(p
                    .spec_json
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_owned())),
                created_by: Set(actor),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;

        ポートを足す(&tx, part.id, &p.ports, now).await?;
    }

    for c in &file.configurations {
        if 既存構成(tx.reader(), &c.chassis_model, &c.name)
            .await?
            .is_some()
        {
            continue;
        }
        let model = 既存モデル(
            tx.reader(),
            &c.chassis_model.vendor,
            &c.chassis_model.model_name,
        )
        .await?
        .expect("ドライランで解決済み");

        let config = tx
            .insert(configuration::ActiveModel {
                chassis_model_id: Set(model.id),
                name: Set(c.name.clone()),
                created_by: Set(actor),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;

        for part in &c.parts {
            let catalog = 既存部品(tx.reader(), &part.part.vendor, &part.part.part_number)
                .await?
                .expect("ドライランで解決済み");
            tx.insert(configuration_part::ActiveModel {
                configuration_id: Set(config.id),
                part_catalog_id: Set(catalog.id),
                quantity: Set(part.quantity),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;
        }
    }

    tx.commit().await?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// 自然キーによる解決（設計書23.2）
// ---------------------------------------------------------------------------

/// スロットを書き込む。**呼ぶ側が「子行が0件か」を確かめている**（23.4）。
async fn スロットを足す(
    tx: &AuditedTx,
    chassis_model_id: i32,
    slots: &[SlotInput],
    now: chrono::DateTime<Utc>,
) -> Result<(), ImportError> {
    for slot in slots {
        for label in slot.expand().expect("ドライランで検証済み") {
            tx.insert(chassis_slot::ActiveModel {
                chassis_model_id: Set(chassis_model_id),
                slot_type: Set(slot.slot_type.clone()),
                slot_label: Set(label),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;
        }
    }
    Ok(())
}

/// ポートと電源定格を書き込む（8.3、12.7）。
///
/// **意味を持たない列はドライランで落としてある**（23.6）。ここでは書くだけ。
async fn ポートを足す(
    tx: &AuditedTx,
    part_catalog_id: i32,
    ports: &[PortInput],
    now: chrono::DateTime<Utc>,
) -> Result<(), ImportError> {
    let (展開済み, _) = ポートを検証する(ports).expect("ドライランで検証済み");

    for port in 展開済み {
        let slot = tx
            .insert(part_port_slot::ActiveModel {
                part_catalog_id: Set(part_catalog_id),
                port_kind: Set(port.port_kind),
                port_label: Set(port.port_label),
                connector_type: Set(port.connector_type),
                port_speed: Set(port.port_speed),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;

        for (current_type, min, max) in port.ratings {
            tx.insert(port_power_rating::ActiveModel {
                part_port_slot_id: Set(slot.id),
                current_type: Set(current_type),
                voltage_min: Set(min),
                voltage_max: Set(max),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await?;
        }
    }
    Ok(())
}

async fn スロット数<C: ConnectionTrait>(
    db: &C,
    chassis_model_id: i32,
) -> Result<usize, DbErr> {
    Ok(chassis_slot::Entity::find()
        .filter(chassis_slot::Column::ChassisModelId.eq(chassis_model_id))
        .all(db)
        .await?
        .len())
}

async fn ポート数<C: ConnectionTrait>(db: &C, part_catalog_id: i32) -> Result<usize, DbErr> {
    Ok(part_port_slot::Entity::find()
        .filter(part_port_slot::Column::PartCatalogId.eq(part_catalog_id))
        .all(db)
        .await?
        .len())
}

async fn 既存ベンダー<C: ConnectionTrait>(
    db: &C,
    name: &str,
) -> Result<Option<vendor::Model>, sea_orm::DbErr> {
    vendor::Entity::find()
        .filter(vendor::Column::Name.eq(name))
        .one(db)
        .await
}

async fn ベンダーを解決できる<C: ConnectionTrait>(
    db: &C,
    name: &str,
    宣言済み: &[&str],
) -> Result<bool, sea_orm::DbErr> {
    if 宣言済み.contains(&name) {
        return Ok(true);
    }
    Ok(既存ベンダー(db, name).await?.is_some())
}

async fn 既存モデル<C: ConnectionTrait>(
    db: &C,
    vendor_name: &str,
    model_name: &str,
) -> Result<Option<chassis_model::Model>, sea_orm::DbErr> {
    let Some(v) = 既存ベンダー(db, vendor_name).await? else {
        return Ok(None);
    };
    chassis_model::Entity::find()
        .filter(chassis_model::Column::VendorId.eq(v.id))
        .filter(chassis_model::Column::ModelName.eq(model_name))
        .one(db)
        .await
}

async fn 既存部品<C: ConnectionTrait>(
    db: &C,
    vendor_name: &str,
    part_number: &str,
) -> Result<Option<part_catalog::Model>, sea_orm::DbErr> {
    let Some(v) = 既存ベンダー(db, vendor_name).await? else {
        return Ok(None);
    };
    part_catalog::Entity::find()
        .filter(part_catalog::Column::VendorId.eq(v.id))
        .filter(part_catalog::Column::PartNumber.eq(part_number))
        .one(db)
        .await
}

async fn 既存構成<C: ConnectionTrait>(
    db: &C,
    model_ref: &ChassisModelRef,
    name: &str,
) -> Result<Option<configuration::Model>, sea_orm::DbErr> {
    let Some(m) = 既存モデル(db, &model_ref.vendor, &model_ref.model_name).await? else {
        return Ok(None);
    };
    configuration::Entity::find()
        .filter(configuration::Column::ChassisModelId.eq(m.id))
        .filter(configuration::Column::Name.eq(name))
        .one(db)
        .await
}
