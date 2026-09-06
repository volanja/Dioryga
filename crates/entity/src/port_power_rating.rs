//! ポートが受け付ける給電方式と定格電圧（設計書12.7）。
//!
//! **給電方式ごとに1行。**交流と直流の双方を受け付けるPSUが実在するため、
//! 1つの `PART_PORT_SLOT` に複数行を許す。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "port_power_rating")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// `port_kind=Power` のポートに限る。
    pub part_port_slot_id: i32,
    /// AC / DC。
    pub current_type: String,
    /// 定格電圧の下限（V）。**DCは負値をとりうる。**
    ///
    /// **絶対値ではなく符号を含めた大小で扱う**（12.7）。Ciscoの`-48V`電源は
    /// `-72`が下限、`-40`が上限になる。
    pub voltage_min: i32,
    /// 定格電圧の上限（V）。同上。
    pub voltage_max: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::part_port_slot::Entity",
        from = "Column::PartPortSlotId",
        to = "super::part_port_slot::Column::Id"
    )]
    PartPortSlot,
}

impl Related<super::part_port_slot::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PartPortSlot.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
