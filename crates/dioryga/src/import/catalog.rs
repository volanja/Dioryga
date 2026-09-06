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
use serde::Deserialize;
use std::collections::HashMap;

use super::{Entry, ImportError, Outcome, Report};
use crate::repository::{Actor, AuditedTx};

/// 対応するフォーマットの版。
const FORMAT_VERSION: u32 = 1;
const KIND: &str = "catalog";

// ---------------------------------------------------------------------------
// ファイルの構造
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CatalogFile {
    pub format_version: u32,
    pub kind: String,
    #[serde(default)]
    pub vendors: Vec<VendorInput>,
    #[serde(default)]
    pub chassis_models: Vec<ChassisModelInput>,
    #[serde(default)]
    pub part_catalogs: Vec<PartCatalogInput>,
    #[serde(default)]
    pub configurations: Vec<ConfigurationInput>,
}

#[derive(Debug, Deserialize)]
pub struct VendorInput {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct ChassisModelInput {
    pub vendor: String,
    pub model_name: String,
    pub device_category: String,
    #[serde(default)]
    pub height_u: i32,
    pub mount_form: String,
    #[serde(default)]
    pub rack_width: Option<String>,
    #[serde(default)]
    pub slots: Vec<SlotInput>,
}

/// スロットの展開記法（23.4）。
///
/// `count` ＋ `label_format` か、`labels` のいずれかを使う。
#[derive(Debug, Deserialize)]
pub struct SlotInput {
    pub slot_type: String,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub label_format: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct PartCatalogInput {
    pub vendor: String,
    pub part_number: String,
    pub category: String,
    #[serde(default)]
    pub core_count: Option<i32>,
    #[serde(default)]
    pub capacity_gb: Option<i32>,
    #[serde(default)]
    pub spec_json: Option<serde_json::Value>,
    /// ポート定義（8.3）。**スロットと同じ展開記法**（23.4）。
    #[serde(default)]
    pub ports: Vec<PortInput>,
}

/// ポートの展開記法（23.4）。`count` ＋ `label_format` か `labels`。
#[derive(Debug, Deserialize)]
pub struct PortInput {
    /// **閉じた語彙**（8.6）。Network / Power / Stack
    pub port_kind: String,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub label_format: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// **開いた語彙**（8.6）。正規化した自由入力として受ける
    pub connector_type: String,
    /// `port_kind=Network` のときだけ意味を持つ。
    #[serde(default)]
    pub port_speed: Option<String>,
    /// `port_kind=Power` のときだけ意味を持つ（12.7）。
    ///
    /// **展開したポートすべてが同じ定格を持つ**（23.4）。同じ型番のインレットが
    /// 2口あって片方だけ定格が違う、ということは起こらない。
    #[serde(default)]
    pub power_ratings: Vec<PowerRatingInput>,
}

#[derive(Debug, Deserialize)]
pub struct PowerRatingInput {
    /// **閉じた語彙**（8.6）。AC / DC
    pub current_type: String,
    /// **DCは負値をとる。**符号を含めたまま持ち、絶対値で比べない（12.7）。
    pub voltage_min: i32,
    pub voltage_max: i32,
}

#[derive(Debug, Deserialize)]
pub struct ConfigurationInput {
    pub chassis_model: ChassisModelRef,
    pub name: String,
    #[serde(default)]
    pub parts: Vec<ConfigurationPartInput>,
}

#[derive(Debug, Deserialize)]
pub struct ChassisModelRef {
    pub vendor: String,
    pub model_name: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfigurationPartInput {
    /// アンカーで部品定義を丸ごと参照できる（23.4）。自然キーだけを見る。
    pub part: PartCatalogRef,
    #[serde(default = "一つ")]
    pub quantity: i32,
}

fn 一つ() -> i32 {
    1
}

#[derive(Debug, Deserialize)]
pub struct PartCatalogRef {
    pub vendor: String,
    pub part_number: String,
}

// ---------------------------------------------------------------------------
// 展開
// ---------------------------------------------------------------------------

impl SlotInput {
    /// ラベルの一覧へ展開する。
    pub fn expand(&self) -> Result<Vec<String>, String> {
        展開(
            self.labels.as_deref(),
            self.count,
            self.label_format.as_deref(),
            &self.slot_type,
        )
    }
}

impl PortInput {
    /// ラベルの一覧へ展開する。**スロットと同じ記法**（23.4）。
    pub fn expand(&self) -> Result<Vec<String>, String> {
        展開(
            self.labels.as_deref(),
            self.count,
            self.label_format.as_deref(),
            &self.port_kind,
        )
    }
}

/// 展開記法の本体（23.4）。
///
/// `{n}` は1始まりの連番に置き換える。**0始まりにしない**のは、
/// 実機のラベル（DIMM1、Bay1、Port1）が1始まりであるため。
fn 展開(
    labels: Option<&[String]>,
    count: Option<u32>,
    label_format: Option<&str>,
    既定の接頭辞: &str,
) -> Result<Vec<String>, String> {
    match (labels, count) {
        (Some(labels), None) => {
            if labels.is_empty() {
                return Err("labels が空です".to_owned());
            }
            Ok(labels.to_vec())
        }
        (None, Some(count)) => {
            if count == 0 {
                return Err("count が 0 です".to_owned());
            }
            // 実機のスロット数として現実的な範囲を超えたら、記述の誤りを疑う
            if count > 1024 {
                return Err(format!("count が大きすぎます（{count}）"));
            }
            let format = label_format
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{既定の接頭辞}{{n}}"));
            Ok((1..=count)
                .map(|n| format.replace("{n}", &n.to_string()))
                .collect())
        }
        (Some(_), Some(_)) => Err("labels と count は同時に指定できません".to_owned()),
        (None, None) => Err("labels か count のどちらかが要ります".to_owned()),
    }
}

// ---------------------------------------------------------------------------
// ポートの検証（設計書8.3、8.6、12.7）
// ---------------------------------------------------------------------------

/// 閉じた語彙（8.6）。**リスト外は拒否する。**
const PORT_KINDS: &[&str] = &["Network", "Power", "Stack"];
const CURRENT_TYPES: &[&str] = &["AC", "DC"];

const NETWORK: &str = "Network";
const POWER: &str = "Power";

/// 検証を通ったポート1本ぶん。
struct 展開後のポート {
    port_kind: String,
    port_label: String,
    connector_type: String,
    port_speed: Option<String>,
    ratings: Vec<(String, i32, i32)>,
}

/// ポート定義を検証して展開する。
///
/// **エラーは取り込まない、警告は取り込むが記録する**（23.6）。ここで警告に
/// なるのは「値そのものは読めるが、その `port_kind` では意味を持たない」場合で、
/// **語彙外の値（拒否）とは別の話である。**
fn ポートを検証する(
    ports: &[PortInput],
) -> Result<(Vec<展開後のポート>, Vec<String>), String> {
    let mut 出力 = Vec::new();
    let mut 警告 = Vec::new();

    for p in ports {
        // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
        if !PORT_KINDS.contains(&p.port_kind.as_str()) {
            return Err(format!("port_kind「{}」は語彙にありません", p.port_kind));
        }
        let connector = crate::server::catalog::正規化(&p.connector_type);
        if connector.is_empty() {
            return Err(format!("{}: connector_type が空です", p.port_kind));
        }

        let labels = p.expand().map_err(|e| format!("{}: {e}", p.port_kind))?;

        // **意味を持たない列は捨てるが、黙って捨てない**（23.6、12.7）
        let port_speed = match p.port_speed.as_deref().map(crate::server::catalog::正規化) {
            Some(s) if s.is_empty() => None,
            Some(s) if p.port_kind == NETWORK => Some(s),
            Some(s) => {
                警告.push(format!(
                    "port_speed「{s}」は port_kind={} では意味を持たないため無視しました",
                    p.port_kind
                ));
                None
            }
            None => None,
        };

        let ratings = if p.power_ratings.is_empty() {
            Vec::new()
        } else if p.port_kind != POWER {
            警告.push(format!(
                "power_ratings は port_kind={} では意味を持たないため無視しました",
                p.port_kind
            ));
            Vec::new()
        } else {
            定格を検証する(&p.power_ratings)?
        };

        for label in labels {
            出力.push(展開後のポート {
                port_kind: p.port_kind.clone(),
                port_label: label,
                connector_type: connector.clone(),
                port_speed: port_speed.clone(),
                // **展開したポートすべてが同じ定格を持つ**（23.4）
                ratings: ratings.clone(),
            });
        }
    }

    Ok((出力, 警告))
}

fn 定格を検証する(ratings: &[PowerRatingInput]) -> Result<Vec<(String, i32, i32)>, String> {
    let mut 出力: Vec<(String, i32, i32)> = Vec::new();

    for r in ratings {
        if !CURRENT_TYPES.contains(&r.current_type.as_str()) {
            return Err(format!(
                "current_type「{}」は語彙にありません",
                r.current_type
            ));
        }
        // **絶対値ではなく符号を含めた大小で見る**（12.7）。DCは -72 が下限
        if r.voltage_min > r.voltage_max {
            return Err(format!(
                "{}: 電圧の下限（{}）が上限（{}）を超えています",
                r.current_type, r.voltage_min, r.voltage_max
            ));
        }
        // **方式ごとに1行**（12.7）。DBのUNIQUEに任せず、ここで理由を返す
        if 出力.iter().any(|(k, _, _)| *k == r.current_type) {
            return Err(format!(
                "current_type「{}」が重複しています",
                r.current_type
            ));
        }
        出力.push((r.current_type.clone(), r.voltage_min, r.voltage_max));
    }

    Ok(出力)
}

// ---------------------------------------------------------------------------
// 取込
// ---------------------------------------------------------------------------

/// 解析して形式を確かめる。
pub fn parse(source: &str) -> Result<CatalogFile, ImportError> {
    let file: CatalogFile = serde_yaml_ng::from_str(source)?;

    if file.format_version != FORMAT_VERSION {
        return Err(ImportError::UnsupportedVersion {
            found: file.format_version,
            expected: FORMAT_VERSION,
        });
    }
    if file.kind != KIND {
        return Err(ImportError::UnexpectedKind {
            found: file.kind.clone(),
            expected: KIND.to_owned(),
        });
    }

    Ok(file)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn スロット(yaml: &str) -> SlotInput {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn 連番へ展開される() {
        let s = スロット(r#"{ slot_type: DIMM, count: 4, label_format: "DIMM{n}" }"#);
        assert_eq!(s.expand().unwrap(), ["DIMM1", "DIMM2", "DIMM3", "DIMM4"]);
    }

    /// **1始まりであること。**実機のラベル（DIMM1、Bay1）に合わせる。
    #[test]
    fn 連番は1始まり() {
        let s = スロット(r#"{ slot_type: DIMM, count: 1, label_format: "DIMM{n}" }"#);
        assert_eq!(s.expand().unwrap(), ["DIMM1"]);
    }

    #[test]
    fn ラベルの明示列挙ができる() {
        let s = スロット(r#"{ slot_type: PCIE, labels: [Slot1, Slot9] }"#);
        assert_eq!(s.expand().unwrap(), ["Slot1", "Slot9"]);
    }

    #[test]
    fn label_formatが無ければslot_typeを使う() {
        let s = スロット("{ slot_type: PSU_BAY, count: 2 }");
        assert_eq!(s.expand().unwrap(), ["PSU_BAY1", "PSU_BAY2"]);
    }

    /// 両方指定・どちらも無しは記述の誤りとして弾く。
    #[test]
    fn 曖昧な指定は拒否される() {
        assert!(スロット(r#"{ slot_type: DIMM, count: 2, labels: [A, B] }"#)
            .expand()
            .is_err());
        assert!(スロット("{ slot_type: DIMM }").expand().is_err());
        assert!(スロット("{ slot_type: DIMM, count: 0 }").expand().is_err());
    }

    /// 桁を打ち間違えた場合に、64本のつもりが6400本入るのを防ぐ。
    #[test]
    fn 現実的でない本数は拒否される() {
        assert!(スロット("{ slot_type: DIMM, count: 5000 }")
            .expand()
            .is_err());
    }

    #[test]
    fn 版と種別が違えば読まない() {
        let 別の版 = "format_version: 99\nkind: catalog\n";
        assert!(matches!(
            parse(別の版),
            Err(ImportError::UnsupportedVersion { .. })
        ));

        let 別の種別 = "format_version: 1\nkind: instances\n";
        assert!(matches!(
            parse(別の種別),
            Err(ImportError::UnexpectedKind { .. })
        ));
    }

    /// YAMLのアンカーがパーサ側で解決されること（23.4）。
    #[test]
    fn アンカーで部品を参照できる() {
        let yaml = r#"
format_version: 1
kind: catalog
part_catalogs:
  - &dimm
    vendor: Cisco
    part_number: UCS-MRX64G2RE5
    category: Memory
    capacity_gb: 64
configurations:
  - chassis_model: { vendor: Fujitsu, model_name: RX4770 M8 }
    name: 標準構成
    parts:
      - { part: *dimm, quantity: 16 }
"#;
        let file = parse(yaml).unwrap();
        let part = &file.configurations[0].parts[0];
        assert_eq!(part.part.part_number, "UCS-MRX64G2RE5");
        assert_eq!(part.quantity, 16);
    }

    #[test]
    fn 数量の既定は1() {
        let yaml = r#"
format_version: 1
kind: catalog
configurations:
  - chassis_model: { vendor: V, model_name: M }
    name: c
    parts:
      - { part: { vendor: V, part_number: P } }
"#;
        let file = parse(yaml).unwrap();
        assert_eq!(file.configurations[0].parts[0].quantity, 1);
    }
}
