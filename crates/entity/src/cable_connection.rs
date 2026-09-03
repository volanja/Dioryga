//! ケーブルの接続（設計書8.3）— 履歴。
//!
//! **`device_id` を持たない。**`PART_INSTANCE_LOCATION` 経由で導出する。
//! 持つと部品の移設時に二重管理になる。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cable_connection")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub cable_instance_id: i32,
    /// どちらの端か。両端が非対称なケーブルがあるため（8.7）。
    pub cable_end_slot_id: i32,
    pub part_instance_id: i32,
    pub port_slot_id: i32,
    /// この変更を引き起こしたWORK_ORDER（11章）。**未作成のため外部キーは未設定。**
    pub work_order_id: Option<i32>,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cable_instance::Entity",
        from = "Column::CableInstanceId",
        to = "super::cable_instance::Column::Id"
    )]
    CableInstance,
    #[sea_orm(
        belongs_to = "super::cable_end_slot::Entity",
        from = "Column::CableEndSlotId",
        to = "super::cable_end_slot::Column::Id"
    )]
    CableEndSlot,
    #[sea_orm(
        belongs_to = "super::part_instance::Entity",
        from = "Column::PartInstanceId",
        to = "super::part_instance::Column::Id"
    )]
    PartInstance,
    #[sea_orm(
        belongs_to = "super::part_port_slot::Entity",
        from = "Column::PortSlotId",
        to = "super::part_port_slot::Column::Id"
    )]
    PartPortSlot,
}

impl Related<super::cable_instance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CableInstance.def()
    }
}

impl Related<super::cable_end_slot::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CableEndSlot.def()
    }
}

impl Related<super::part_instance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PartInstance.def()
    }
}

impl Related<super::part_port_slot::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PartPortSlot.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
