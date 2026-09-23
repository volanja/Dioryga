//! マイルストーン（設計書10.4）。
//!
//! **予定と実績を別の列で持つ。**片方に上書きすると「当初いつの予定だったか」が
//! 失われ、QCDの「D」を定量的に見られなくなる（5.1）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "milestone")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 登録時に採番する不変の識別子。取込時の突合に使う（23.5）。
    pub uid: String,
    /// 取込元システムでの識別子。再取込時の突合用（23.5）。
    pub external_id: Option<String>,
    pub project_id: i32,
    /// ServiceStart / ServiceUpdate / ServiceMaintenance / ServiceEnd
    pub milestone_type: String,
    /// **`date` のまま。**同日内の順序を問わない（C-5）。
    pub planned_date: Date,
    pub actual_date: Option<Date>,
    /// planned / completed / cancelled
    pub status: String,
    pub description: String,
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
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
