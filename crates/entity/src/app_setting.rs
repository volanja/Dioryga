//! アプリ全体の設定（#217、設計書16.1）。
//!
//! **`id = 1` の1行だけを使う。**行が無いのは、まだ初回セットアップをしていない
//! DBである。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 使う行の `id`。
pub const ID: i32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "app_setting")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i32,
    /// 新しい利用者を加える先（#196、#218）。最初は倉庫用のプロジェクトを指す。
    /// **倉庫プロジェクトの印ではない。**この参加先にだけ効かせる。
    pub default_project_id: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::DefaultProjectId",
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
