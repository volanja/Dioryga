//! 部品の取込（設計書23.5、6.2、12.4）。
//!
//! `PART_INSTANCE`（部品の実物）と、その所在 `PART_INSTANCE_LOCATION` を扱う。
//! 所在は `location_type=Device` なら搭載中（12.4）。予備は倉庫用のプロジェクトの
//! 棚かプロジェクトに置く（#196）。
//!
//! # 部品も機器と同じく、棚に置いてもプロジェクトに置いてもよい（#219、16.1）
//!
//! | `location_type` | 置き場所 | `location_name` |
//! |---|---|---|
//! | `Device` | 機器に載っている（`location_hostname` にホスト名） | 書かない |
//! | `MountContainer` | 設備・什器（ラック・棚）に置いてある | 設備・什器の名前 |
//! | `Project` | どこにも載せずプロジェクトに置いてある | 書かない |
//! | `Disposed` | 廃棄 | 書かない |
//!
//! **置けるのはこのプロジェクトの設備・什器と、このプロジェクトだけ。**1つの
//! 取込ファイルは1プロジェクトに閉じる（23.5）。他のプロジェクトへ移すのは
//! 移譲（11章）の担当である。設備・什器の上の位置（棚の段）は持たない。
//! 部品がどのプロジェクトのものかは、設備・什器の置き場所からたどる。
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
//! **突合の候補は、このプロジェクトに一度でもあった部品に限る**（23.5、機器の
//! A-6と同じ考え方、#144）。次のいずれかに当たる部品である。
//!
//! - このプロジェクトの機器に載ったことがある
//! - このプロジェクトの設備・什器に置かれたことがある（撤去した設備・什器を含む）
//! - このプロジェクトに置かれたことがある
//!
//! 候補外に同じベンダー＋シリアルがあれば、**新規作成せずエラーとする**——
//! 他プロジェクトの部品を書き換えることも、黙って2つ目を作ることもしない
//! （23.9.1と同根）。
//!
//! **候補でも、今は他のプロジェクトにある部品はエラーにする**（機器の#143と
//! 同じ）。移譲された部品を取込で引き戻すと、チケットを通らない移譲になる
//! （11章）。候補から外さないのは、外すと2つ目を作ってしまうため。
//!
//! 倉庫用のプロジェクトにある部品も、このプロジェクトに関わったことが無ければ候補外になる。
//! **倉庫からの払い出しは変更管理チケットの担当**であり（11章）、取込で
//! 引き込めると誰がいつ持ち出したのかが残らない。予備部品の初期投入は、
//! 倉庫用のプロジェクトの取込で「プロジェクトに置く」形で行う（16.1）。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{
    chassis_slot, configuration, device, mount_container, part_catalog, part_instance,
    part_instance_location, vendor,
};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
use serde::Deserialize;

use super::instances::{語彙, HEALTHS, STATUSES};
use super::placement::{機器の索引, 機器キー, 空ならnone, 読み取る};
use super::{理由, Entry, ImportError, Message, Outcome, Report};
use crate::repository::{Actor, AuditedTx};

