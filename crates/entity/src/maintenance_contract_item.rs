//! 保守契約の対象（設計書10章）。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "maintenance_contract_item")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub maintenance_contract_id: i32,
    /// 多態的参照のため外部キーを持てない。
    pub item_type: String,
    pub item_id: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::maintenance_contract::Entity",
        from = "Column::MaintenanceContractId",
        to = "super::maintenance_contract::Column::Id"
    )]
    MaintenanceContract,
}

impl Related<super::maintenance_contract::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MaintenanceContract.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
