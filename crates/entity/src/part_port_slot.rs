//! 部品が備えるポート（設計書8.3、8.7）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "part_port_slot")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub part_catalog_id: i32,
    /// Network / Power / Stack
    pub port_kind: String,
    pub port_label: String,
    pub connector_type: String,
    /// port_kind=Network のときのみ意味を持つ。
    pub port_speed: Option<String>,
    /// 定格電圧の下限（V）。**`port_kind=Power` のときのみ意味を持つ**（12.7）。
    ///
    /// **範囲で持つのは「100-240V対応」と「200V専用」を区別するため**であり、
    /// 単一値では表せない。
    pub voltage_min: Option<i32>,
    /// 定格電圧の上限（V）。同上。
    pub voltage_max: Option<i32>,
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
