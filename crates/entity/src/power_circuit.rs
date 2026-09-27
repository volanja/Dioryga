//! 設備・什器が受ける給電の回路（設計書12.7、12.10、#206）— 履歴テーブル。
//!
//! 1つの設備・什器に複数の回路（A系・B系）が来る。**変更は閉じて開く**（4章）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "power_circuit")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// 給電を受ける設備・什器（`MOUNT_CONTAINER`）。
    pub container_id: i32,
    /// 系統名（A系、B系など）。同じ設備・什器の中で現在の行を突き合わせる鍵。
    pub circuit_label: String,
    /// 実際に供給されている電圧（V）。
    pub voltage: i32,
    /// Single / Three。**三相の計算はv2**（12.7）。
    pub phase: String,
    /// ブレーカー定格（mA）。**最小単位の整数**（24.2.1）。
    pub breaker_current_ma: i32,
    /// コンセントの形状（NEMA L6-30R 等）。**開いた語彙**（8.6）。
    pub connector_type: Option<String>,
    pub from_date: DateTimeUtc,
    /// null = 現在有効。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::mount_container::Entity",
        from = "Column::ContainerId",
        to = "super::mount_container::Column::Id"
    )]
    MountContainer,
}

impl Related<super::mount_container::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MountContainer.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
