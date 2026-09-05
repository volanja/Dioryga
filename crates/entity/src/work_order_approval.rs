//! 変更管理チケットの承認（設計書11章）。
//!
//! 影響を受けるプロジェクトごとに1行を起こす。他プロジェクトの機器を巻き込む
//! 変更（移設・移譲）では承認が複数必要になる。
//!
//! **自己承認の禁止（旧C-9）はDB制約にできない。**比較対象が別テーブルの列
//! （`WORK_ORDER.primary_assignee_id`）になるため。登録時に確かめること。
//! ただし**承認者が自分しかいない場合は許可し、監査ログに明示的に記録する**
//! （22章）。一人で運用する現場で詰まないようにするため。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "work_order_approval")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub work_order_id: i32,
    /// どのプロジェクトの承認か。
    pub required_project_id: i32,
    /// 承認前は `None`。誰が承認したかは押した時点で決まる。
    pub approver_id: Option<i32>,
    /// pending / approved / rejected
    pub status: String,
    pub approved_at: Option<DateTimeUtc>,
    /// **承認者が自分自身だった**（11.4-9、22章R-2）。他に承認できる人が
    /// いない場合に限り許される。**保存するのは、後からメンバーが増えた際に
    /// 経緯を追えるようにするため**であり、その場の再計算では復元できない。
    pub self_approved: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::work_order::Entity",
        from = "Column::WorkOrderId",
        to = "super::work_order::Column::Id"
    )]
    WorkOrder,
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::RequiredProjectId",
        to = "super::project::Column::Id"
    )]
    RequiredProject,
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::ApproverId",
        to = "super::app_user::Column::Id"
    )]
    Approver,
}

impl Related<super::work_order::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::WorkOrder.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
