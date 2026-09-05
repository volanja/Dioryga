//! 直前の取込との差分（設計書9.5、9.6）。
//!
//! **SBOM取込の目的は「差分を記録すること」である**（9.2）。いつ・誰の操作で・
//! どのバージョンからどのバージョンへ変わったかを残すことが目的であり、
//! コンポーネント1件ごとに集計可能な形で保持することは要件ではない。
//!
//! **同一性の判定は `purl` をキーとし、無いコンポーネントは `name` で代替する**
//! （9.6）。同じ `purl` で `version` だけ変われば `version_changed`、片側に
//! しか無ければ `added` / `removed` となる。
//!
//! 構成に変化がなければ0件になる。`SBOM_IMPORT` の行だけが残る（9.6の手順3）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sbom_component_change")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub sbom_import_id: i32,
    /// added / removed / version_changed
    pub change_type: String,
    pub name: String,
    pub purl: Option<String>,
    /// `added` のときは `None`。
    pub version_from: Option<String>,
    /// `removed` のときは `None`。
    pub version_to: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::sbom_import::Entity",
        from = "Column::SbomImportId",
        to = "super::sbom_import::Column::Id"
    )]
    SbomImport,
}

impl Related<super::sbom_import::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SbomImport.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
