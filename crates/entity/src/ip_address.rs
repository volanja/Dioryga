//! IPアドレス（設計書8.3、8.6、14章）— 履歴。
//!
//! **`vlan_id` を持たない。**インターフェース側から辿る（8.6で廃止）。
//! 同一サブネット内の重複は部分インデックスで禁じている（14.2）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ip_address")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub os_interface_id: i32,
    pub ip_address: String,
    pub prefix_length: i32,
    pub subnet_id: Option<i32>,
    /// この変更を引き起こしたWORK_ORDER（11章）。**外部キーは張らない**（24.3）。
    pub work_order_id: Option<i32>,
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
    #[sea_orm(
        belongs_to = "super::subnet::Entity",
        from = "Column::SubnetId",
        to = "super::subnet::Column::Id"
    )]
    Subnet,
}

impl Related<super::os_interface::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OsInterface.def()
    }
}

impl Related<super::subnet::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Subnet.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
