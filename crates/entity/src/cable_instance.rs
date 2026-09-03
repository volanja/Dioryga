//! ケーブルの実物（設計書8.3）。
//!
//! **機器と違い所在の履歴を持たない**ため、`in_stock` / `disposed` も
//! `status` に含む。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cable_instance")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub cable_catalog_id: i32,
    pub serial_number: Option<String>,
    pub asset_number: Option<String>,
    /// in_stock / in_use / broken / disposed
    pub status: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cable_catalog::Entity",
        from = "Column::CableCatalogId",
        to = "super::cable_catalog::Column::Id"
    )]
    CableCatalog,
}

impl Related<super::cable_catalog::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CableCatalog.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
