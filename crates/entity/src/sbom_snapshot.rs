//! SBOMのスナップショット（設計書9.5、9.7）。
//!
//! **本システムで唯一、代理キーを持たないテーブル。**主キーは正規化後の内容の
//! SHA-256であり、**同一内容は1件しか保存しない。**
//!
//! サーバ群はゴールデンイメージから構築されるため、同一構成の機器のSBOMは完全に
//! 一致する。内容を主キーにすれば、**保存されるblobはイメージの種類数に比例し、
//! 台数には比例しない。**10,000台規模（17.2）ではこれが成立の条件になる。
//!
//! `content` は正規化済みコンポーネント一覧を**zstdで圧縮**したもの。内側の
//! 形式はJSONのままとし、デバッグ容易性を優先している（9.7）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sbom_snapshot")]
pub struct Model {
    /// 正規化後の内容のSHA-256。**代理キーではない。**
    #[sea_orm(primary_key, auto_increment = false)]
    pub content_hash: String,
    /// zstdで圧縮した正規化済みコンポーネント一覧。
    pub content: Vec<u8>,
    pub component_count: i32,
    pub first_seen_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::sbom_import::Entity")]
    Import,
    #[sea_orm(has_many = "super::sbom_component_index::Entity")]
    ComponentIndex,
}

impl Related<super::sbom_import::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Import.def()
    }
}

impl Related<super::sbom_component_index::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ComponentIndex.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
