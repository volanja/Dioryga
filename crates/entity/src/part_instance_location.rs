//! 部品の所在（設計書6.2、12.4）— 履歴。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "part_instance_location")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub part_instance_id: i32,
    /// Warehouse / Device / Disposed
    pub location_type: String,
    pub location_id: Option<i32>,
    /// **任意項目。**どのスロットに挿さっているかまでは求めない（6.2）。
    pub chassis_slot_id: Option<i32>,
    pub work_order_id: Option<i32>,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::part_instance::Entity",
        from = "Column::PartInstanceId",
        to = "super::part_instance::Column::Id"
    )]
    PartInstance,
    #[sea_orm(
        belongs_to = "super::chassis_slot::Entity",
        from = "Column::ChassisSlotId",
        to = "super::chassis_slot::Column::Id"
    )]
    ChassisSlot,
}

impl Related<super::part_instance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PartInstance.def()
    }
}

impl Related<super::chassis_slot::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChassisSlot.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
