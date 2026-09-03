//! インターフェースの積み重ね（設計書8.5）— 履歴。
//!
//! ボンド（1 upper : N lower）とVLANサブインターフェース（1 lower : N upper）の
//! 双方を扱うため中間テーブルにしている。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "interface_stack")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 束ねる側（bond0、bond0.100）。
    pub upper_interface_id: i32,
    /// 束ねられる側（ens1f0、bond0）。
    pub lower_interface_id: i32,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
