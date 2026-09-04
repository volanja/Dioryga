//! マイルストーンと機器の対応（設計書10.4）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "milestone_device")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub milestone_id: i32,
    pub device_id: i32,
    /// addition / relocation / removal
    pub change_type: String,
    /// **WORK_ORDER未作成のため外部キーは未設定。**
    pub work_order_id: Option<i32>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::milestone::Entity",
        from = "Column::MilestoneId",
        to = "super::milestone::Column::Id"
    )]
    Milestone,
    #[sea_orm(
        belongs_to = "super::device::Entity",
        from = "Column::DeviceId",
        to = "super::device::Column::Id"
    )]
    Device,
}

impl Related<super::milestone::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Milestone.def()
    }
}

impl Related<super::device::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Device.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
