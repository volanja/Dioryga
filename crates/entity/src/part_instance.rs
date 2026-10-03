//! 部品の実物（設計書6.2）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "part_instance")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub part_catalog_id: i32,
    pub serial_number: Option<String>,
    /// 機器と同じ語彙。planned / provisioning / running / standby（設計書6.3）
    pub status: String,
    /// ok / failed。**機器の `health` とは独立に持つ**（設計書6.3）
    pub health: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::part_catalog::Entity",
        from = "Column::PartCatalogId",
        to = "super::part_catalog::Column::Id"
    )]
    PartCatalog,
}

impl Related<super::part_catalog::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PartCatalog.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
