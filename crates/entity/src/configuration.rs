//! 筐体モデルに対する構成（設計書6.2）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "configuration")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub chassis_model_id: i32,
    pub name: String,
    pub created_by: i32,
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
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::chassis_model::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChassisModel.def()
    }
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
