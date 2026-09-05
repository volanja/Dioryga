//! VLAN（設計書8.3）。
//!
//! **一意制約は張らない。**VLANタグはL2ドメインごとに独立しており、
//! 拠点が違えば同じタグを使える。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "vlan")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub vlan_tag: i32,
    pub name: String,
    /// DMZ / WAN / LAN / Management / Isolated。セキュリティ境界（8.5）。
    pub zone: Option<String>,
    pub description: String,
    /// 廃番（18.5）。`None` = 現役。**参照済みでも設定できる**——18.2が禁じて
    /// いるのはスペックを定義するフィールドの編集であり、選択可否はスペックでは
    /// ない。既存の参照は壊さず、過去の事実として残る。
    pub retired_at: Option<DateTimeUtc>,
    pub created_by: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
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
