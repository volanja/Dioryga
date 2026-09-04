//! 固定資産（設計書10章）。
//!
//! **`disposal_date` と簿価を持たない。**廃棄は `DEVICE_ASSIGNMENT` の
//! `Disposed` 行から、簿価は取得価額と経過期間から計算する（旧B-1、不変条件2）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "fixed_asset")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub item_type: String,
    pub item_id: i32,
    /// **最小通貨単位の整数**（24.2.1）。SQLiteにDECIMALが無く、REALでは
    /// 丸め誤差が累積するため。桁数は `PROJECT.currency` から決まる。
    pub acquisition_cost: i64,
    /// straight_line / declining_balance
    pub depreciation_method: String,
    pub useful_life_years: i32,
    pub acquisition_date: Date,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
