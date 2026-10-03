//! 部品の所在からたどる問い（設計書6.2、12.4、16.1）。
//!
//! **部品はプロジェクトを列に持たない。**どのプロジェクトのものかは置き場所から
//! たどる（#219）。機器に載っていれば機器の所属、設備・什器に置いてあれば
//! 設備・什器の置き場所、プロジェクトに置いてあればそのもの。倉庫・廃棄は
//! どのプロジェクトにも無い。
//!
//! 部品の廃棄（`server/work_order.rs`）と部品の取込（`import/parts.rs`）が
//! 同じ答えを使う。

use entity::{device_assignment, mount_container, part_instance_location};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};

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
