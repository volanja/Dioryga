//! 設備・什器の型番のカタログ（設計書12.10、#205）。
//!
//! 型番で決まるもの——種別、収容能力、寸法・重量・静荷重——を持つ。設備・什器
//! （`MOUNT_CONTAINER`）はこれを指す。機器の機種（`CHASSIS_MODEL`）と同じ置き方。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "container_model")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub vendor_id: i32,
    /// 型番。自然キーは「ベンダー＋型番」。
    pub model_name: String,
    /// Rack / Desk / Shelving（閉じた語彙、8.6）。**使う列が種別で分かれる。**
    pub container_type: String,
    /// Rack のみ。総U数。
    pub height_u: Option<i32>,
    /// Shelving のみ。段数。
    pub shelf_count: Option<i32>,
    /// 寸法（mm）。**最小単位の整数**（24.2.1）。
    pub width_mm: Option<i32>,
    pub depth_mm: Option<i32>,
    pub height_mm: Option<i32>,
    /// 本体の重量（g）。画面では kg で入力させる。
    pub weight_g: Option<i32>,
    /// 静荷重（耐荷重、g）。**移動時の動荷重は持たない**——台帳が答えるのは
    /// 設置した状態の重さである（12.10）。
    pub static_load_g: Option<i32>,
    /// 廃番（18.5）。参照済みでも設定できる。
    pub retired_at: Option<DateTimeUtc>,
    pub created_by: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::vendor::Entity",
        from = "Column::VendorId",
        to = "super::vendor::Column::Id"
    )]
    Vendor,
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::vendor::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vendor.def()
    }
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
