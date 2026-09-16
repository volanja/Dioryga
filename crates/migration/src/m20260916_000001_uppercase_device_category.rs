//! 機器の種別の略語を大文字に改める（`Vpn` → `VPN` 等、#125）。
//!
//! 種別は**DB制約にせずアプリケーションの語彙で検証している**（設計書8.6）。
//! 語彙の表を改めただけでは、既に保存された `Vpn` が語彙外の値として残る。
//! 画面は選択肢に無い値を選べず、編集のたびに種別を選び直させることになる。
//!
//! 書き換えるのは種別を持つ2か所——`CHASSIS_MODEL` と、構成を持たない
//! `DEVICE`——だけである。

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// （旧表記, 新表記）
const 対応: &[(&str, &str)] = &[
    ("Vpn", "VPN"),
    ("Pdu", "PDU"),
    ("Ups", "UPS"),
    ("Kvm", "KVM"),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (旧, 新) in 対応 {
            書き換える(manager, 旧, 新).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (旧, 新) in 対応 {
            書き換える(manager, 新, 旧).await?;
        }
        Ok(())
    }
}

async fn 書き換える(manager: &SchemaManager<'_>, from: &str, to: &str) -> Result<(), DbErr> {
    manager
        .exec_stmt(
            Query::update()
                .table(ChassisModel::Table)
                .value(ChassisModel::DeviceCategory, to)
                .and_where(Expr::col(ChassisModel::DeviceCategory).eq(from))
                .to_owned(),
        )
        .await?;
    manager
        .exec_stmt(
            Query::update()
                .table(Device::Table)
                .value(Device::DeviceCategory, to)
                .and_where(Expr::col(Device::DeviceCategory).eq(from))
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum ChassisModel {
    Table,
    DeviceCategory,
}

#[derive(DeriveIden)]
enum Device {
    Table,
    DeviceCategory,
}
