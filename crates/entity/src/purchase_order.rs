//! 発注（設計書10章）。
//!
//! **`amount` を持たない。**明細の合計から計算する（旧B-5）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "purchase_order")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub order_number: String,
    pub order_date: Date,
    pub vendor_id: i32,
    pub currency: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::vendor::Entity",
        from = "Column::VendorId",
        to = "super::vendor::Column::Id"
    )]
    Vendor,
}

impl Related<super::vendor::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vendor.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
