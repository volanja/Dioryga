//! 発注明細（設計書10章）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "purchase_order_item")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub purchase_order_id: i32,
    /// Device / PartInstance / SoftwareInstance。多態的参照のため外部キーを持てない。
    pub item_type: String,
    pub item_id: i32,
    pub quantity: i32,
    /// **最小通貨単位の整数**（24.2.1）。SQLiteにDECIMALが無く、REALでは
    /// 丸め誤差が累積するため。桁数は `PROJECT.currency` から決まる。
    pub unit_price: i64,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::purchase_order::Entity",
        from = "Column::PurchaseOrderId",
        to = "super::purchase_order::Column::Id"
    )]
    PurchaseOrder,
}

impl Related<super::purchase_order::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PurchaseOrder.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
