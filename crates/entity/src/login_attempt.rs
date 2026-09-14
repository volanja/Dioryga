//! ログイン試行の記録。レート制限の判定に用いる（設計書20.6）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "login_attempt")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 正規化したユーザー名（設計書20.6）。存在しないユーザーへの試行も記録するため、
    /// `app_user` へのFKにはしない。
    pub username: String,
    pub ip_address: String,
    pub succeeded: bool,
    pub attempted_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
