//! 配置の取込（設計書23.5、12.3）。
//!
//! `DEVICE_ASSIGNMENT`（どの組織が所有するか）と `DEVICE_MOUNT`（什器の何番目に
//! 物理的に載っているか）を扱う。**この2つは独立した履歴である**（12.3）——
//! 粒度が違い、片方だけが動くことがある。
//!
//! # 宣言であって命令ではない（23.1）
//!
//! 「今こうなっているはず」を書く。現在の事実と一致していれば**履歴行を作らない。**
//! 違っていれば**閉じて開く**（4章、不変条件1/2）。既存行は更新しない。
//!
//! これにより**同じファイルを何度流しても結果が変わらない。**命令形にすると
//! 2回流した時点で履歴行が重複し、事実が壊れる。
//!
//! # 所属の移動先はこのプロジェクトか倉庫に限る
//!
//! **他プロジェクトへ移す取込は受け付けない。**23.5の「1つの取込ファイルは
//! 1プロジェクトに閉じる」に反し、**取込ファイル1つで他プロジェクトのデータを
//! 書き換えられてしまう。**機器の移譲は11章の変更管理チケット（Transfer）の
//! 担当である。
//!
//! したがって `location_type` に書けるのは次の3つ。
//!
//! | 値 | 意味 |
//! |---|---|
//! | `Project` | マニフェストのプロジェクト。`location_name` は書かない |
//! | `Warehouse` | 倉庫。`location_name` に倉庫名が要る |
//! | `Disposed` | 廃棄。参照先を持たない |
//!
//! # 重複配置は警告に留める（12.3、不変条件6）
//!
//! 同じUに置けるのは左右の組か前後の組のときだけ、という規則がある。
//! **取込では違反を警告として記録し、取り込む。**実機が規則の想定外である
//! ことはあり、**誤って拒否すると事実を記録できなくなる。**画面（`rack.rs`）
//! が登録時に同じ判定をしており、そちらも衝突を止めてはいない。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{device, device_assignment, device_mount, mount_container, warehouse};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use super::{Entry, ImportError, Outcome, Report};
use crate::repository::{Actor, AuditedTx};

const PROJECT: &str = "Project";
const WAREHOUSE: &str = "Warehouse";
const DISPOSED: &str = "Disposed";

const LEFT: &str = "Left";
const RIGHT: &str = "Right";
const FRONT: &str = "Front";
const REAR: &str = "Rear";

/// 機器を指す自然キー（23.2）。**両方のCSVが同じ形で持つ。**
///
/// **DBの内部IDは人が書くファイルに現れない。**
///
/// # 各行の構造体に展開している
///
/// `#[serde(flatten)]` でまとめたいところだが、**csvクレートは flatten を
/// 扱えない。**列の集合を事前に知る必要があるためで、使うと実行時に落ちる。
/// 同じ理由で `axum::Form`（urlencoded）でも使えない。
#[derive(Debug, Clone, Default)]
pub struct 機器キー {
    pub uid: String,
    pub external_id: String,
    pub hostname: String,
    pub serial_number: String,
}

impl 機器キー {
    pub(super) fn 表示名(&self) -> String {
        for candidate in [
            &self.hostname,
            &self.uid,
            &self.external_id,
            &self.serial_number,
        ] {
            let v = candidate.trim();
            if !v.is_empty() {
                return v.to_owned();
            }
        }
        "(識別子なし)".to_owned()
    }
}

pub(super) fn 空ならnone(value: &str) -> Option<&str> {
    match value.trim() {
        "" => None,
        v => Some(v),
    }
}

// ---------------------------------------------------------------------------
// 所属（DEVICE_ASSIGNMENT）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct AssignmentRow {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub external_id: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub serial_number: String,
    /// Project / Warehouse / Disposed。
    pub location_type: String,
    /// `Warehouse` のときだけ意味を持つ。
    #[serde(default)]
    pub location_name: String,
}

impl AssignmentRow {
    fn key(&self) -> 機器キー {
        機器キー {
            uid: self.uid.clone(),
            external_id: self.external_id.clone(),
            hostname: self.hostname.clone(),
            serial_number: self.serial_number.clone(),
        }
    }
}

pub fn parse_assignments(source: &str) -> Result<Vec<AssignmentRow>, ImportError> {
    読み取る(source)
}

