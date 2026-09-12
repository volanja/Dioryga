//! インスタンスCSVの取込（設計書23.2、23.5、23.6、23.9）。
//!
//! # 突合の解決順序（23.2）
//!
//! ```text
//! ① uid が指定されていればそれで特定（export → 編集 → import の往復）
//! ② external_id が一致するものを探す
//! ③ マニフェストの match_on で指定された列で探す
//! ④ いずれも該当しなければ新規作成し、uid を採番する
//! ```
//!
//! **④へ落ちる前に重複を疑う**（23.9.1）。`serial_number`・`asset_number`・
//! `external_id`・`hostname` のいずれかが既存機器と一致するのに突合できなかった
//! 場合は、**新規作成せずエラーとする。**黙って2台目を作らないことが要点である。
//!
//! # 統合された機器を辿る（23.9.4）
//!
//! 吸収された側の `uid`・`external_id`・`serial_number` で取り込んでも、
//! `merged_into_device_id` を辿って統合先に着地する。**統合後も従来の取込
//! ファイルがそのまま使い続けられる**、という点がこの方式の実利である。
//!
//! # 1ファイルは1プロジェクトに閉じる（23.5）
//!
//! 複数プロジェクトにまたがると認可の判定が曖昧になる（3章）。突合も重複検出も
//! **このプロジェクトに属する（属したことがある）機器の中だけ**で行う。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use entity::{chassis_model, configuration, device, device_assignment, project, vendor};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use super::placement::機器の索引;
use super::{Entry, ImportError, Outcome, Report};
use crate::repository::{Actor, AuditedTx};

const FORMAT_VERSION: u32 = 1;
const KIND: &str = "instances";
const PROJECT: &str = "Project";

// ---------------------------------------------------------------------------
// マニフェスト（設計書23.5）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub kind: String,
    /// `uid` → `code` → `name` の順で解決する（5.3）。
    pub project: String,
    /// 履歴行の `from_date` に使う基準時刻（23.1）。省略時は実行時刻。
    #[serde(default)]
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default)]
    pub match_on: MatchOn,
    #[serde(default)]
    pub files: Vec<FileRef>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MatchOn {
    /// `uid` / `external_id` が無い場合の突合キー。
    #[serde(default)]
    pub device: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileRef {
    pub entity: String,
    pub path: String,
}

pub fn parse_manifest(source: &str) -> Result<Manifest, ImportError> {
    let manifest: Manifest = serde_yaml_ng::from_str(source)?;

    if manifest.format_version != FORMAT_VERSION {
        return Err(ImportError::UnsupportedVersion {
            found: manifest.format_version,
            expected: FORMAT_VERSION,
        });
    }
    if manifest.kind != KIND {
        return Err(ImportError::UnexpectedKind {
            found: manifest.kind.clone(),
            expected: KIND.to_owned(),
        });
    }

    Ok(manifest)
}

// ---------------------------------------------------------------------------
// 機器のCSV（設計書23.5）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceRow {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub external_id: String,
    pub hostname: String,
    #[serde(default)]
    pub serial_number: String,
    #[serde(default)]
    pub asset_number: String,
    #[serde(default)]
    pub device_type: String,
    #[serde(default)]
    pub configuration_vendor: String,
    /// 省略できる。**省略時はベンダーと構成名で解決し、複数該当ならエラー**（23.5）。
    #[serde(default)]
    pub configuration_model: String,
    #[serde(default)]
    pub configuration_name: String,
    #[serde(default)]
    pub device_category: String,
    #[serde(default)]
    pub power_watt: String,
    #[serde(default)]
    pub status: String,
}

impl DeviceRow {
    fn 空ならnone(value: &str) -> Option<String> {
        match value.trim() {
            "" => None,
            v => Some(v.to_owned()),
        }
    }

    /// 行を人が読める形で示す。**利用者がファイルの中の行を特定できる**ように、
    /// 空でない識別子を優先して出す。
    fn 表示名(&self) -> String {
        for candidate in [&self.uid, &self.external_id, &self.serial_number] {
            if !candidate.trim().is_empty() {
                return format!("{} ({})", self.hostname, candidate.trim());
            }
        }
        self.hostname.clone()
    }
}

