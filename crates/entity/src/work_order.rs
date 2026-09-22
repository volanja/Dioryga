//! 変更管理チケット（設計書11章）。
//!
//! **`status` の `approved` は導出結果の書き戻しではない。**承認そのものは
//! [`super::work_order_approval`] の行が真実の源であり、紐づく**全**行が
//! `approved` になった時点でここを遷移させる。個々の承認をここから読み取ろう
//! とすると、承認テーブルとずれる。
//!
//! **`work_order_id` を持つ履歴テーブルからの外部キーは存在しない**（24.3）。
//! SQLiteが後から制約を足せないため。参照整合はアプリケーション層と
//! `dioryga check`（24.5）で担保する。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "work_order")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 起票元プロジェクト。
    pub project_id: i32,
    /// `work_type = Transfer` の移譲先。それ以外では `None`。
    pub target_project_id: Option<i32>,
    pub device_id: Option<i32>,
    pub part_instance_id: Option<i32>,
    /// Repair / Addition / Relocation / Disposal / Transfer
    pub work_type: String,
    pub title: String,
    pub description: String,
    /// 主担当。副担当は任意（11章）。
    pub primary_assignee_id: Option<i32>,
    pub secondary_assignee_id: Option<i32>,
    pub due_date: Option<Date>,
    /// planned / approved / in_progress / completed / cancelled
    pub status: String,
    /// **予定と実績を別の列で持つ。**上書きすると「当初いつの予定だったか」が
    /// 失われ、QCDの「D」を定量的に見られなくなる（5.1）。
    pub planned_at: Option<DateTimeUtc>,
    pub executed_at: Option<DateTimeUtc>,
    pub completed_at: Option<DateTimeUtc>,
    pub cancelled_at: Option<DateTimeUtc>,
    pub cancelled_reason: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
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
    #[sea_orm(has_many = "super::work_order_approval::Entity")]
    Approval,
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

impl Related<super::work_order_approval::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Approval.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
