//! ファームウェアの版（設計書6.2）— 履歴。
//!
//! `item_type` / `item_id` は多態的参照。DEVICE と PART_INSTANCE の
//! どちらも対象になるため、外部キー制約を持てない。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "firmware_version")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Device / PartInstance
    pub item_type: String,
    pub item_id: i32,
    /// BIOS / BMC / NIC / RAIDController 等
    pub component: String,
    pub version: String,
    pub work_order_id: Option<i32>,
    pub changed_by: i32,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::ChangedBy",
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
