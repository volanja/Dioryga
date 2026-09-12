//! 部品の取込（設計書23.5、6.2、12.4）。
//!
//! `PART_INSTANCE`（部品の実物）と、その所在 `PART_INSTANCE_LOCATION` を扱う。
//! 所在は `location_type=Device` なら搭載中、`Warehouse` なら在庫（12.4）。
//!
//! # シリアル番号を必須にする
//!
//! **宣言的な取込（23.1）は、行が何を指すかを特定できて初めて成り立つ。**
//! `PART_INSTANCE` は `uid` も `external_id` も持たず、識別子はシリアル番号しか
//! ない。空のまま受け付けると**突合できず毎回新規になり、二度流すと部品が倍に
//! なる。**シリアルの無い部品は画面から登録する。
//!
//! # シリアルはベンダーの中で一意とみなす（23.10）
//!
//! 23.10が「ベンダーをまたぐと衝突しうる」と残していた論点に、部品については
//! **ベンダー＋シリアル番号**で答える。シリアルの採番はベンダーごとに独立して
//! おり、**別ベンダーの同じ番号は別の部品である。**全体で一意とみなすと、
//! 実在する別々の部品を同一として取り違える。
//!
//! # このプロジェクトに関わった部品だけを動かせる
//!
//! **突合の候補は、このプロジェクトの機器に一度でも載ったことがある部品に限る**
//! （23.5、機器のA-6と同じ考え方）。候補外に同じベンダー＋シリアルがあれば、
//! **新規作成せずエラーとする**——他プロジェクトの部品を書き換えることも、
//! 黙って2つ目を作ることもしない（23.9.1と同根）。
//!
//! 倉庫にある部品も、このプロジェクトに関わったことが無ければ候補外になる。
//! **倉庫からの払い出しは変更管理チケットの担当**であり（11章）、取込で
//! 引き込めると誰がいつ持ち出したのかが残らない。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{
    chassis_slot, configuration, device, part_catalog, part_instance, part_instance_location,
    vendor, warehouse,
};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use super::instances::{語彙, STATUSES};
use super::placement::{
    このプロジェクトの機器, 機器キー, 空ならnone, 解決する機器, 読み取る
};
use super::{Entry, ImportError, Outcome, Report};
use crate::repository::{Actor, AuditedTx};

const DEVICE: &str = "Device";
const WAREHOUSE: &str = "Warehouse";
const DISPOSED: &str = "Disposed";

#[derive(Debug, Clone, Deserialize)]
pub struct PartRow {
    /// **必須。**識別子がこれしかない（本モジュールのドキュメント）。
    #[serde(default)]
    pub serial_number: String,
    /// 部品カタログの自然キー（6.2）。
    pub part_vendor: String,
    pub part_number: String,
    #[serde(default)]
    pub status: String,
    /// Device / Warehouse / Disposed。
    pub location_type: String,
    /// `Device` のとき、載っている機器のホスト名。
    #[serde(default)]
    pub location_hostname: String,
    /// `Warehouse` のとき、倉庫名。
    #[serde(default)]
    pub location_name: String,
    /// **任意。**どのスロットに挿さっているかまでは求めない（6.2）。
    #[serde(default)]
    pub chassis_slot: String,
}

impl PartRow {
    fn 表示名(&self) -> String {
        format!(
            "{} {} ({})",
            self.part_vendor.trim(),
            self.part_number.trim(),
            match self.serial_number.trim() {
                "" => "シリアルなし",
                s => s,
            }
        )
    }
}

pub fn parse_parts(source: &str) -> Result<Vec<PartRow>, ImportError> {
    読み取る(source)
}

// ---------------------------------------------------------------------------
// 計画
// ---------------------------------------------------------------------------

struct 部品の計画 {
    /// `None` なら新規作成。
    既存: Option<part_instance::Model>,
    part_catalog_id: i32,
    serial_number: String,
    status: String,
    location_type: String,
    location_id: Option<i32>,
    chassis_slot_id: Option<i32>,
    /// 閉じる所在。**一致していれば `None` で、履歴行を作らない**（23.1）。
    閉じる所在: Option<part_instance_location::Model>,
    /// 所在を開き直す必要があるか。
    所在を開く: bool,
}

