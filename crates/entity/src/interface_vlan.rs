//! インターフェースに載るVLAN（設計書8.3、8.6）— 履歴。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "interface_vlan")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub os_interface_id: i32,
    pub vlan_id: i32,
    /// Untagged（ポートVLAN）/ Tagged（タグVLAN）
    pub tagging_mode: String,
    /// この変更を引き起こしたWORK_ORDER（11章）。**未作成のため外部キーは未設定。**
    pub work_order_id: Option<i32>,
    pub from_date: DateTimeUtc,
    /// **null が現在有効な行。**既存行を更新せず、閉じて新しい行を開く。
    pub to_date: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::os_interface::Entity",
        from = "Column::OsInterfaceId",
        to = "super::os_interface::Column::Id"
    )]
    OsInterface,
    #[sea_orm(
        belongs_to = "super::vlan::Entity",
        from = "Column::VlanId",
        to = "super::vlan::Column::Id"
    )]
    Vlan,
}

impl Related<super::os_interface::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OsInterface.def()
    }
}

impl Related<super::vlan::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Vlan.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
