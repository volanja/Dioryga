use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "project")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 登録時に採番する不変の識別子。名前を変更しても参照が切れない（設計書5.3）。
    /// PostgreSQLの `uuid` 型ではなく文字列で持つ（24.2.3）。
    #[sea_orm(unique)]
    pub uid: String,
    /// 組織のプロジェクトコード。設定されていれば一意。
    #[sea_orm(unique)]
    pub code: Option<String>,
    /// 表示名。一意制約は張らない（年度違いで同名の案件がありうる）。
    pub name: String,
    pub description: String,
    /// 集計通貨。金額の小数点以下の桁数はこの値から決まる（設計書24.2.1）。
    pub currency: String,
    /// null = 進行中。サービス上の終了ではなく、一覧から外すという運用判断（5.2）。
    pub archived_at: Option<DateTimeUtc>,
    /// `Completed` / `Cancelled`。`archived_at` がある場合のみ意味を持つ。
    pub closure_reason: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::project_member::Entity")]
    ProjectMember,
}

impl Related<super::project_member::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ProjectMember.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
