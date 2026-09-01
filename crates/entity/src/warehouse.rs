//! 倉庫（設計書12章）。
//!
//! プロジェクトを横断する。未割当の機器の置き場であり、
//! `DEVICE_ASSIGNMENT.location_type="Warehouse"` から参照される。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "warehouse")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub address: String,
    pub created_by: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