// ---------------------------------------------------------------------------
// 搭載（DEVICE_MOUNT）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct MountRow {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub external_id: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub serial_number: String,
    /// 什器の名前。`host_hostname` を使う場合は空。
    #[serde(default)]
    pub container: String,
    #[serde(default)]
    pub position: String,
    /// Left / Right / Full。
    #[serde(default)]
    pub horizontal_position: String,
    /// Front / Rear / Full。
    #[serde(default)]
    pub depth_position: String,
    /// 棚板やハイパーバイザの上に載る場合（12.3、13章）。
    #[serde(default)]
    pub host_hostname: String,
}

impl MountRow {
    fn key(&self) -> 機器キー {
        機器キー {
            uid: self.uid.clone(),
            external_id: self.external_id.clone(),
            hostname: self.hostname.clone(),
            serial_number: self.serial_number.clone(),
        }
    }
}

pub fn parse_mounts(source: &str) -> Result<Vec<MountRow>, ImportError> {
    読み取る(source)
}

pub(super) fn 読み取る<T: serde::de::DeserializeOwned>(
    source: &str,
) -> Result<Vec<T>, ImportError> {
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
// 機器の解決
// ---------------------------------------------------------------------------

/// 自然キーから機器を引く（23.2）。
///
/// **`uid` → `external_id` → `serial_number` → `hostname` の順。**機器CSVの
/// 突合（`instances.rs`）と違い `match_on` は見ない——配置のCSVは**既にある
/// 機器を指すだけ**であり、新規作成しないため突合キーを選ぶ必要がない。
///
/// **統合先を辿る**（23.9.4）。吸収された側の識別子で書かれていても着地する。
pub(super) async fn 解決する機器<C: ConnectionTrait>(
    db: &C,
    候補: &[device::Model],
    key: &機器キー,
) -> Result<Result<device::Model, String>, sea_orm::DbErr> {
    type 引く = (&'static str, fn(&device::Model) -> Option<String>);
    let 順序: [(Option<&str>, 引く); 4] = [
        (空ならnone(&key.uid), ("uid", |d| Some(d.uid.clone()))),
        (
            空ならnone(&key.external_id),
            ("external_id", |d| d.external_id.clone()),
        ),
        (
            空ならnone(&key.serial_number),
            ("serial_number", |d| d.serial_number.clone()),
        ),
        (
            空ならnone(&key.hostname),
            ("hostname", |d| Some(d.hostname.clone())),
        ),
    ];

    for (value, (列, 取り出す)) in 順序 {
        let Some(value) = value else { continue };
        let 一致: Vec<&device::Model> = 候補
            .iter()
            .filter(|d| 取り出す(d).as_deref() == Some(value))
            .collect();

        return Ok(match 一致.len() {
            1 => Ok(統合先を辿る(db, 一致[0].clone()).await?),
            0 => Err(format!(
                "{列}「{value}」の機器がこのプロジェクトにありません"
            )),
            // **黙って1台目を選ばない。**どれを指しているか決められない
            n => Err(format!(
                "{列}「{value}」に{n}台が該当します。uid で指定してください"
            )),
        });
    }

    Ok(Err("機器を特定する列が空です".to_owned()))
}

/// 統合で吸収された機器を辿る（設計書23.9.4）。
async fn 統合先を辿る<C: ConnectionTrait>(
    db: &C,
    mut d: device::Model,
) -> Result<device::Model, sea_orm::DbErr> {
    // **鎖が閉じている場合に止まらなくならないよう、上限を置く。**
    // `dioryga check` が循環を検出する（24.5）が、取込がその前に回りうる
    for _ in 0..16 {
        let Some(next) = d.merged_into_device_id else {
            return Ok(d);
        };
        let Some(found) = device::Entity::find_by_id(next).one(db).await? else {
            return Ok(d);
        };
        d = found;
    }
    Ok(d)
}

/// このプロジェクトに属する（属したことがある）機器（23.5、A-6）。
pub(super) async fn このプロジェクトの機器<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> Result<Vec<device::Model>, sea_orm::DbErr> {
    let ids: Vec<i32> = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .all(db)
        .await?
        .into_iter()
        .map(|a| a.device_id)
        .collect();

    if ids.is_empty() {
        return Ok(Vec::new());
    }
    device::Entity::find()
        .filter(device::Column::Id.is_in(ids))
        .all(db)
        .await
}

// ---------------------------------------------------------------------------
// 所属の計画
// ---------------------------------------------------------------------------

struct 所属の計画 {
    device_id: i32,
    location_type: String,
    location_id: Option<i32>,
    /// 閉じる対象。**現在の行と一致していれば `None` で、何も書かない。**
    閉じる: Option<device_assignment::Model>,
}

async fn 所属を計画する(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[AssignmentRow],
) -> Result<(Report, Vec<所属の計画>), ImportError> {
    let 候補 = このプロジェクトの機器(db, project_id).await?;
    let 倉庫 = 倉庫の索引(db).await?;

    let mut report = Report::default();
    let mut planned = Vec::new();

    for row in rows {
        let key = row.key();
        let 表示 = key.表示名();

        let d = match 解決する機器(db, &候補, &key).await? {
            Ok(d) => d,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, 表示, 理由));
                continue;
            }
        };

        let (location_type, location_id) = match row.location_type.trim() {
            PROJECT => (PROJECT.to_owned(), Some(project_id)),
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
                    Some(id) => (WAREHOUSE.to_owned(), Some(*id)),
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
            DISPOSED => (DISPOSED.to_owned(), None),
            other => {
                // **他プロジェクトへの移動は受け付けない**（23.5）。
                // 移譲は変更管理チケットの担当（11章）
                report.push(Entry::new(
                    Outcome::Error,
                    表示,
                    format!(
                        "location_type「{other}」は扱えません。Project / Warehouse / Disposed のいずれかです"
                    ),
                ));
                continue;
            }
        };

        let 現在 = 現在の所属(db, d.id).await?;

        // **一致していれば履歴行を作らない**（23.1）
        if let Some(現在) = &現在 {
            if 現在.location_type == location_type && 現在.location_id == location_id {
                report.push(Entry::new(Outcome::Unchanged, 表示, String::new()));
                continue;
            }
        }

        let outcome = if 現在.is_some() {
            Outcome::Updated
        } else {
            Outcome::Created
        };
        let detail = match &現在 {
            Some(a) => format!("{} → {location_type}", a.location_type),
            None => location_type.clone(),
        };
        report.push(Entry::new(outcome, 表示, detail));

        planned.push(所属の計画 {
            device_id: d.id,
            location_type,
            location_id,
            閉じる: 現在,
        });
    }

    Ok((report, planned))
}

