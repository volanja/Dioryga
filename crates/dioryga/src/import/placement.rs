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

/// 機器を自然キーで引くための索引（#111）。
///
/// # 行ごとに走査しない
///
/// **候補を行ごとに線形走査すると、行数×台数で伸びる。**#22の実測では、
/// 10,000台の再取込でドライランに267秒かかっていた（初回は3.5秒）。初回が
/// 速いのは候補が空だからで、**既存に当たる取込ほど遅くなる。**23.1が
/// 「1行直して再取込」を前提に宣言的な形式を選んだ以上、そこが遅くては困る。
///
/// 列ごとのHashMapに変え、**統合先も取込の最初にまとめて解決する。**行ごとの
/// DB問い合わせが無くなる。
///
/// # 何件該当したかを保つ
///
/// 値ごとに `Vec<i32>` を持つ。**「1件」と「複数件」を区別できなくなると、
/// 黙って1台目を選ぶことになる**（23.2）。
pub(super) struct 機器の索引 {
    列: HashMap<&'static str, HashMap<String, Vec<i32>>>,
    行: HashMap<i32, device::Model>,
    /// 統合で吸収された機器の、最終的な着地先（23.9.4）。
    統合先: HashMap<i32, i32>,
}

/// 索引を張る列。突合（23.2）と重複の疑い（23.9.1）が使うものすべて。
const 索引する列: [&str; 5] = [
    "uid",
    "external_id",
    "serial_number",
    "asset_number",
    "hostname",
];

pub(super) fn 機器の値(d: &device::Model, 列: &str) -> Option<String> {
    match 列 {
        "uid" => Some(d.uid.clone()),
        "external_id" => d.external_id.clone(),
        "serial_number" => d.serial_number.clone(),
        "asset_number" => d.asset_number.clone(),
        "hostname" => Some(d.hostname.clone()),
        _ => None,
    }
}

impl 機器の索引 {
    pub(super) async fn 作る<C: ConnectionTrait>(
        db: &C,
        project_id: i32,
    ) -> Result<Self, sea_orm::DbErr> {
        let 候補 = このプロジェクトの機器(db, project_id).await?;

        let mut 行: HashMap<i32, device::Model> = HashMap::new();
        let mut 列: HashMap<&'static str, HashMap<String, Vec<i32>>> = HashMap::new();
        for 名 in 索引する列 {
            列.insert(名, HashMap::new());
        }
        for d in &候補 {
            for 名 in 索引する列 {
                let Some(v) = 機器の値(d, 名) else {
                    continue;
                };
                if v.trim().is_empty() {
                    continue;
                }
                列.get_mut(名)
                    .expect("索引する列は先に用意している")
                    .entry(v)
                    .or_default()
                    .push(d.id);
            }
            行.insert(d.id, d.clone());
        }

        // **統合先が候補の外にあることがある。**辿れるところまでまとめて読む
        for _ in 0..16 {
            let 不足: Vec<i32> = 行
                .values()
                .filter_map(|d| d.merged_into_device_id)
                .filter(|id| !行.contains_key(id))
                .collect();
            if 不足.is_empty() {
                break;
            }
            for d in device::Entity::find()
                .filter(device::Column::Id.is_in(不足))
                .all(db)
                .await?
            {
                行.insert(d.id, d);
            }
        }

        let ids: Vec<i32> = 行.keys().copied().collect();
        let mut 統合先 = HashMap::new();
        for id in ids {
            統合先.insert(id, 辿り着く先(&行, id));
        }

        Ok(Self {
            列, 行, 統合先
        })
    }

