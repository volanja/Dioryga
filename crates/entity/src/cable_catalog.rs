//! ケーブルのカタログ（設計書8.3）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cable_catalog")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub cable_type: String,
    /// **ミリメートルの整数。**SQLiteに DECIMAL が無いため（24.2.1）。
    /// 単位を列名に含めているのは1000倍の取り違えを防ぐため。
    pub length_mm: Option<i32>,
    pub color: String,
    pub vendor_id: Option<i32>,
    pub part_number: Option<String>,
    pub created_by: i32,
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
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::vendor::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vendor.def()
    }
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
