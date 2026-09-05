//! 部品カタログ（設計書6.2、6.4）。
//!
//! **集計に使う値だけをカラム化する**ハイブリッド方針（6.4）。コア数と容量は
//! 横断集計の対象なので実カラム、それ以外は `spec_json` に入れる。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "part_catalog")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// CPU / Memory / NIC / Storage / PSU / PDU
    pub category: String,
    pub vendor_id: i32,
    pub part_number: String,
    /// 該当しない部品では null。
    pub core_count: Option<i32>,
    /// 同上。
    pub capacity_gb: Option<i32>,
    /// 周波数・ECC有無・RPM等。JSONは文字列で持つ（24.2.3）。
    pub spec_json: String,
    /// 廃番（18.5）。`None` = 現役。**参照済みでも設定できる**——18.2が禁じて
    /// いるのはスペックを定義するフィールドの編集であり、選択可否はスペックでは
    /// ない。既存の参照は壊さず、過去の事実として残る。
    pub retired_at: Option<DateTimeUtc>,
    /// 重複統合で吸収された場合の統合先（18.5）。**削除ではなくリダイレクト**。
    ///
    /// **外部キーは張らない**（24.3）。後からの列追加であり、SQLiteは
    /// `ALTER TABLE` で制約を足せない。
    pub merged_into_part_catalog_id: Option<i32>,
    pub merged_at: Option<DateTimeUtc>,
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
