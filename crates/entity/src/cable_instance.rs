//! ケーブルの実物（設計書8.3）。
//!
//! **`status` を持たない**（設計書8.4）。所在・接続・健全性・存在の軸が1列に
//! 混ざるため、導出できない「使えなくなった」だけを `retired_at` で持つ。

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
    /// 使えなくなった日。故障と廃棄は区別しない（設計書8.4）
    pub retired_at: Option<DateTimeUtc>,
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
