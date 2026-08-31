//! 構成に含まれる部品と数量（設計書6.2）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "configuration_part")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub configuration_id: i32,
    pub part_catalog_id: i32,
    /// 同じ部品を複数行に分けず、この数量で表す。
    pub quantity: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::configuration::Entity",
        from = "Column::ConfigurationId",
        to = "super::configuration::Column::Id"
    )]
    Configuration,
    #[sea_orm(
        belongs_to = "super::part_catalog::Entity",
        from = "Column::PartCatalogId",
        to = "super::part_catalog::Column::Id"
    )]
    PartCatalog,
}

impl Related<super::configuration::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Configuration.def()
    }
}

impl Related<super::part_catalog::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PartCatalog.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
