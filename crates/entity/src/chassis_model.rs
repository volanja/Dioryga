//! 筐体モデル（設計書6.2、8.6）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "chassis_model")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub vendor_id: i32,
    pub model_name: String,
    /// Server / Switch / ... 語彙は vocabularies.md
    pub device_category: String,
    /// ラック搭載時の高さ。mount_form=Surface では意味を持たない。
    pub height_u: i32,
    /// RackU / RackSide / Surface
    pub mount_form: String,
    /// Full / Half。mount_form=RackU のときのみ意味を持つ。
    pub rack_width: Option<String>,
    pub created_by: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::vendor::Entity",
        from = "Column::VendorId",
        to = "super::vendor::Column::Id"
    )]
    Vendor,
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::vendor::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vendor.def()
    }
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
