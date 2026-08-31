//! ベンダーのマスタ（設計書18.3）。
//!
//! **名称の表記ゆれを防ぐために存在する。**自由入力を許すと同一ベンダーが
//! 複数の綴りで登録され、横断的な集計ができなくなる。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "vendor")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 表記ゆれを防ぐため一意。
    pub name: String,
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
