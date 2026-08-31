//! スタック構成（設計書8.6）— 履歴。
//!
//! スタック全体を `device_type="Logical"` の DEVICE として登録し、物理筐体は
//! `Physical` の DEVICE として別に登録する。hostname や OS_INTERFACE は論理側、
//! DEVICE_MOUNT や power_watt は物理側が持つ。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "device_stack")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// スタック全体を表す Logical な DEVICE。
    pub logical_device_id: i32,
    /// 実際の筐体（Physical）。
    pub member_device_id: i32,
    pub member_number: i32,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
