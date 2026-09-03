//! インターフェースの役割（設計書8.5）— 履歴。
//!
//! バックアップ用・vMotion用など目的別の区別。セキュリティ境界を表す
//! `zone`（VLAN / SUBNET）とは別軸である。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "interface_role")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub os_interface_id: i32,
    pub role: String,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::os_interface::Entity",
        from = "Column::OsInterfaceId",
        to = "super::os_interface::Column::Id"
    )]
    OsInterface,
}

impl Related<super::os_interface::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OsInterface.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
