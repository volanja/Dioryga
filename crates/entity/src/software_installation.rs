//! ソフトウェアのインストール — **履歴テーブル**（設計書9.4）。
//!
//! `to_date IS NULL` が現在有効な行であり、既存行を更新せず「閉じて開く」
//! （不変条件1）。**1つのインスタンスが同時に2台へ入ることはない**ため、
//! 現行行に部分ユニークを張っている。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "software_installation")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub software_instance_id: i32,
    pub device_id: i32,
    /// この変更を引き起こしたWORK_ORDER（11章）。
    /// **先行して作った履歴テーブルと違い、ここは外部キーを張れている**（24.3）。
    pub work_order_id: Option<i32>,
    pub from_date: DateTimeUtc,
    /// `None` が現在有効な行。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::software_instance::Entity",
        from = "Column::SoftwareInstanceId",
        to = "super::software_instance::Column::Id"
    )]
    SoftwareInstance,
    #[sea_orm(
        belongs_to = "super::device::Entity",
        from = "Column::DeviceId",
        to = "super::device::Column::Id"
    )]
    Device,
    #[sea_orm(
        belongs_to = "super::work_order::Entity",
        from = "Column::WorkOrderId",
        to = "super::work_order::Column::Id"
    )]
    WorkOrder,
    #[sea_orm(has_many = "super::software_role_assignment::Entity")]
    Role,
}

impl Related<super::software_instance::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SoftwareInstance.def()
    }
}

impl Related<super::device::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Device.def()
    }
}

impl Related<super::software_role_assignment::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Role.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
