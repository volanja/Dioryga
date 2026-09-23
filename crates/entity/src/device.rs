//! 機器（設計書6.2、8.6、13.2、23.2、23.9）。
//!
//! **`in_stock` / `disposed` は `status` に持たない。**所在は
//! `DEVICE_ASSIGNMENT` から導出する（旧B-1）。状態を二重に持つと必ずずれる。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "device")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 登録時に採番する不変の識別子。取込時の突合に使う（23.2）。
    pub uid: String,
    /// 取込元システムでの識別子。再取込時の突合用。
    pub external_id: Option<String>,
    /// 重複統合で吸収された場合の統合先。**削除ではなくリダイレクト**（23.9）。
    pub merged_into_device_id: Option<i32>,
    pub merged_at: Option<DateTimeUtc>,
    /// Virtual / Container / Logical は持たない。
    pub configuration_id: Option<i32>,
    /// Physical / Virtual / Container / Logical
    pub device_type: String,
    /// `configuration_id` が無い機器の分類。
    pub device_category: Option<String>,
    pub hostname: String,
    /// Virtual / Container には存在しない。
    pub serial_number: Option<String>,
    /// **採番待ちでも登録できるよう nullable**（23.2）。番号が届くまで
    /// 登録できないのは実務上成立しない。
    pub asset_number: Option<String>,
    pub power_watt: i32,
    /// running / failed / repairing / planned / provisioning
    pub status: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::configuration::Entity",
        from = "Column::ConfigurationId",
        to = "super::configuration::Column::Id"
    )]
    Configuration,
}

impl Related<super::configuration::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Configuration.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
