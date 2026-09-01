//! 機器の搭載位置（設計書12.2、13.2）— 履歴。
//!
//! **`container_id` と `host_device_id` はどちらか一方だけが埋まる。**
//! 什器に直接載るのか、棚板やハイパーバイザのような他の機器の上に載るのかを
//! 表し分ける。排他はDB制約にせず、アプリケーション層で検証する。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "device_mount")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub device_id: i32,
    /// 什器に直接搭載する場合。
    pub container_id: Option<i32>,
    /// Rack なら開始U番号、Shelving なら段番号。
    pub position: Option<i32>,
    /// Left / Right / Full
    pub horizontal_position: Option<String>,
    /// Front / Rear / Full
    pub depth_position: Option<String>,
    /// 他の機器の上に載る場合（棚板、VMの実行ホスト）。
    pub host_device_id: Option<i32>,
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
        belongs_to = "super::mount_container::Entity",
        from = "Column::ContainerId",
        to = "super::mount_container::Column::Id"
    )]
    MountContainer,
}

impl Related<super::device::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Device.def()
    }
}

impl Related<super::mount_container::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MountContainer.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
