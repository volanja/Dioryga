//! 機器・部品の状態を、運用（`status`）と故障（`health`）の2列に分ける（#221、設計書6.3）。
//!
//! | 列 | 値 |
//! |---|---|
//! | `status` | `planned` / `provisioning` / `running` / `standby` |
//! | `health` | `ok` / `failed` |
//!
//! **修理中は列に持たない。**未完了の修理（`Repair`）のチケットから導出する。
//!
//! 既存の `failed` / `repairing` の行は `health=failed`・`status=running` にする。
//! 未リリースのため、故障前の段階を厳密には復元しない。
//!
//! ケーブル（`CABLE_INSTANCE`）は `status` をやめ、`retired_at`（nullable）だけを
//! 持つ（設計書8.4）。所在・接続・健全性・存在の4つの軸が1列に混ざっていた。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// 旧い `status` のうち、故障を表していた値。
const 故障だった: &[&str] = &["failed", "repairing"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Device::Table)
                    .add_column(string(Device::Health).default("ok"))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PartInstance::Table)
                    .add_column(string(PartInstance::Health).default("ok"))
                    .to_owned(),
            )
            .await?;

        manager
            .exec_stmt(
                Query::update()
                    .table(Device::Table)
                    .value(Device::Health, "failed")
                    .value(Device::Status, "running")
                    .and_where(Expr::col(Device::Status).is_in(故障だった.iter().copied()))
                    .to_owned(),
            )
            .await?;
        manager
            .exec_stmt(
                Query::update()
                    .table(PartInstance::Table)
                    .value(PartInstance::Health, "failed")
                    .value(PartInstance::Status, "running")
                    .and_where(Expr::col(PartInstance::Status).is_in(故障だった.iter().copied()))
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(CableInstance::Table)
                    .drop_column(CableInstance::Status)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(CableInstance::Table)
                    .add_column(timestamp_with_time_zone_null(CableInstance::RetiredAt))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CableInstance::Table)
                    .drop_column(CableInstance::RetiredAt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(CableInstance::Table)
                    .add_column(string(CableInstance::Status).default("in_use"))
                    .to_owned(),
            )
            .await?;

        // `standby` は旧い語彙に無い。最も近い `running` に倒す
        for (table, status, health) in [
            (
                Device::Table.into_iden(),
                Device::Status.into_iden(),
                Device::Health.into_iden(),
            ),
            (
                PartInstance::Table.into_iden(),
                PartInstance::Status.into_iden(),
                PartInstance::Health.into_iden(),
            ),
        ] {
            manager
                .exec_stmt(
                    Query::update()
                        .table(table.clone())
                        .value(status.clone(), "running")
                        .and_where(Expr::col(status.clone()).eq("standby"))
                        .to_owned(),
                )
                .await?;
            manager
                .exec_stmt(
                    Query::update()
                        .table(table.clone())
                        .value(status.clone(), "failed")
                        .and_where(Expr::col(health.clone()).eq("failed"))
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(Table::alter().table(table).drop_column(health).to_owned())
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Device {
    Table,
    Status,
    Health,
}

#[derive(DeriveIden)]
enum PartInstance {
    Table,
    Status,
    Health,
}

#[derive(DeriveIden)]
enum CableInstance {
    Table,
    Status,
    RetiredAt,
}