const DEVICE: &str = "Device";
const MOUNT_CONTAINER: &str = "MountContainer";
const PROJECT: &str = "Project";
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
    /// `ok` / `failed`。空欄は `ok`。**機器の `health` とは独立**（設計書6.3）
    #[serde(default)]
    pub health: String,
    /// Device / MountContainer / Project / Disposed。
    pub location_type: String,
    /// `Device` のとき、載っている機器のホスト名。
    #[serde(default)]
    pub location_hostname: String,
    /// `MountContainer` のとき設備・什器の名前。
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
                // 対象の欄は訳さない（#214）。記号で示す
                "" => "—",
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
    health: String,
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
    let 索引 = 機器の索引::作る(db, project_id).await?;
    let 候補 = このプロジェクトに関わった部品(db, project_id, &索引.ids()).await?;
    // **倉庫プロジェクトでは既存の部品の `status` を動かさない**（#221、設計書6.3）
    let 倉庫プロジェクト = crate::setting::倉庫プロジェクトか(db, project_id).await?;

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
                理由!("import_detail.parts.serial_required"),
            ));
            continue;
        };

        let 鍵 = (row.part_vendor.trim().to_owned(), serial.to_owned());
        if let Some(前) = 既出.insert(鍵, i) {
            report.push(Entry::new(
                Outcome::Error,
                表示,
                理由!("import_detail.parts.duplicate_in_file", row = 前 + 2),
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
                report.push(Entry::new(
                    Outcome::Error,
                    表示,
                    理由!(
                        "import_detail.common.field_prefixed",
                        field = "status",
                        reason = 理由
                    ),
                ));
                continue;
            }
        };
        let health = match 語彙(&row.health, HEALTHS, crate::device_state::OK) {
            Ok(s) => s,
            Err(理由) => {
                report.push(Entry::new(
                    Outcome::Error,
                    表示,
                    理由!(
                        "import_detail.common.field_prefixed",
                        field = "health",
                        reason = 理由
                    ),
                ));
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
                let d = match 索引.引く(&key) {
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
            MOUNT_CONTAINER => {
                let Some(name) = 空ならnone(&row.location_name) else {
                    report.push(Entry::new(
                        Outcome::Error,
                        表示,
                        理由!("import_detail.parts.container_name_required"),
                    ));
                    continue;
                };
                // **このプロジェクトの設備・什器だけ。**撤去したものは置き場にしない
                // （名前の比較は大文字小文字を区別しない、#204）
                match crate::container::同じ名前の設備(db, PROJECT, project_id, name, None).await?
                {
                    Some(c) => (MOUNT_CONTAINER.to_owned(), Some(c.id), None),
                    None => {
                        report.push(Entry::new(
                            Outcome::Error,
                            表示,
                            理由!("import_detail.parts.container_not_found", name = name),
                        ));
                        continue;
                    }
                }
            }
            PROJECT => {
                // **置けるのはこのプロジェクトだけ。**名前を書けると、他の
                // プロジェクトへ移す取込に読めてしまう（23.5）
                if 空ならnone(&row.location_name).is_some() {
                    report.push(Entry::new(
                        Outcome::Error,
                        表示,
                        理由!("import_detail.parts.project_name_given"),
                    ));
                    continue;
                }
                (PROJECT.to_owned(), Some(project_id), None)
            }
            DISPOSED => (DISPOSED.to_owned(), None, None),
            // **倉庫は倉庫用のプロジェクトになった**（#220）。旧い値を黙って
            // 読み替えず、移し方を案内する
            "Warehouse" => {
                report.push(Entry::new(
                    Outcome::Error,
                    表示,
                    理由!("import_detail.common.warehouse_removed"),
                ));
                continue;
            }
            other => {
                report.push(Entry::new(
                    Outcome::Error,
                    表示,
                    理由!("import_detail.parts.location_type_invalid", value = other),
                ));
                continue;
            }
        };

        if location_type != DEVICE && 空ならnone(&row.chassis_slot).is_some() {
            report.push(Entry::new(
                Outcome::Error,
                表示,
                理由!("import_detail.parts.slot_only_device"),
            ));
            continue;
        }

        // 突合（ベンダー＋シリアル）
        let 既存 = match 突合(db, project_id, &候補, catalog.vendor_id, serial).await? {
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
        // **ケーブルが挿さったままの部品は、機器から外さない**（#229）。先にポート接続の
        // 画面で外させる。スロットだけの変更は機器に載ったままなので通す
        if let (Some(p), Some(l)) = (&既存, &現在) {
            let 同じ場所 = l.location_type == location_type && l.location_id == location_id;
            if !同じ場所 && crate::part_location::ケーブルが挿さっている(db, p.id).await?
            {
                report.push(Entry::new(
                    Outcome::Error,
                    表示,
                    理由!("import_detail.parts.has_cable", serial = serial),
                ));
                continue;
            }
        }

        // 倉庫にある間の `status` には意味が無い。比べず、書き換えもしない。
        // **黙って捨てない**——書いてあれば行の説明に添える
        let mut 説明 = Message::default();
        let status = match &既存 {
            Some(p) if 倉庫プロジェクト && p.status != status => {
                if !row.status.trim().is_empty() {
                    説明 = 理由!("import_detail.common.warehouse_status_ignored");
                }
                p.status.clone()
            }
            _ => status,
        };
        let 属性が同じ = 既存.as_ref().is_some_and(|p| {
            p.part_catalog_id == catalog.id && p.status == status && p.health == health
        });

        let outcome = match (&既存, 所在が同じ && 属性が同じ) {
            (None, _) => Outcome::Created,
            (Some(_), true) => Outcome::Unchanged,
            (Some(_), false) => Outcome::Updated,
        };
        report.push(Entry::new(outcome, 表示, 説明));
        if outcome == Outcome::Unchanged {
            continue;
        }

        planned.push(部品の計画 {
            既存,
            part_catalog_id: catalog.id,
            serial_number: serial.to_owned(),
            status,
            health,
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
) -> Result<Result<part_catalog::Model, Message>, sea_orm::DbErr> {
    let vendor_name = row.part_vendor.trim();
    let part_number = row.part_number.trim();

    let Some(v) = vendor::Entity::find()
        .filter(vendor::Column::Name.eq(vendor_name))
        .one(db)
        .await?
    else {
        return Ok(Err(理由!(
            "import_detail.common.vendor_not_found",
            vendor = vendor_name
        )));
    };
    // **統合で吸収されたベンダーは統合先で引き直す**（23.9.4）
    let vendor_id = v.merged_into_vendor_id.unwrap_or(v.id);

    let Some(mut c) = part_catalog::Entity::find()
        .filter(part_catalog::Column::VendorId.eq(vendor_id))
        .filter(part_catalog::Column::PartNumber.eq(part_number))
        .one(db)
        .await?
    else {
        return Ok(Err(理由!(
            "import_detail.parts.catalog_not_found",
            vendor = vendor_name,
            part_number = part_number
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
) -> Result<Result<i32, Message>, sea_orm::DbErr> {
    let Some(configuration_id) = d.configuration_id else {
        return Ok(Err(理由!(
            "import_detail.parts.no_configuration",
            hostname = &d.hostname
        )));
    };
    let Some(c) = configuration::Entity::find_by_id(configuration_id)
        .one(db)
        .await?
    else {
        return Ok(Err(理由!("import_detail.parts.configuration_missing")));
    };

    let slot = chassis_slot::Entity::find()
        .filter(chassis_slot::Column::ChassisModelId.eq(c.chassis_model_id))
        .filter(chassis_slot::Column::SlotLabel.eq(label))
        .one(db)
        .await?;
    Ok(match slot {
        Some(s) => Ok(s.id),
        None => Err(理由!(
            "import_detail.parts.slot_not_found",
            hostname = &d.hostname,
            label = label
        )),
    })
}

/// このプロジェクトに一度でもあった部品（23.5、A-6、#144）。
///
/// このプロジェクトの機器に載ったことがある・このプロジェクトの設備・什器に
/// 置かれたことがある・このプロジェクトに置かれたことがある部品。`機器id` は
/// このプロジェクトに属したことのある機器（[`機器の索引`]）。
pub(super) async fn このプロジェクトに関わった部品<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    機器id: &[i32],
) -> Result<Vec<part_instance::Model>, sea_orm::DbErr> {
    // **撤去した設備・什器も含める。**置かれていた履歴は事実として残る
    let 設備id: Vec<i32> = mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .all(db)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();

    let mut 条件 = Condition::any().add(
        Condition::all()
            .add(part_instance_location::Column::LocationType.eq(PROJECT))
            .add(part_instance_location::Column::LocationId.eq(project_id)),
    );
    if !機器id.is_empty() {
        条件 = 条件.add(
            Condition::all()
                .add(part_instance_location::Column::LocationType.eq(DEVICE))
                .add(part_instance_location::Column::LocationId.is_in(機器id.to_vec())),
        );
    }
    if !設備id.is_empty() {
        条件 = 条件.add(
            Condition::all()
                .add(part_instance_location::Column::LocationType.eq(MOUNT_CONTAINER))
                .add(part_instance_location::Column::LocationId.is_in(設備id)),
        );
    }

    let mut ids: Vec<i32> = part_instance_location::Entity::find()
        .filter(条件)
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
    project_id: i32,
    候補: &[part_instance::Model],
    vendor_id: i32,
    serial: &str,
) -> Result<Result<Option<part_instance::Model>, Message>, sea_orm::DbErr> {
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
            if !候補.iter().any(|c| c.id == p.id) {
                return Ok(Err(理由!(
                    "import_detail.parts.never_in_project",
                    serial = serial
                )));
            }
            // **今は他のプロジェクトにある部品には触れない**（#143 の機器と同じ）
            match crate::part_location::部品の現在のプロジェクト(db, p.id).await? {
                Some(今) if 今 != project_id => Ok(Err(理由!(
                    "import_detail.parts.in_other_project",
                    serial = serial
                ))),
                _ => Ok(Ok(Some(p))),
            }
        }
        n => Ok(Err(理由!(
            "import_detail.parts.ambiguous",
            serial = serial,
            count = n
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
                active.health = Set(p.health.clone());
                active.updated_at = Set(now);
                tx.update(existing, active).await?;
                existing.id
            }
            None => {
                tx.insert(part_instance::ActiveModel {
                    part_catalog_id: Set(p.part_catalog_id),
                    serial_number: Set(Some(p.serial_number.clone())),
                    status: Set(p.status.clone()),
                    health: Set(p.health.clone()),
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
             SN-2,Samsung,M393A4K40DB3,,MountContainer,,予備棚,\n",
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].chassis_slot, "DIMM_A1");
        assert_eq!(rows[1].location_name, "予備棚");
    }

    #[test]
    fn シリアルが無ければ表示でそう示す() {
        let rows =
            parse_parts("serial_number,part_vendor,part_number,location_type\n,Samsung,X,Device\n")
                .unwrap();
        assert!(rows[0].表示名().contains("(—)"));
    }
}
