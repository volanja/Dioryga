//! ケーブルのカタログ（設計書8.3）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cable_catalog")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Network / Power / Stack（8.7）。**画面と取込をここで分ける。**
    ///
    /// **DB上はnullableだが、アプリケーション層では必須**——SQLiteが
    /// `NOT NULL` の列を後から足す際に要求するDEFAULTが以後も残り、種別未指定の
    /// ケーブルを黙って `Network` に分類してしまうため（8.6、24.3）。
    pub cable_kind: Option<String>,
    /// **開いた語彙**（8.6）。Cat6 / OM4 / OS2 / DAC / Power Cord ...
    pub cable_type: String,
    /// **ミリメートルの整数。**SQLiteに DECIMAL が無いため（24.2.1）。
    /// 単位を列名に含めているのは1000倍の取り違えを防ぐため。
    pub length_mm: Option<i32>,
    pub color: String,
    pub vendor_id: Option<i32>,
    pub part_number: Option<String>,
    /// 定格電圧（V）。**`cable_kind=Power` のときだけ意味を持つ**（8.7）。
    ///
    /// **形状が嵌合しても定格が足りなければ使えない。**同じC13でも10A品と
    /// 15A品がある。適合表（Q-8）は形状の対応関係を、この2列は定格を扱う。
    pub rated_voltage: Option<i32>,
    /// 定格電流（mA）。同上。**`length_mm` と同じく最小単位の整数**（24.2.1）。
    pub rated_current_ma: Option<i32>,
    /// 廃番（18.5）。`None` = 現役。**参照済みでも設定できる**——18.2が禁じて
    /// いるのはスペックを定義するフィールドの編集であり、選択可否はスペックでは
    /// ない。既存の参照は壊さず、過去の事実として残る。
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
