//! OSから見えるインターフェース（設計書8.3、8.5）— 履歴。
//!
//! **物理ポートではなくこちらを中心に置く。**ボンド・VLANサブインターフェース・
//! SVI・VMの仮想NICは物理ポートに1対1で対応しない（8.3）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "os_interface")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// **必須。**物理ポートを持たないインターフェースがあり、ポートから
    /// 機器を導出できないため（8.3）。
    pub device_id: i32,
    /// Physical / Bond / Vlan / Svi / Bridge / Virtual
    pub interface_type: String,
    /// interface_type=Physical のときのみ。
    pub part_instance_id: Option<i32>,
    /// 同上。
    pub port_slot_id: Option<i32>,
    /// ens1f0, bond0, bond0.100, Eth1/1/1, Vlan100
    pub os_interface_name: String,
    /// LACP / Static / ActiveBackup。interface_type=Bond のみ。
    pub aggregation_mode: Option<String>,
    /// この変更を引き起こしたWORK_ORDER（11章）。**外部キーは張らない**（24.3）。
    pub work_order_id: Option<i32>,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::device::Entity",
        from = "Column::DeviceId",
        to = "super::device::Column::Id"
    )]
    Device,
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

impl Related<super::device::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Device.def()
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
