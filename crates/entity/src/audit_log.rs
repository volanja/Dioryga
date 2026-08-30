//! 監査ログ。
//!
//! 書き込みはDBトリガーではなくリポジトリ層で行い、本来の変更と同一トランザクション
//! 内に記録する（設計書15.2）。その仕組みは別のissueで実装する。
//!
//! `before_json` / `after_json` には機微なカラムを含めてはならない（設計書21.7）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "audit_log")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub user_id: i32,
    pub table_name: String,
    pub record_id: i32,
    /// `insert` / `update` / `delete`
    pub action: String,
    /// JSONは両DBとも文字列で持つ（設計書24.2.3）。
    #[sea_orm(column_type = "Text", nullable)]
    pub before_json: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub after_json: Option<String>,
    /// 一括取込による変更の場合に設定する（設計書23.7）。
    pub import_run_id: Option<i32>,
    pub changed_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::UserId",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
