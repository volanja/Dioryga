//! 機器を載せる什器（設計書12章）。
//!
//! `location_type` / `location_id` は多態的参照であり、DBの外部キー制約を
//! 持てない（旧C-7）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mount_container")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    /// Rack / Desk / Shelving
    pub container_type: String,
    /// Warehouse / Project
    pub location_type: String,
    pub location_id: i32,
    /// ラックのU数、棚の段数。**超過はエラーではなく警告**として扱う（不変条件6）。
    pub capacity: Option<i32>,
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