    /// この値に該当する機器のID。**件数をそのまま返す**（23.2）。
    pub(super) fn 該当(&self, 列: &str, value: &str) -> &[i32] {
        self.列
            .get(列)
            .and_then(|m| m.get(value))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 統合先まで辿った機器（23.9.4）。
    pub(super) fn 機器(&self, id: i32) -> Option<device::Model> {
        let 先 = self.統合先.get(&id).copied().unwrap_or(id);
        self.行.get(&先).cloned()
    }

    /// 候補に入っている機器のID。
    pub(super) fn ids(&self) -> Vec<i32> {
        self.行.keys().copied().collect()
    }

    /// 自然キーから機器を引く（23.2）。
    ///
    /// **`uid` → `external_id` → `serial_number` → `hostname` の順。**機器CSVの
    /// 突合（`instances.rs`）と違い `match_on` は見ない——配置のCSVは**既にある
    /// 機器を指すだけ**であり、新規作成しないため突合キーを選ぶ必要がない。
    ///
    /// **統合先を辿る**（23.9.4）。吸収された側の識別子で書かれていても着地する。
    pub(super) fn 引く(&self, key: &機器キー) -> Result<device::Model, String> {
        let 順序 = [
            ("uid", 空ならnone(&key.uid)),
            ("external_id", 空ならnone(&key.external_id)),
            ("serial_number", 空ならnone(&key.serial_number)),
            ("hostname", 空ならnone(&key.hostname)),
        ];

        for (列, value) in 順序 {
            let Some(value) = value else { continue };
            let 一致 = self.該当(列, value);
            return match 一致.len() {
                1 => self
                    .機器(一致[0])
                    .ok_or_else(|| format!("{列}「{value}」の機器を読み出せません")),
                0 => Err(format!(
                    "{列}「{value}」の機器がこのプロジェクトにありません"
                )),
                // **黙って1台目を選ばない。**どれを指しているか決められない
                n => Err(format!(
                    "{列}「{value}」に{n}台が該当します。uid で指定してください"
                )),
            };
        }

        Err("機器を特定する列が空です".to_owned())
    }
}

/// 統合の鎖を辿った先のID。
///
/// **鎖が閉じている場合に止まらなくならないよう、上限を置く。**
/// `dioryga check` が循環を検出する（24.5）が、取込がその前に回りうる。
fn 辿り着く先(行: &HashMap<i32, device::Model>, mut id: i32) -> i32 {
    for _ in 0..16 {
        match 行.get(&id).and_then(|d| d.merged_into_device_id) {
            Some(next) if 行.contains_key(&next) => id = next,
            _ => return id,
        }
    }
    id
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

async fn 所属を計画する<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    rows: &[AssignmentRow],
) -> Result<(Report, Vec<所属の計画>), ImportError> {
    let 索引 = 機器の索引::作る(db, project_id).await?;
    let 倉庫 = 倉庫の索引(db).await?;

    let mut report = Report::default();
    let mut planned = Vec::new();

    for row in rows {
        let key = row.key();
        let 表示 = key.表示名();

        let d = match 索引.引く(&key) {
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

/// 同じトランザクションの中で計画し、書き込む（23.6）。
///
/// **エラーの行は書かず、正しい行だけを書く。**取込全体の判定は呼び出し側が
/// 行い、1件でもエラーがあればトランザクションごと捨てる。正しい行を書いて
/// おくのは、**後のエンティティがそれを参照できるようにするため**である。
pub async fn 所属を取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[AssignmentRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    // **トランザクションの中で読む。**同じ取込で先に書いたもの（機器など）が
    // 見えるのはこの経路だけであり、SQLiteのインメモリでは外の接続で読むと
    // 止まる（24.2.5）
    let (report, planned) = 所属を計画する(tx.reader(), project_id, rows).await?;

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

    Ok(report)
}

/// 単独で反映する。**エラーが1件でもあれば何も書かない。**
pub async fn assignments_apply(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[AssignmentRow],
    as_of: DateTime<Utc>,
    import_run_id: i32,
) -> Result<Report, ImportError> {
    let tx = AuditedTx::begin(db, Actor::Import { import_run_id }).await?;
    let report = 所属を取り込む(&tx, project_id, rows, as_of).await?;
    if report.has_error() {
        tx.rollback().await?;
        return Err(ImportError::HasErrors(report.count(Outcome::Error)));
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

async fn 搭載を計画する<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    rows: &[MountRow],
) -> Result<(Report, Vec<搭載の計画>), ImportError> {
    let 索引 = 機器の索引::作る(db, project_id).await?;
    let 什器 = 什器の索引(db, project_id).await?;

    let mut report = Report::default();
    let mut planned = Vec::new();

    for row in rows {
        let key = row.key();
        let 表示 = key.表示名();

        let d = match 索引.引く(&key) {
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
                    match 索引.引く(&key) {
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

/// 同じトランザクションの中で計画し、書き込む（23.6）。
///
/// **エラーの行は書かず、正しい行だけを書く。**取込全体の判定は呼び出し側が
/// 行い、1件でもエラーがあればトランザクションごと捨てる。正しい行を書いて
/// おくのは、**後のエンティティがそれを参照できるようにするため**である。
pub async fn 搭載を取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[MountRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    // **トランザクションの中で読む。**同じ取込で先に書いたもの（機器など）が
    // 見えるのはこの経路だけであり、SQLiteのインメモリでは外の接続で読むと
    // 止まる（24.2.5）
    let (report, planned) = 搭載を計画する(tx.reader(), project_id, rows).await?;

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

    Ok(report)
}

/// 単独で反映する。**エラーが1件でもあれば何も書かない。**
pub async fn mounts_apply(
    db: &DatabaseConnection,
    project_id: i32,
    rows: &[MountRow],
    as_of: DateTime<Utc>,
    import_run_id: i32,
) -> Result<Report, ImportError> {
    let tx = AuditedTx::begin(db, Actor::Import { import_run_id }).await?;
    let report = 搭載を取り込む(&tx, project_id, rows, as_of).await?;
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
