//! 機器横断のコンポーネント検索用の索引（設計書9.8）。
//!
//! **再構築可能な派生索引であり、真実の源は [`super::sbom_snapshot`] である。**
//! いつでも捨てて作り直せる。不整合はデータ損失にならない。
//!
//! **索引は `content_hash` ごとに張る。Deviceごとにしない。**Device単位だと
//! 3,000万行になるが、スナップショットを共有している以上その必要がなく、
//! イメージ種類数に比例する約60,000行で済む。
//!
//! 作成は新規 `content_hash` が登録された時のみ発生する。既存スナップショットの
//! 再利用時は索引の更新も要らない（9.6の手順5）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sbom_component_index")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub content_hash: String,
    pub purl: Option<String>,
    pub name: String,
    pub version: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::sbom_snapshot::Entity",
        from = "Column::ContentHash",
        to = "super::sbom_snapshot::Column::ContentHash"
    )]
    Snapshot,
}

impl Related<super::sbom_snapshot::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Snapshot.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
