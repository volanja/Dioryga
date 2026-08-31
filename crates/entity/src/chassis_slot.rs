//! 筐体モデルが持つスロット（設計書6.2）。
//!
//! **無条件のスロット一覧である。**「4CPU構成でなければ使えない」といった
//! 条件付きの制約は表現しない（6.1で対応しないと決着済み）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "chassis_slot")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub chassis_model_id: i32,
    /// CPU_SOCKET / DIMM / DRIVE_BAY / PCIE / PSU_BAY
    pub slot_type: String,
    pub slot_label: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chassis_model::Entity",
        from = "Column::ChassisModelId",
        to = "super::chassis_model::Column::Id"
    )]
    ChassisModel,
}

impl Related<super::chassis_model::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChassisModel.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
