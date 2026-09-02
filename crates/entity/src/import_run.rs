//! 取込の実行記録（設計書23.7）。
//!
//! **取込では行ごとの監査ログを書かない**（24.4）。25万行の取込で25万行の
//! 監査ログが生まれると肥大するうえ、すべて同じ主体・時刻・`import_run_id` を
//! 持つため情報が増えない。追跡はこのテーブルが担う。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "import_run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// カタログ取込はプロジェクトに属さない（18.1）。
    pub project_id: Option<i32>,
    /// `catalog` / `instances`
    pub kind: String,
    /// 取り込んだファイルのSHA-256。同じファイルを二度流したかを判別できる。
    pub file_hash: String,
    /// マニフェストで宣言された基準時刻。履歴行の `from_date` に使う（23.1）。
    pub as_of: DateTimeUtc,
    pub created_count: i32,
    pub updated_count: i32,
    pub warning_count: i32,
    pub imported_by: i32,
    pub imported_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::ImportedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