async fn 現在の所属<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> Result<Option<device_assignment::Model>, sea_orm::DbErr> {
    device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
}

async fn 倉庫の索引<C: ConnectionTrait>(
    db: &C,
) -> Result<HashMap<String, i32>, sea_orm::DbErr> {
    Ok(warehouse::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|w| (w.name, w.id))
        .collect())
}

pub async fn assignments_dry_run(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[AssignmentRow],
) -> Result<Report, ImportError> {
    Ok(所属を計画する(db, project_id, rows).await?.0)
}

pub async fn assignments_apply(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[AssignmentRow],
    as_of: DateTime<Utc>,
    import_run_id: i32,
) -> Result<Report, ImportError> {
    let (report, planned) = 所属を計画する(db, project_id, rows).await?;
    if report.has_error() {
        return Err(ImportError::HasErrors(report.count(Outcome::Error)));
    }

    // **行ごとの監査ログを書かない**（24.4）
    let tx = AuditedTx::begin(db, Actor::Import { import_run_id }).await?;

    for p in planned {
        // **閉じて開く。**既存行を書き換えると、いつ移ったのかが失われる（4章）
        if let Some(現在) = &p.閉じる {
            let mut active: device_assignment::ActiveModel = 現在.clone().into();
            active.to_date = Set(Some(as_of));
            tx.update(現在, active).await?;
        }
        tx.insert(device_assignment::ActiveModel {
            device_id: Set(p.device_id),
            location_type: Set(p.location_type),
            location_id: Set(p.location_id),
            work_order_id: Set(None),
            from_date: Set(as_of),
            to_date: Set(None),
            ..Default::default()
        })
        .await?;
    }

    tx.commit().await?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// 搭載の計画
// ---------------------------------------------------------------------------

struct 搭載の計画 {
    device_id: i32,
    container_id: Option<i32>,
    position: Option<i32>,
    horizontal_position: Option<String>,
    depth_position: Option<String>,
    host_device_id: Option<i32>,
    閉じる: Option<device_mount::Model>,
}

async fn 搭載を計画する(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[MountRow],
) -> Result<(Report, Vec<搭載の計画>), ImportError> {
    let 候補 = このプロジェクトの機器(db, project_id).await?;
    let 什器 = 什器の索引(db, project_id).await?;

    let mut report = Report::default();
    let mut planned = Vec::new();

    for row in rows {
        let key = row.key();
        let 表示 = key.表示名();

        let d = match 解決する機器(db, &候補, &key).await? {
            Ok(d) => d,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, 表示, 理由));
                continue;
            }
        };

        // 什器に載るか、他の機器の上に載るか（12.3）
        let (container_id, host_device_id) =
            match (空ならnone(&row.container), 空ならnone(&row.host_hostname)) {
                (Some(_), Some(_)) => {
                    report.push(Entry::new(
                        Outcome::Error,
                        表示,
                        "container と host_hostname は同時に指定できません",
                    ));
                    continue;
                }
                (Some(name), None) => match 什器.get(name) {
                    Some(id) => (Some(*id), None),
                    None => {
                        report.push(Entry::new(
                            Outcome::Error,
                            表示,
                            format!("什器「{name}」がこのプロジェクトにありません"),
                        ));
                        continue;
                    }
                },
                (None, Some(host)) => {
                    let key = 機器キー {
                        hostname: host.to_owned(),
                        ..Default::default()
                    };
                    match 解決する機器(db, &候補, &key).await? {
                        Ok(h) if h.id == d.id => {
                            // **自分の上には載れない。**辿ると止まらなくなる
                            report.push(Entry::new(
                                Outcome::Error,
                                表示,
                                "host_hostname が自分自身を指しています",
                            ));
                            continue;
                        }
                        Ok(h) => (None, Some(h.id)),
                        Err(理由) => {
                            report.push(Entry::new(Outcome::Error, 表示, 理由));
                            continue;
                        }
                    }
                }
                (None, None) => {
                    report.push(Entry::new(
                        Outcome::Error,
                        表示,
                        "container か host_hostname のどちらかが要ります",
                    ));
                    continue;
                }
            };

        let position = match 空ならnone(&row.position) {
            None => None,
            Some(v) => match v.parse::<i32>() {
                Ok(n) => Some(n),
                Err(_) => {
                    report.push(Entry::new(
                        Outcome::Error,
                        表示,
                        format!("position「{v}」を数値として読めません"),
                    ));
                    continue;
                }
            },
        };

        let horizontal_position = 空ならnone(&row.horizontal_position).map(str::to_owned);
        let depth_position = 空ならnone(&row.depth_position).map(str::to_owned);

        let 現在 = 現在の搭載(db, d.id).await?;

        // **一致していれば履歴行を作らない**（23.1）
        if let Some(現在) = &現在 {
            if 現在.container_id == container_id
                && 現在.position == position
                && 現在.horizontal_position == horizontal_position
                && 現在.depth_position == depth_position
                && 現在.host_device_id == host_device_id
            {
                report.push(Entry::new(Outcome::Unchanged, 表示, String::new()));
                continue;
            }
        }

        // **重複配置は警告に留める**（12.3、不変条件6）
        let mut outcome = if 現在.is_some() {
            Outcome::Updated
        } else {
            Outcome::Created
        };
        let mut detail = String::new();

        if let (Some(cid), Some(pos)) = (container_id, position) {
            if let Some(理由) = 重複を調べる(
                db,
                cid,
                pos,
                d.id,
                horizontal_position.as_deref(),
                depth_position.as_deref(),
            )
            .await?
            {
                outcome = Outcome::Warning;
                detail = 理由;
            }
        }

        report.push(Entry::new(outcome, 表示, detail));

        planned.push(搭載の計画 {
            device_id: d.id,
            container_id,
            position,
            horizontal_position,
            depth_position,
            host_device_id,
            閉じる: 現在,
        });
    }

    Ok((report, planned))
}

