//! 部品の所在からたどる問い（設計書6.2、12.4、16.1）。
//!
//! **部品はプロジェクトを列に持たない。**どのプロジェクトのものかは置き場所から
//! たどる（#219）。機器に載っていれば機器の所属、設備・什器に置いてあれば
//! 設備・什器の置き場所、プロジェクトに置いてあればそのもの。廃棄した部品は
//! どのプロジェクトにも無い。倉庫は倉庫用のプロジェクトである（#196）。
//!
//! 部品の廃棄（`server/work_order.rs`）・部品の取込（`import/parts.rs`）・
//! 部品の画面（`server/part_instance.rs`）が同じ答えを使う。

use entity::{cable_connection, device_assignment, mount_container, part_instance_location};
use sea_orm::{ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, QueryFilter};

/// `PART_INSTANCE_LOCATION.location_type`（6.2、#219）。
pub const DEVICE: &str = "Device";
pub const MOUNT_CONTAINER: &str = "MountContainer";
pub const PROJECT: &str = "Project";

/// 部品がいまあるプロジェクト。プロジェクトに無ければ `None`。
pub async fn 部品の現在のプロジェクト<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
) -> Result<Option<i32>, DbErr> {
    let Some(所在) = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(part_instance_id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let Some(location_id) = 所在.location_id else {
        return Ok(None);
    };
    match 所在.location_type.as_str() {
        DEVICE => Ok(device_assignment::Entity::find()
            .filter(device_assignment::Column::DeviceId.eq(location_id))
            .filter(device_assignment::Column::ToDate.is_null())
            .one(db)
            .await?
            .filter(|a| a.location_type == PROJECT)
            .and_then(|a| a.location_id)),
        MOUNT_CONTAINER => Ok(mount_container::Entity::find_by_id(location_id)
            .one(db)
            .await?
            .filter(|c| c.location_type == PROJECT)
            .map(|c| c.location_id)),
        PROJECT => Ok(Some(location_id)),
        _ => Ok(None),
    }
}

/// 一度に `IN` へ渡す件数。
const まとめて引く件数: usize = 500;

/// このプロジェクトに今ある部品の、現行の所在の行（#219、#229）。
///
/// このプロジェクトの機器に載っている・このプロジェクトの設備・什器に置いてある・
/// このプロジェクトに置いてある部品。[`部品の現在のプロジェクト`] と同じたどり方を、
/// プロジェクトの側から行う。
pub async fn プロジェクトの部品の所在<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> Result<Vec<part_instance_location::Model>, DbErr> {
    let 機器: Vec<i32> = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(db)
        .await?
        .into_iter()
        .map(|a| a.device_id)
        .collect();
    let 設備: Vec<i32> = mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .all(db)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();

    let mut out = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::ToDate.is_null())
        .filter(part_instance_location::Column::LocationType.eq(PROJECT))
        .filter(part_instance_location::Column::LocationId.eq(project_id))
        .all(db)
        .await?;
    for (種類, ids) in [(DEVICE, 機器), (MOUNT_CONTAINER, 設備)] {
        for 組 in ids.chunks(まとめて引く件数) {
            out.extend(
                part_instance_location::Entity::find()
                    .filter(part_instance_location::Column::ToDate.is_null())
                    .filter(
                        Condition::all()
                            .add(part_instance_location::Column::LocationType.eq(種類))
                            .add(part_instance_location::Column::LocationId.is_in(組.to_vec())),
                    )
                    .all(db)
                    .await?,
            );
        }
    }
    Ok(out)
}

/// 部品に、現行のケーブルの接続があるか（#225、#229）。
///
/// **挿さったままの部品は動かさない。**移譲・廃棄・置き場所の移動・取込が
/// 同じ答えを使う。自動では外さない。
pub async fn ケーブルが挿さっている<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
) -> Result<bool, DbErr> {
    Ok(cable_connection::Entity::find()
        .filter(cable_connection::Column::PartInstanceId.eq(part_instance_id))
        .filter(cable_connection::Column::ToDate.is_null())
        .one(db)
        .await?
        .is_some())
}
