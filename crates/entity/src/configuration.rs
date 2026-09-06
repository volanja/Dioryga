//! 筐体モデルに対する構成（設計書6.2）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "configuration")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub chassis_model_id: i32,
    pub name: String,
    /// 廃番（18.5）。`None` = 現役。**参照済みでも設定できる**——18.2が禁じて
    /// いるのはスペックを定義するフィールドの編集であり、選択可否はスペックでは
    /// ない。既存の参照は壊さず、過去の事実として残る。
    pub retired_at: Option<DateTimeUtc>,
    /// その構成で**実際に使う**給電方式（AC / DC、12.8）。
    ///
    /// **カタログでは決まらない。**交流と直流の双方を受け付けるPSUが実在し、
    /// どちらで使うかは設置環境による（12.7）。
    pub current_type: Option<String>,
    /// 想定電圧（V）。**DCは負値をとりうる。**
    pub assumed_voltage: Option<i32>,
    /// 引くと見込む皮相電力（VA）。
    ///
    /// **ワットではなくVAで持つ。**ブレーカーに効くのは皮相電力である（12.7）。
    /// **電流は保存しない**——`A = VA ÷ V` で求める（不変条件2）。
    pub assumed_va: Option<i32>,
    pub created_by: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chassis_model::Entity",
        from = "Column::ChassisModelId",
        to = "super::chassis_model::Column::Id"
    )]
    ChassisModel,
    #[sea_orm(
        belongs_to = "super::app_user::Entity",
        from = "Column::CreatedBy",
        to = "super::app_user::Column::Id"
    )]
    AppUser,
}

impl Related<super::chassis_model::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChassisModel.def()
    }
}

impl Related<super::app_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AppUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
