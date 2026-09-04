//! 定期費用（設計書10章）。ラック料金・回線費用など。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "recurring_cost")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// MountContainer / Project
    pub item_type: String,
    pub item_id: i32,
    pub cost_type: String,
    pub vendor_id: Option<i32>,
    /// **最小通貨単位の整数**（24.2.1）。SQLiteにDECIMALが無く、REALでは
    /// 丸め誤差が累積するため。桁数は `PROJECT.currency` から決まる。
    pub amount: i64,
    /// Monthly / Annual
    pub billing_cycle: String,
    pub start_date: Date,
    /// null = 継続中。
    pub end_date: Option<Date>,
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
