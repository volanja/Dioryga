//! 機器・部品の状態（設計書6.3、#221）。
//!
//! 運用の段階（`status`）と故障の有無（`health`）を別の列に持つ。冗長構成の
//! 片側が壊れても、修理まで運用は続くからである。**機器と部品で同じ語彙を使う。**
//!
//! **修理中は列に持たない。**未完了の修理（`Repair`）のチケットが対象にあるか
//! どうかから導出する（[`容体を求める`]）。列にも持つと、チケットを完了したのに
//! 修理中のまま、という食い違いが起きる。

use std::collections::{HashMap, HashSet};

use entity::{device, part_instance, part_instance_location, work_order};
use sea_orm::{ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, QueryFilter};

pub const PLANNED: &str = "planned";
pub const PROVISIONING: &str = "provisioning";
pub const RUNNING: &str = "running";
/// 構築済みで、すぐ使える予備（ホット・コールドスタンバイ）。**保管中の意味ではない。**
pub const STANDBY: &str = "standby";

pub const OK: &str = "ok";
pub const FAILED: &str = "failed";

/// `status` の語彙。
pub const STATUSES: &[&str] = &[PLANNED, PROVISIONING, RUNNING, STANDBY];
/// 登録と編集で選べる `status`。**`planned` を含まない**——予約は増設の変更管理
/// チケットが作る（設計書11.6）。画面からも作れると、同じ状態を作る経路が2つになる。
pub const STATUSES_ON_EDIT: &[&str] = &[PROVISIONING, RUNNING, STANDBY];
/// `health` の語彙。
pub const HEALTHS: &[&str] = &[OK, FAILED];

const REPAIR: &str = "Repair";
const DEVICE: &str = "Device";
/// 未完了のチケットの状態（11章）。
const 未完了: &[&str] = &["planned", "approved", "in_progress"];

/// 機器の容体。`health` と、載っている部品と、修理のチケットから導く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum 容体 {
    正常,
    /// 機器が `failed`、または載っている部品に `failed` がある。
    故障,
    /// 故障していて、未完了の修理のチケットが機器か載っている部品にある。
    修理中,
}

impl 容体 {
    /// 対応が必要か（故障か修理中）。
    pub fn 要対応(self) -> bool {
        self != Self::正常
    }

    /// 表示灯のクラスと表示名に使う値（`device_statuses.*`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::正常 => OK,
            Self::故障 => FAILED,
            Self::修理中 => "repairing",
        }
    }
}

/// 機器ごとの容体を求める（設計書6.3）。
///
/// **機器と部品の `health` は独立に持ち、ここで合わせる。**部品を `failed` に
/// しても機器の `health` は書き換えないため、載っている部品も見る。
///
/// **機器ごとに問い合わせない**（22章 R-4）。IDをまとめて3回で引く。
pub async fn 容体を求める<C: ConnectionTrait>(
    db: &C,
    devices: &[device::Model],
) -> Result<HashMap<i32, 容体>, DbErr> {
    let ids: Vec<i32> = devices.iter().map(|d| d.id).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    // 載っている部品 → 機器
    let 部品の載せ先: HashMap<i32, i32> = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(DEVICE))
        .filter(part_instance_location::Column::LocationId.is_in(ids.clone()))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(db)
        .await?
        .into_iter()
        .filter_map(|l| l.location_id.map(|d| (l.part_instance_id, d)))
        .collect();
    let 部品: Vec<i32> = 部品の載せ先.keys().copied().collect();

    let 故障した部品を載せた機器: HashSet<i32> = if 部品.is_empty() {
        HashSet::new()
    } else {
        part_instance::Entity::find()
            .filter(part_instance::Column::Id.is_in(部品.clone()))
            .filter(part_instance::Column::Health.eq(FAILED))
            .all(db)
            .await?
            .into_iter()
            .filter_map(|p| 部品の載せ先.get(&p.id).copied())
            .collect()
    };

    // 未完了の修理。**機器が対象のものと、載っている部品が対象のもの**の両方
    let mut 対象 = Condition::any().add(work_order::Column::DeviceId.is_in(ids.clone()));
    if !部品.is_empty() {
        対象 = 対象.add(work_order::Column::PartInstanceId.is_in(部品));
    }
    let 修理中の機器: HashSet<i32> = work_order::Entity::find()
        .filter(work_order::Column::WorkType.eq(REPAIR))
        .filter(work_order::Column::Status.is_in(未完了.iter().copied()))
        .filter(対象)
        .all(db)
        .await?
        .into_iter()
        .filter_map(|w| {
            w.device_id.filter(|d| ids.contains(d)).or_else(|| {
                w.part_instance_id
                    .and_then(|p| 部品の載せ先.get(&p).copied())
            })
        })
        .collect();

    Ok(devices
        .iter()
        .map(|d| {
            let 故障 = d.health == FAILED || 故障した部品を載せた機器.contains(&d.id);
            let v = match (故障, 修理中の機器.contains(&d.id)) {
                (false, _) => 容体::正常,
                (true, true) => 容体::修理中,
                (true, false) => 容体::故障,
            };
            (d.id, v)
        })
        .collect())
}
