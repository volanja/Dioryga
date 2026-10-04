//! 機器を載せる什器（設計書12章）。
//!
//! `location_type` / `location_id` は多態的参照であり、DBの外部キー制約を
//! 持てない（旧C-7）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mount_container")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    /// 型番（`CONTAINER_MODEL`、#205）。種別と収容能力はカタログが持つ。
    ///
    /// **DB上はnullableだが、アプリケーション層では必須**——SQLiteは外部キーを
    /// 持つ列を後から足すとき、既定値を NULL にしか取れない（`cable_kind` と同じ）。
    pub container_model_id: Option<i32>,
    /// Project。倉庫の棚も倉庫用のプロジェクトに置く（#196、#220）
    pub location_type: String,
    pub location_id: i32,
    /// 設置場所（自由記述、#206）。データセンター・フロア・列など。**置き場所
    /// （`location_type`）は所有者であり、物理的な場所ではない**（12.10）。
    pub installation_site: Option<String>,
    /// 撤去（#204）。**過去に使った設備は行を残す**——搭載の履歴が指し続けている。
    /// 名前の一意（置き場所の中、大文字小文字を区別しない）は撤去していないものに限る。
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