async fn 計画する<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    rows: &[PartRow],
) -> Result<(Report, Vec<部品の計画>), ImportError> {
    let 機器 = このプロジェクトの機器(db, project_id).await?;
    let 候補 = このプロジェクトに関わった部品(db, &機器).await?;
    let 倉庫: HashMap<String, i32> = warehouse::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|w| (w.name, w.id))
        .collect();

    let mut report = Report::default();
    let mut planned = Vec::new();
    // **ファイル内の重複を先に見る。**同じ部品を2行で別の場所に置くと、
    // どちらが正しいのか決められない
    let mut 既出: HashMap<(String, String), usize> = HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let 表示 = row.表示名();

        let Some(serial) = 空ならnone(&row.serial_number) else {
            report.push(Entry::new(
                Outcome::Error,
                表示,
                "serial_number が要ります。シリアルの無い部品は画面から登録してください",
            ));
            continue;
        };

        let 鍵 = (row.part_vendor.trim().to_owned(), serial.to_owned());
        if let Some(前) = 既出.insert(鍵, i) {
            report.push(Entry::new(
                Outcome::Error,
                表示,
                format!("{}行目と同じ部品です", 前 + 2),
            ));
            continue;
        }

        let catalog = match 解決する部品カタログ(db, row).await? {
            Ok(c) => c,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, 表示, 理由));
                continue;
            }
        };

        let status = match 語彙(&row.status, STATUSES, "running") {
            Ok(s) => s,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, 表示, format!("status: {理由}")));
                continue;
            }
        };

        // 所在の解決
        let (location_type, location_id, chassis_slot_id) = match row.location_type.trim() {
            DEVICE => {
                let key = 機器キー {
                    hostname: row.location_hostname.clone(),
                    ..Default::default()
                };
                let d = match 解決する機器(db, &機器, &key).await? {
                    Ok(d) => d,
                    Err(理由) => {
                        report.push(Entry::new(Outcome::Error, 表示, 理由));
                        continue;
                    }
                };
                let slot = match 空ならnone(&row.chassis_slot) {
                    None => None,
                    Some(label) => match 解決するスロット(db, &d, label).await? {
                        Ok(id) => Some(id),
                        Err(理由) => {
                            report.push(Entry::new(Outcome::Error, 表示, 理由));
                            continue;
                        }
                    },
                };
                (DEVICE.to_owned(), Some(d.id), slot)
            }
            WAREHOUSE => {
                let Some(name) = 空ならnone(&row.location_name) else {
                    report.push(Entry::new(
                        Outcome::Error,
                        表示,
                        "location_type=Warehouse には location_name が要ります",
                    ));
                    continue;
                };
                match 倉庫.get(name) {
                    Some(id) => (WAREHOUSE.to_owned(), Some(*id), None),
                    None => {
                        report.push(Entry::new(
                            Outcome::Error,
                            表示,
                            format!("倉庫「{name}」が見つかりません"),
                        ));
                        continue;
                    }
                }
            }
            DISPOSED => (DISPOSED.to_owned(), None, None),
            other => {
                report.push(Entry::new(
                    Outcome::Error,
                    表示,
                    format!(
                        "location_type「{other}」は扱えません。Device / Warehouse / Disposed のいずれかです"
                    ),
                ));
                continue;
            }
        };

        if location_type != DEVICE && 空ならnone(&row.chassis_slot).is_some() {
            report.push(Entry::new(
                Outcome::Error,
                表示,
                "chassis_slot は location_type=Device のときだけ指定できます",
            ));
            continue;
        }

        // 突合（ベンダー＋シリアル）
        let 既存 = match 突合(db, &候補, catalog.vendor_id, serial).await? {
            Ok(p) => p,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, 表示, 理由));
                continue;
            }
        };

        let 現在 = match &既存 {
            Some(p) => 現在の所在(db, p.id).await?,
            None => None,
        };

        let 所在が同じ = 現在.as_ref().is_some_and(|l| {
            l.location_type == location_type
                && l.location_id == location_id
                && l.chassis_slot_id == chassis_slot_id
        });
        let 属性が同じ = 既存
            .as_ref()
            .is_some_and(|p| p.part_catalog_id == catalog.id && p.status == status);

        let outcome = match (&既存, 所在が同じ && 属性が同じ) {
            (None, _) => Outcome::Created,
            (Some(_), true) => Outcome::Unchanged,
            (Some(_), false) => Outcome::Updated,
        };
        report.push(Entry::new(outcome, 表示, String::new()));
        if outcome == Outcome::Unchanged {
            continue;
        }

        planned.push(部品の計画 {
            既存,
            part_catalog_id: catalog.id,
            serial_number: serial.to_owned(),
            status,
            location_type,
            location_id,
            chassis_slot_id,
            所在を開く: !所在が同じ,
            閉じる所在: if 所在が同じ { None } else { 現在 },
        });
    }

    Ok((report, planned))
}

