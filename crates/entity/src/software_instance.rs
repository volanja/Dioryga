//! ソフトウェアの固有のインストール単位（設計書9.3、9.4）。
//!
//! ライセンスキー・固定資産番号など、**バージョンアップを跨いで引き継がれる
//! 固有情報**を持つ。バージョンアップは「新しいインスタンスを作り、
//! [`super::software_installation`] の旧行を閉じて新行を開く」で表す。
//!
//! **`status` を持たない**（9.4.1）。「インストールされているか」は
//! `SOFTWARE_INSTALLATION` の現行行から導出する。二重に持つと必ずずれる。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "software_instance")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub software_catalog_id: i32,
    pub license_key: Option<String>,
    pub asset_number: Option<String>,
    /// `None` = 保有中。**導出できないのはここだけ**（9.4.1）。
    /// 未インストールの在庫と失効済みは、インストール履歴からは区別がつかない。
    pub retired_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::software_catalog::Entity",
        from = "Column::SoftwareCatalogId",
        to = "super::software_catalog::Column::Id"
    )]
    SoftwareCatalog,
    #[sea_orm(has_many = "super::software_installation::Entity")]
    Installation,
}

impl Related<super::software_catalog::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SoftwareCatalog.def()
    }
}

impl Related<super::software_installation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Installation.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
