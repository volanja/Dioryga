//! SBOMの取込記録（設計書9.5）。
//!
//! **`superseded_at` は他の履歴テーブルの `to_date` と同じ役割**を果たす。
//! 「そのDeviceの現在の観測結果」は `superseded_at IS NULL` で引ける。
//!
//! **SBOM取込は `SOFTWARE_INSTANCE` を自動生成しない**（9.6）。観測記録であり、
//! 資産として管理するかを決めるのは人間の判断である。**`VENDOR` も作らない**
//! （18.3のマスタが数千件のサプライヤ名で汚染されるため）。
//!
//! 3章のクロスプロジェクト可視性は `device_id` を経由して適用される。
//! スナップショット自体が複数プロジェクトの機器間で共有されていても、
//! **閲覧できるのは自分が権限を持つDeviceの取込行を通じてのみ**である（9.8）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sbom_import")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub device_id: i32,
    pub content_hash: String,
    /// CycloneDX / SPDX
    pub source_format: String,
    pub work_order_id: Option<i32>,
    pub imported_by: i32,
    pub imported_at: DateTimeUtc,
    /// `None` = そのDeviceの最新の観測結果。
    pub superseded_at: Option<DateTimeUtc>,
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
        belongs_to = "super::sbom_snapshot::Entity",
        from = "Column::ContentHash",
        to = "super::sbom_snapshot::Column::ContentHash"
    )]
    Snapshot,
    #[sea_orm(
        belongs_to = "super::work_order::Entity",
        from = "Column::WorkOrderId",
        to = "super::work_order::Column::Id"
    )]
    WorkOrder,
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::ImportedBy",
        to = "super::app_user::Column::Id"
    )]
    ImportedBy,
    #[sea_orm(has_many = "super::sbom_component_change::Entity")]
    Change,
}

impl Related<super::device::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Device.def()
    }
}

impl Related<super::sbom_snapshot::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Snapshot.def()
    }
}

impl Related<super::sbom_component_change::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Change.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