/// 部品カタログをベンダー＋型番で引く（6.2）。**統合先を辿る**（18.5）。
async fn 解決する部品カタログ<C: ConnectionTrait>(
    db: &C,
    row: &PartRow,
) -> Result<Result<part_catalog::Model, String>, sea_orm::DbErr> {
    let vendor_name = row.part_vendor.trim();
    let part_number = row.part_number.trim();

    let Some(v) = vendor::Entity::find()
        .filter(vendor::Column::Name.eq(vendor_name))
        .one(db)
        .await?
    else {
        return Ok(Err(format!("ベンダー「{vendor_name}」が見つかりません")));
    };
    // **統合で吸収されたベンダーは統合先で引き直す**（23.9.4）
    let vendor_id = v.merged_into_vendor_id.unwrap_or(v.id);

    let Some(mut c) = part_catalog::Entity::find()
        .filter(part_catalog::Column::VendorId.eq(vendor_id))
        .filter(part_catalog::Column::PartNumber.eq(part_number))
        .one(db)
        .await?
    else {
        return Ok(Err(format!(
            "部品カタログ「{vendor_name} {part_number}」が見つかりません"
        )));
    };

    // 統合先を辿る。鎖が閉じていても止まるよう上限を置く
    for _ in 0..16 {
        let Some(next) = c.merged_into_part_catalog_id else {
            break;
        };
        match part_catalog::Entity::find_by_id(next).one(db).await? {
            Some(found) => c = found,
            None => break,
        }
    }
    Ok(Ok(c))
}

/// 機器の筐体モデルが持つスロットをラベルで引く（6.2）。
async fn 解決するスロット<C: ConnectionTrait>(
    db: &C,
    d: &device::Model,
    label: &str,
) -> Result<Result<i32, String>, sea_orm::DbErr> {
    let Some(configuration_id) = d.configuration_id else {
        return Ok(Err(format!(
            "{} は構成を持たないため、スロットを指定できません",
            d.hostname
        )));
    };
    let Some(c) = configuration::Entity::find_by_id(configuration_id)
        .one(db)
        .await?
    else {
        return Ok(Err("構成が見つかりません".to_owned()));
    };

    let slot = chassis_slot::Entity::find()
        .filter(chassis_slot::Column::ChassisModelId.eq(c.chassis_model_id))
        .filter(chassis_slot::Column::SlotLabel.eq(label))
        .one(db)
        .await?;
    Ok(match slot {
        Some(s) => Ok(s.id),
        None => Err(format!(
            "{} の筐体にスロット「{label}」がありません",
            d.hostname
        )),
    })
}

/// このプロジェクトの機器に一度でも載ったことがある部品（23.5、A-6）。
pub(super) async fn このプロジェクトに関わった部品<C: ConnectionTrait>(
    db: &C,
    機器: &[device::Model],
) -> Result<Vec<part_instance::Model>, sea_orm::DbErr> {
    if 機器.is_empty() {
        return Ok(Vec::new());
    }
    let 機器id: Vec<i32> = 機器.iter().map(|d| d.id).collect();

    let mut ids: Vec<i32> = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(DEVICE))
        .filter(part_instance_location::Column::LocationId.is_in(機器id))
        .all(db)
        .await?
        .into_iter()
        .map(|l| l.part_instance_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();

    if ids.is_empty() {
        return Ok(Vec::new());
    }
    part_instance::Entity::find()
        .filter(part_instance::Column::Id.is_in(ids))
        .all(db)
        .await
}

/// ベンダー＋シリアルで突合する。
///
/// **候補外に同じ部品があれば新規にしない。**他プロジェクトの部品を書き換える
/// ことも、黙って2つ目を作ることもしない（23.9.1と同根）。
async fn 突合<C: ConnectionTrait>(
    db: &C,
    候補: &[part_instance::Model],
    vendor_id: i32,
    serial: &str,
) -> Result<Result<Option<part_instance::Model>, String>, sea_orm::DbErr> {
    // 同じシリアルの部品を全体から引き、ベンダーで絞る
    let 同シリアル = part_instance::Entity::find()
        .filter(part_instance::Column::SerialNumber.eq(serial))
        .all(db)
        .await?;

    let mut 同一 = Vec::new();
    for p in 同シリアル {
        let Some(c) = part_catalog::Entity::find_by_id(p.part_catalog_id)
            .one(db)
            .await?
        else {
            continue;
        };
        if c.vendor_id == vendor_id {
            同一.push(p);
        }
    }

    match 同一.len() {
        0 => Ok(Ok(None)),
        1 => {
            let p = 同一.remove(0);
            if 候補.iter().any(|c| c.id == p.id) {
                Ok(Ok(Some(p)))
            } else {
                Ok(Err(format!(
                    "シリアル「{serial}」の部品は、このプロジェクトの機器に載ったことがありません。倉庫からの払い出しは変更管理チケットで行ってください"
                )))
            }
        }
        n => Ok(Err(format!(
            "シリアル「{serial}」の部品が同じベンダーに{n}件あります。突合できません"
        ))),
    }
}

