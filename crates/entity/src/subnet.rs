//! サブネット（設計書14.1）。
//!
//! **プロジェクト単位に分ける。**異なるプロジェクトが同じプライベート
//! アドレス帯を独立に使っていても衝突しない（14.2）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "subnet")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 共有バックボーン等は null。
    pub project_id: Option<i32>,
    /// VLANを伴わないサブネットがありうる。
    pub vlan_id: Option<i32>,
    pub cidr: String,
    /// VLAN側にも持つ。**両方あるときはこちらを優先する**（14.2）。
    pub zone: Option<String>,
    pub description: String,
    pub created_by: i32,
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
        belongs_to = "super::vlan::Entity",
        from = "Column::VlanId",
        to = "super::vlan::Column::Id"
    )]
    Vlan,
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl Related<super::vlan::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vlan.def()
    }
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
