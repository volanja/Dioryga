//! 設計書の `USER`。`user` はPostgreSQLの予約語のため物理名は `app_user`。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "app_user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 表示名。画面のヘッダー・一覧に出すのはこちら（設計書20.1）。
    pub name: String,
    /// **ログインID。**ASCII英小文字・数字と `.` `_` `-`、2〜32文字。小文字で保存する
    /// （設計書20.1、`auth::username`）。
    #[sea_orm(unique)]
    pub username: String,
    /// 任意。**ログインIDではない。**値がある場合は一意（設計書20.1）。
    #[sea_orm(unique)]
    pub email: Option<String>,
    pub password_hash: String,
    pub must_change_password: bool,
    /// プロジェクトロールとは別軸で扱う（設計書4章）。
    pub is_system_admin: bool,
    pub locale: String,
    /// 表示モード。`system`（OSに従う）／`light`／`dark`（#120）。
    pub theme: String,
    pub last_login_at: Option<DateTimeUtc>,
    /// null = 有効。物理削除はしない（設計書20.11）。
    pub disabled_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::project_member::Entity")]
    ProjectMember,
    #[sea_orm(has_many = "super::session::Entity")]
    Session,
}

impl Related<super::project_member::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ProjectMember.def()
    }
}

impl Related<super::session::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Session.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