async fn 現在の所在<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
) -> Result<Option<part_instance_location::Model>, sea_orm::DbErr> {
    part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(part_instance_id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .one(db)
        .await
}

// ---------------------------------------------------------------------------
// ドライランと反映
// ---------------------------------------------------------------------------

pub async fn dry_run(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[PartRow],
) -> Result<Report, ImportError> {
    Ok(計画する(db, project_id, rows).await?.0)
}

/// 同じトランザクションの中で計画し、書き込む（23.6）。
///
/// **エラーの行は書かず、正しい行だけを書く。**取込全体の判定は呼び出し側が
/// 行い、1件でもエラーがあればトランザクションごと捨てる。正しい行を書いて
/// おくのは、**後のエンティティがそれを参照できるようにするため**である。
pub async fn 取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[PartRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    // **トランザクションの中で読む。**同じ取込で先に書いたもの（機器など）が
    // 見えるのはこの経路だけであり、SQLiteのインメモリでは外の接続で読むと
    // 止まる（24.2.5）
    let (report, planned) = 計画する(tx.reader(), project_id, rows).await?;
    let now = Utc::now();

    for p in planned {
        // 部品そのものは履歴ではない。**所在だけが履歴**（12.4）
        let part_id = match &p.既存 {
            Some(existing) => {
                let mut active: part_instance::ActiveModel = existing.clone().into();
                active.part_catalog_id = Set(p.part_catalog_id);
                active.status = Set(p.status.clone());
                active.updated_at = Set(now);
                tx.update(existing, active).await?;
                existing.id
            }
            None => {
                tx.insert(part_instance::ActiveModel {
                    part_catalog_id: Set(p.part_catalog_id),
                    serial_number: Set(Some(p.serial_number.clone())),
                    status: Set(p.status.clone()),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?
                .id
            }
        };

        if !p.所在を開く {
            continue;
        }
        // **閉じて開く**（4章）
        if let Some(現在) = &p.閉じる所在 {
            let mut active: part_instance_location::ActiveModel = 現在.clone().into();
            active.to_date = Set(Some(as_of));
            tx.update(現在, active).await?;
        }
        tx.insert(part_instance_location::ActiveModel {
            part_instance_id: Set(part_id),
            location_type: Set(p.location_type),
            location_id: Set(p.location_id),
            chassis_slot_id: Set(p.chassis_slot_id),
            work_order_id: Set(None),
            from_date: Set(as_of),
            to_date: Set(None),
            ..Default::default()
        })
        .await?;
    }

    Ok(report)
}

/// 単独で反映する。**エラーが1件でもあれば何も書かない。**
pub async fn apply(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[PartRow],
    as_of: DateTime<Utc>,
    import_run_id: i32,
) -> Result<Report, ImportError> {
    let tx = AuditedTx::begin(db, Actor::Import { import_run_id }).await?;
    let report = 取り込む(&tx, project_id, rows, as_of).await?;
    if report.has_error() {
        tx.rollback().await?;
        return Err(ImportError::HasErrors(report.count(Outcome::Error)));
    }
    tx.commit().await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 部品のcsvを読める() {
        let rows = parse_parts(
            "serial_number,part_vendor,part_number,status,location_type,location_hostname,location_name,chassis_slot\n\
             SN-1,Samsung,M393A4K40DB3,running,Device,web01,,DIMM_A1\n\
             SN-2,Samsung,M393A4K40DB3,,Warehouse,,本社倉庫,\n",
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].chassis_slot, "DIMM_A1");
        assert_eq!(rows[1].location_name, "本社倉庫");
    }

    #[test]
    fn シリアルが無ければ表示でそう示す() {
        let rows =
            parse_parts("serial_number,part_vendor,part_number,location_type\n,Samsung,X,Device\n")
                .unwrap();
        assert!(rows[0].表示名().contains("シリアルなし"));
    }
}