/// 同じ位置に既に何かあるか（設計書12.3）。
///
/// **左右の組か前後の組なら許す。**それ以外の重なりは理由を返す。
/// **返しても止めない**——呼び出し側が警告として記録する。
///
/// 画面側（`server::rack`）は既存機器の高さまで見て区間の重なりを判定するが、
/// **ここでは開始位置だけを見る。**取込は機器CSVと同時に流れることがあり、
/// **`CHASSIS_MODEL` から高さを引ける保証が無い**（構成を持たない機器がある）。
/// 見落としは `dioryga check` の「重複配置」が拾う（24.5）。
async fn 重複を調べる<C: ConnectionTrait>(
    db: &C,
    container_id: i32,
    position: i32,
    device_id: i32,
    horizontal: Option<&str>,
    depth: Option<&str>,
) -> Result<Option<String>, sea_orm::DbErr> {
    let 現行 = device_mount::Entity::find()
        .filter(device_mount::Column::ContainerId.eq(container_id))
        .filter(device_mount::Column::ToDate.is_null())
        .all(db)
        .await?;

    for r in 現行 {
        // 自分自身の現行行は、これから閉じるので数えない
        if r.device_id == device_id {
            continue;
        }
        if r.position != Some(position) {
            continue;
        }

        let 左右で分かれている = matches!(
            (horizontal, r.horizontal_position.as_deref()),
            (Some(LEFT), Some(RIGHT)) | (Some(RIGHT), Some(LEFT))
        );
        if 左右で分かれている {
            continue;
        }

        let 前後で分かれている = matches!(
            (depth, r.depth_position.as_deref()),
            (Some(FRONT), Some(REAR)) | (Some(REAR), Some(FRONT))
        );
        if 前後で分かれている {
            return Ok(Some(format!(
                "{position}U に前後で同居します。排熱上は推奨されません"
            )));
        }

        return Ok(Some(format!("{position}U に既に別の機器があります")));
    }

    Ok(None)
}