pub fn parse_devices(source: &str) -> Result<Vec<DeviceRow>, ImportError> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(source.as_bytes());

    let mut rows = Vec::new();
    for record in reader.deserialize() {
        rows.push(record.map_err(|e| ImportError::Csv(e.to_string()))?);
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// 突合（設計書23.2、23.9.1）
// ---------------------------------------------------------------------------

/// 突合の結果。
#[derive(Debug)]
enum Matched {
    /// 既存の機器に当たった。
    Existing(Box<device::Model>),
    /// 新規に作る。
    New,
}

/// 1行を既存の機器に突き合わせる。
///
/// 解決順序は23.2の通り。**④へ落ちる前に23.9.1の重複検査を行う。**
///
/// **候補は索引で引く**（#111）。以前は行ごとに候補を読み直したうえで線形走査
/// しており、10,000台の再取込に267秒かかっていた（#22の実測）。
async fn 突合<C: ConnectionTrait>(
    db: &C,
    索引: &機器の索引,
    row: &DeviceRow,
    match_on: &[String],
) -> Result<Result<Matched, String>, sea_orm::DbErr> {
    // ① uid
    //
    // **ここだけはプロジェクトの外も探す。**書き出したファイルを編集して戻す
    // 運用では、移管された機器の uid が書かれていることがある
    if let Some(uid) = DeviceRow::空ならnone(&row.uid) {
        let found = device::Entity::find()
            .filter(device::Column::Uid.eq(uid.as_str()))
            .one(db)
            .await?;
        return Ok(match found {
            // **統合先を辿る**（23.9.4）
            Some(d) => Ok(Matched::Existing(Box::new(統合先を辿る(db, d).await?))),
            None => Err(format!("uid「{uid}」の機器が見つかりません")),
        });
    }

    // ② external_id
    if let Some(external_id) = DeviceRow::空ならnone(&row.external_id) {
        let 一致 = 索引.該当("external_id", &external_id);
        if 一致.len() == 1 {
            if let Some(d) = 索引.機器(一致[0]) {
                return Ok(Ok(Matched::Existing(Box::new(d))));
            }
        }
    }

    // ③ match_on
    for key in match_on {
        let Some(value) = 行の値(row, key) else {
            return Ok(Err(format!("match_on の列「{key}」を解釈できません")));
        };
        let Some(value) = DeviceRow::空ならnone(&value) else {
            continue;
        };
        let 一致 = 索引.該当(key, &value);

        match 一致.len() {
            0 => {}
            1 => {
                return Ok(match 索引.機器(一致[0]) {
                    Some(d) => Ok(Matched::Existing(Box::new(d))),
                    None => Err(format!("{key}「{value}」の機器を読み出せません")),
                })
            }
            n => {
                return Ok(Err(format!(
                    "{key}「{value}」に{n}台が該当します。突合できません"
                )))
            }
        }
    }

    // ④ 新規作成の前に、重複を疑う（23.9.1）
    if let Some(理由) = 重複の疑い(索引, row) {
        return Ok(Err(理由));
    }

    Ok(Ok(Matched::New))
}

/// **黙って2台目を作らない**（設計書23.9.1）。
///
/// 突合できなかったのに、識別子のいずれかが既存機器と一致する場合はエラーとする。
/// 利用者はドライランの指摘を見て、`uid` を書く／`external_id` を設定する／
/// 別機器であることを確認する、のいずれかを選べる。
fn 重複の疑い(索引: &機器の索引, row: &DeviceRow) -> Option<String> {
    let 検査 = [
        ("serial_number", DeviceRow::空ならnone(&row.serial_number)),
        ("asset_number", DeviceRow::空ならnone(&row.asset_number)),
        ("external_id", DeviceRow::空ならnone(&row.external_id)),
        ("hostname", DeviceRow::空ならnone(&row.hostname)),
    ];

    for (label, value) in 検査 {
        let Some(value) = value else { continue };
        let Some(&id) = 索引.該当(label, &value).first() else {
            continue;
        };
        let 既存 = 索引.機器(id)?;
        return Some(format!(
            "{label}「{value}」が既存の機器（{}）と一致しますが、突合できませんでした。\
             同一の機器なら uid か external_id を指定してください",
            既存.hostname
        ));
    }
    None
}

/// 統合された機器を辿る（設計書23.9.4）。
///
/// 吸収された側で取り込んでも統合先に着地する。**統合後も従来の取込ファイルが
/// そのまま使える**というのが、削除ではなくリダイレクトにした実利である。
async fn 統合先を辿る<C: ConnectionTrait>(
    db: &C,
    mut d: device::Model,
) -> Result<device::Model, sea_orm::DbErr> {
    // 統合の連鎖は想定しないが、万一の循環で止まらなくならないよう上限を置く
    for _ in 0..8 {
        let Some(next) = d.merged_into_device_id else {
            return Ok(d);
        };
        match device::Entity::find_by_id(next).one(db).await? {
            Some(found) => d = found,
            None => return Ok(d),
        }
    }
    Ok(d)
}

fn 行の値(row: &DeviceRow, key: &str) -> Option<String> {
    match key {
        "serial_number" => Some(row.serial_number.clone()),
        "asset_number" => Some(row.asset_number.clone()),
        "hostname" => Some(row.hostname.clone()),
        "external_id" => Some(row.external_id.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 参照の解決
// ---------------------------------------------------------------------------

/// プロジェクトを解決する。`uid` → `code` → `name` の順（設計書5.3）。
///
/// **`name` で複数該当する場合はエラー。**同名の案件が年度違いで存在しうる。
pub async fn 解決するプロジェクト<C: ConnectionTrait>(
    db: &C,
    key: &str,
) -> Result<Result<project::Model, String>, sea_orm::DbErr> {
    if let Some(p) = project::Entity::find()
        .filter(project::Column::Uid.eq(key))
        .one(db)
        .await?
    {
        return Ok(Ok(p));
    }
    if let Some(p) = project::Entity::find()
        .filter(project::Column::Code.eq(key))
        .one(db)
        .await?
    {
        return Ok(Ok(p));
    }

    let 同名 = project::Entity::find()
        .filter(project::Column::Name.eq(key))
        .all(db)
        .await?;

    Ok(match 同名.len() {
        1 => Ok(同名.into_iter().next().expect("1件あることを確認済み")),
        0 => Err(format!("プロジェクト「{key}」が見つかりません")),
        n => Err(format!(
            "プロジェクト名「{key}」に{n}件が該当します。uid か code で指定してください"
        )),
    })
}

/// 構成を解決する（設計書23.5）。
///
/// `configuration_model` を省略した場合は「ベンダー＋構成名」で解決し、
/// **複数該当すればエラー**とする。同じベンダーの別モデルに同名の構成がありうる。
async fn 解決する構成<C: ConnectionTrait>(
    db: &C,
    row: &DeviceRow,
) -> Result<Result<Option<i32>, String>, sea_orm::DbErr> {
    let (Some(vendor_name), Some(config_name)) = (
        DeviceRow::空ならnone(&row.configuration_vendor),
        DeviceRow::空ならnone(&row.configuration_name),
    ) else {
        // 構成を持たない機器（Virtual / Container / Logical）がある
        return Ok(Ok(None));
    };

    let Some(v) = vendor::Entity::find()
        .filter(vendor::Column::Name.eq(vendor_name.as_str()))
        .one(db)
        .await?
    else {
        return Ok(Err(format!("ベンダー「{vendor_name}」が見つかりません")));
    };

    let mut models = chassis_model::Entity::find().filter(chassis_model::Column::VendorId.eq(v.id));
    if let Some(model_name) = DeviceRow::空ならnone(&row.configuration_model) {
        models = models.filter(chassis_model::Column::ModelName.eq(model_name));
    }
    let model_ids: Vec<i32> = models.all(db).await?.into_iter().map(|m| m.id).collect();

    if model_ids.is_empty() {
        return Ok(Err(format!(
            "ベンダー「{vendor_name}」の筐体モデルが見つかりません"
        )));
    }

    let 該当 = configuration::Entity::find()
        .filter(configuration::Column::ChassisModelId.is_in(model_ids))
        .filter(configuration::Column::Name.eq(config_name.as_str()))
        .all(db)
        .await?;

    Ok(match 該当.len() {
        1 => Ok(Some(該当[0].id)),
        0 => Err(format!(
            "構成「{vendor_name} / {config_name}」が見つかりません"
        )),
        n => Err(format!(
            "構成「{vendor_name} / {config_name}」に{n}件が該当します。\
             configuration_model を指定してください"
        )),
    })
}

// ---------------------------------------------------------------------------
// ドライラン（設計書23.6）
// ---------------------------------------------------------------------------

/// 1行の反映内容。ドライランで組み立て、そのまま反映に使う。
struct Planned {
    row: DeviceRow,
    existing: Option<device::Model>,
    configuration_id: Option<i32>,
    device_type: String,
    status: String,
    power_watt: i32,
    /// このプロジェクトへの割当を作る必要があるか。
    needs_assignment: bool,
}

const DEVICE_TYPES: &[&str] = &["Physical", "Virtual", "Container", "Logical"];
/// **部品（`PART_INSTANCE`）も同じ語彙を持つ**ため、`parts.rs` と共有する。
pub(super) const STATUSES: &[&str] = &["running", "broken", "repair", "plan", "building"];

/// 閉じた語彙で検証する（8.6）。**語彙外は既定へ寄せず拒否し、空欄は既定にする。**
pub(super) fn 語彙(
    value: &str,
    allowed: &[&'static str],
    default: &'static str,
) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(default.to_owned());
    }
    allowed
        .iter()
        .find(|v| **v == value)
        .map(|v| (*v).to_owned())
        .ok_or_else(|| format!("「{value}」は使えません（{})", allowed.join(" / ")))
}

/// 現在の事実と突き合わせ、差分レポートと反映計画を作る。
async fn 計画する<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    rows: &[DeviceRow],
    match_on: &[String],
) -> Result<(Report, Vec<Planned>), ImportError> {
    let mut report = Report::default();
    let mut planned = Vec::new();

    // **索引は取込のはじめに1度だけ作る**（#111）。行ごとに候補を読み直すと、
    // 行数×台数で伸びる
    let 索引 = 機器の索引::作る(db, project_id).await?;

    // **ファイル内の重複を先に見る。**DBと突き合わせる前に弾かないと、
    // 同じ機器を2回作るか、2行目が1行目を上書きする
    let mut 出現済み: HashMap<String, usize> = HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let target = row.表示名();

        if row.hostname.trim().is_empty() {
            report.push(Entry::new(Outcome::Error, target, "hostname が空です"));
            continue;
        }

        for key in ["uid", "external_id", "serial_number"] {
            let Some(value) = 行の値(row, key).or_else(|| Some(row.uid.clone())) else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            let 鍵 = format!("{key}={value}");
            if let Some(前) = 出現済み.insert(鍵, i) {
                report.push(Entry::new(
                    Outcome::Error,
                    target.clone(),
                    format!("{key}「{value}」が{}行目と重複しています", 前 + 2),
                ));
            }
        }

        let device_type = match 語彙(&row.device_type, DEVICE_TYPES, "Physical") {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!("device_type: {e}"),
                ));
                continue;
            }
        };
        let status = match 語彙(&row.status, STATUSES, "building") {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, format!("status: {e}")));
                continue;
            }
        };
        let power_watt = match row.power_watt.trim() {
            "" => 0,
            v => match v.parse::<i32>() {
                Ok(n) if n >= 0 => n,
                _ => {
                    report.push(Entry::new(
                        Outcome::Error,
                        target,
                        format!("power_watt「{v}」は0以上の整数ではありません"),
                    ));
                    continue;
                }
            },
        };

        let configuration_id = match 解決する構成(db, row).await? {
            Ok(v) => v,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, e));
                continue;
            }
        };

        let matched = match 突合(db, &索引, row, match_on).await? {
            Ok(m) => m,
            Err(e) => {
                report.push(Entry::new(Outcome::Error, target, e));
                continue;
            }
        };

        let existing = match matched {
            Matched::Existing(d) => Some(*d),
            Matched::New => None,
        };

        let 現在ここにいる = match &existing {
            Some(d) => 現在の割当(db, d.id)
                .await?
                .is_some_and(|a| a.location_type == PROJECT && a.location_id == Some(project_id)),
            None => false,
        };

        let 変更あり = match &existing {
            None => true,
            Some(d) => {
                d.hostname != row.hostname.trim()
                    || d.serial_number != DeviceRow::空ならnone(&row.serial_number)
                    || d.asset_number != DeviceRow::空ならnone(&row.asset_number)
                    || d.external_id != DeviceRow::空ならnone(&row.external_id)
                    || d.device_type != device_type
                    || d.status != status
                    || d.power_watt != power_watt
                    || d.configuration_id != configuration_id
                    || d.device_category != DeviceRow::空ならnone(&row.device_category)
            }
        };

        let outcome = match (&existing, 変更あり || !現在ここにいる) {
            (None, _) => Outcome::Created,
            (Some(_), true) => Outcome::Updated,
            // **現在の事実と一致していれば履歴行を作らない**（23.1）
            (Some(_), false) => Outcome::Unchanged,
        };

        report.push(Entry::new(
            outcome,
            target,
            match &existing {
                Some(d) if outcome != Outcome::Unchanged => format!("既存 uid={}", d.uid),
                _ => String::new(),
            },
        ));

        planned.push(Planned {
            row: row.clone(),
            existing,
            configuration_id,
            device_type,
            status,
            power_watt,
            needs_assignment: !現在ここにいる,
        });
    }

    Ok((report, planned))
}

