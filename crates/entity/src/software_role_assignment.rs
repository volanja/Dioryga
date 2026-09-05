//! ソフトウェアの役割 — 履歴テーブル（設計書9.4、9.9）。
//!
//! **SBOM取込の対象外。常に運用者が画面上で設定する**（9.9）。SBOMは技術的な
//! `type` は持つが、「このBIND9はDNSサーバーとして動いている」という業務的な
//! 役割は範囲外であり、機械的に導出できない。
//!
//! **1つのインストールに複数のroleを付与できる。**dnsmasqのようにDNSとDHCPを
//! 兼ねるミドルウェアを表すため、単一カラムにせず別テーブルへ分離した。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "software_role_assignment")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub software_installation_id: i32,
    /// OS / DNS / NTP / WebServer / App ...
    pub role: String,
    pub from_date: DateTimeUtc,
    /// `None` が現在有効な行。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::software_installation::Entity",
        from = "Column::SoftwareInstallationId",
        to = "super::software_installation::Column::Id"
    )]
    SoftwareInstallation,
}

impl Related<super::software_installation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SoftwareInstallation.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
