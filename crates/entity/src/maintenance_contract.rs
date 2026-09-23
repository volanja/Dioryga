//! 保守契約（設計書10章）。
//!
//! **保守期限は `end_date` のみが正**（旧B-2）。機器側に保守期限を持たない。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "maintenance_contract")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub contract_number: String,
    pub vendor_id: i32,
    pub start_date: Date,
    pub end_date: Date,
    /// **最小通貨単位の整数**（24.2.1）。SQLiteにDECIMALが無く、REALでは
    /// 丸め誤差が累積するため。桁数は `PROJECT.currency` から決まる。
    pub amount: i64,
    pub quote_contact: String,
    pub failure_contact: String,
    /// 契約自体の発注番号。**自由入力**（10.2。発注のテーブルは持たない）。
    pub order_number: Option<String>,
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