async fn 現在の搭載<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> Result<Option<device_mount::Model>, sea_orm::DbErr> {
    device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(db)
        .await
}

/// このプロジェクトの什器（12.1）。**倉庫の什器は含めない**——搭載のCSVは
/// プロジェクトに閉じており、倉庫内の配置は倉庫領域の担当である（16.1のC領域）。
async fn 什器の索引<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> Result<HashMap<String, i32>, sea_orm::DbErr> {
    Ok(mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .all(db)
        .await?
        .into_iter()
        .map(|c| (c.name, c.id))
        .collect())
}

pub async fn mounts_dry_run(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[MountRow],
) -> Result<Report, ImportError> {
    Ok(搭載を計画する(db, project_id, rows).await?.0)
}

pub async fn mounts_apply(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[MountRow],
    as_of: DateTime<Utc>,
    import_run_id: i32,
) -> Result<Report, ImportError> {
    let (report, planned) = 搭載を計画する(db, project_id, rows).await?;
    if report.has_error() {
        return Err(ImportError::HasErrors(report.count(Outcome::Error)));
    }

    let tx = AuditedTx::begin(db, Actor::Import { import_run_id }).await?;

    for p in planned {
        // **閉じて開く**（4章）
        if let Some(現在) = &p.閉じる {
            let mut active: device_mount::ActiveModel = 現在.clone().into();
            active.to_date = Set(Some(as_of));
            tx.update(現在, active).await?;
        }
        tx.insert(device_mount::ActiveModel {
            device_id: Set(p.device_id),
            container_id: Set(p.container_id),
            position: Set(p.position),
            horizontal_position: Set(p.horizontal_position),
            depth_position: Set(p.depth_position),
            host_device_id: Set(p.host_device_id),
            work_order_id: Set(None),
            from_date: Set(as_of),
            to_date: Set(None),
            ..Default::default()
        })
        .await?;
    }

    tx.commit().await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 所属のcsvを読める() {
        let rows = parse_assignments(
            "uid,external_id,hostname,serial_number,location_type,location_name\n\
             ,,web01,,Warehouse,本社倉庫\n\
             ,,web02,,Disposed,\n",
        )
        .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].hostname, "web01");
        assert_eq!(rows[0].location_name, "本社倉庫");
        assert_eq!(rows[1].location_type, "Disposed");
    }

    #[test]
    fn 搭載のcsvを読める() {
        let rows = parse_mounts(
            "uid,external_id,hostname,serial_number,container,position,horizontal_position,depth_position,host_hostname\n\
             ,,web01,,Rack-01,10,Full,Front,\n\
             ,,vm01,,,,,,esxi01\n",
        )
        .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].container, "Rack-01");
        assert_eq!(rows[0].position, "10");
        assert_eq!(rows[1].host_hostname, "esxi01");
        assert!(rows[1].container.is_empty());
    }

    #[test]
    fn 識別子が無ければそう示す() {
        let key = 機器キー::default();
        assert_eq!(key.表示名(), "(識別子なし)");
    }
}
