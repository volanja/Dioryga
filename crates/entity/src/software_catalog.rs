//! ソフトウェアのカタログ（設計書9.4）。
//!
//! **SBOM取込はこのテーブルを自動生成しない。**人が資産として登録したものだけを
//! 置く（9.2）。観測されたコンポーネントは SBOM 側のテーブルが持つ。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "software_catalog")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub vendor_id: Option<i32>,
    /// バージョンごとに別レコード。
    pub version: String,
    /// OS / Application / Library / Framework
    pub category: String,
    /// あればこれが一意。NULL同士は重複と見なされない。
    pub purl: Option<String>,
    pub license_expression: String,
    pub spec_json: String,
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
