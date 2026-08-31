//! ケーブルの端（設計書8.3、8.7）。
//!
//! **端ごとに異なるコネクタを持てる。**NEMA 5-15P と C13、LC と SC のような
//! 非対称なケーブルを表すために、両端を別レコードにしている。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cable_end_slot")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub cable_catalog_id: i32,
    /// A / B、またはブレイクアウトの Trunk / Branch1..N
    pub end_label: String,
    pub connector_type: String,
    pub port_speed: Option<String>,
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