async fn 現在の割当<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> Result<Option<device_assignment::Model>, sea_orm::DbErr> {
    device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
}

/// ドライラン。**DBを書き換えない。**
pub async fn dry_run(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[DeviceRow],
    match_on: &[String],
) -> Result<Report, ImportError> {
    Ok(計画する(db, project_id, rows, match_on).await?.0)
}

// ---------------------------------------------------------------------------
// 反映（設計書23.1）
// ---------------------------------------------------------------------------

/// 差分を反映する。
///
/// **履歴行の `from_date` は `as_of` を使う**（23.1）。過去のデータを遡って
/// 登録する場合にマニフェストで指定できる。
/// 同じトランザクションの中で計画し、書き込む（23.6）。
///
/// **エラーの行は書かず、正しい行だけを書く。**取込全体の判定は呼び出し側が
/// 行い、1件でもエラーがあればトランザクションごと捨てる。正しい行を書いて
/// おくのは、**後のエンティティがそれを参照できるようにするため**である。
pub async fn 取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[DeviceRow],
    match_on: &[String],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    // **トランザクションの中で読む。**同じ取込で先に書いたもの（機器など）が
    // 見えるのはこの経路だけであり、SQLiteのインメモリでは外の接続で読むと
    // 止まる（24.2.5）
    let (report, planned) = 計画する(tx.reader(), project_id, rows, match_on).await?;
    let now = Utc::now();

    for p in planned {
        let device_id = match &p.existing {
            Some(existing) => {
                let mut active: device::ActiveModel = existing.clone().into();
                active.hostname = Set(p.row.hostname.trim().to_owned());
                active.external_id = Set(DeviceRow::空ならnone(&p.row.external_id));
                active.serial_number = Set(DeviceRow::空ならnone(&p.row.serial_number));
                active.asset_number = Set(DeviceRow::空ならnone(&p.row.asset_number));
                active.device_type = Set(p.device_type.clone());
                active.device_category = Set(DeviceRow::空ならnone(&p.row.device_category));
                active.configuration_id = Set(p.configuration_id);
                active.power_watt = Set(p.power_watt);
                active.status = Set(p.status.clone());
                active.updated_at = Set(now);
                tx.update(existing, active).await?;
                existing.id
            }
            None => {
                let uid = match DeviceRow::空ならnone(&p.row.uid) {
                    Some(uid) => uid,
                    // **④で uid を採番する**（23.2）。書き出せば往復できる
                    None => uuid::Uuid::new_v4().to_string(),
                };
                tx.insert(device::ActiveModel {
                    uid: Set(uid),
                    external_id: Set(DeviceRow::空ならnone(&p.row.external_id)),
                    merged_into_device_id: Set(None),
                    merged_at: Set(None),
                    configuration_id: Set(p.configuration_id),
                    device_type: Set(p.device_type.clone()),
                    device_category: Set(DeviceRow::空ならnone(&p.row.device_category)),
                    hostname: Set(p.row.hostname.trim().to_owned()),
                    serial_number: Set(DeviceRow::空ならnone(&p.row.serial_number)),
                    asset_number: Set(DeviceRow::空ならnone(&p.row.asset_number)),
                    power_watt: Set(p.power_watt),
                    status: Set(p.status.clone()),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?
                .id
            }
        };

        if p.needs_assignment {
            // 別の場所にいたなら、その行を閉じてから開く（不変条件1）
            if let Some(現在) = 現在の割当(tx.reader(), device_id).await? {
                let mut active: device_assignment::ActiveModel = 現在.clone().into();
                active.to_date = Set(Some(as_of));
                tx.update(&現在, active).await?;
            }

            tx.insert(device_assignment::ActiveModel {
                device_id: Set(device_id),
                location_type: Set(PROJECT.to_owned()),
                location_id: Set(Some(project_id)),
                // **初期取込に WORK_ORDER は不要**（23.1）
                work_order_id: Set(None),
                from_date: Set(as_of),
                to_date: Set(None),
                ..Default::default()
            })
            .await?;
        }
    }

    Ok(report)
}

/// 単独で反映する。**エラーが1件でもあれば何も書かない。**
pub async fn apply(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[DeviceRow],
    match_on: &[String],
    as_of: DateTime<Utc>,
    import_run_id: i32,
) -> Result<Report, ImportError> {
    let tx = AuditedTx::begin(db, Actor::Import { import_run_id }).await?;
    let report = 取り込む(&tx, project_id, rows, match_on, as_of).await?;
    if report.has_error() {
        tx.rollback().await?;
        return Err(ImportError::HasErrors(report.count(Outcome::Error)));
    }
    tx.commit().await?;
    Ok(report)
}

/// マニフェストが参照するCSVのパスを、マニフェスト自身の位置から解決する。
pub fn resolve(manifest_path: &Path, relative: &str) -> PathBuf {
    manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn マニフェストを読める() {
        let yaml = r#"
format_version: 1
kind: instances
project: "PRJ-2026-001"
as_of: 2026-04-01T00:00:00+09:00
match_on:
  device: [serial_number]
files:
  - { entity: device, path: devices.csv }
"#;
        let m = parse_manifest(yaml).unwrap();
        assert_eq!(m.project, "PRJ-2026-001");
        assert_eq!(m.match_on.device, ["serial_number"]);
        assert_eq!(m.files[0].entity, "device");
        assert!(m.as_of.is_some());
    }

    #[test]
    fn 種別が違えば読まない() {
        let yaml = "format_version: 1\nkind: catalog\nproject: P\n";
        assert!(matches!(
            parse_manifest(yaml),
            Err(ImportError::UnexpectedKind { .. })
        ));
    }

    #[test]
    fn csvを読める() {
        let csv = "uid,external_id,hostname,serial_number,device_type,status\n\
                   ,SV-0001,web01,JP123,Physical,running\n";
        let rows = parse_devices(csv).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hostname, "web01");
        assert_eq!(rows[0].external_id, "SV-0001");
        assert!(rows[0].uid.is_empty());
    }

    /// 表計算ソフトが付ける前後の空白で突合が外れないこと。
    #[test]
    fn 前後の空白は落とす() {
        let csv = "hostname,serial_number\n  web01  ,  JP123  \n";
        let rows = parse_devices(csv).unwrap();
        assert_eq!(rows[0].hostname, "web01");
        assert_eq!(rows[0].serial_number, "JP123");
    }

    #[test]
    fn 語彙外は拒否し空欄は既定にする() {
        assert_eq!(語彙("", DEVICE_TYPES, "Physical").unwrap(), "Physical");
        assert_eq!(
            語彙("Virtual", DEVICE_TYPES, "Physical").unwrap(),
            "Virtual"
        );
        assert!(語彙("でたらめ", DEVICE_TYPES, "Physical").is_err());
    }

    #[test]
    fn マニフェストからの相対パスを解く() {
        let base = Path::new("/tmp/import/manifest.yaml");
        assert_eq!(
            resolve(base, "devices.csv"),
            PathBuf::from("/tmp/import/devices.csv")
        );
    }
}
