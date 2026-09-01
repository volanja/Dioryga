//! 外部キー列に張り忘れていたインデックスを追加する（設計書24.3）。
//!
//! **`tbls lint` の `requireForeignKeyIndex` が検出した。**両DBとも外部キー列に
//! 自動でインデックスを張らないため、明示が要る——と分かっていても手作業では
//! 抜ける。以降はCIが同じ検査を行う。
//!
//! | 列 | 引き方 |
//! |---|---|
//! | `cable_catalog.vendor_id` | ベンダーからケーブルカタログを引く |
//! | `part_instance_location.chassis_slot_id` | スロットから搭載中の部品を引く |
//!
//! 既存のマイグレーションを書き換えず追加で直すのは、**適用済みのマイグレーションを
//! 変更してはならない**ため。

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_cable_catalog_vendor")
                    .table(CableCatalog::Table)
                    .col(CableCatalog::VendorId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_part_instance_location_chassis_slot")
                    .table(PartInstanceLocation::Table)
                    .col(PartInstanceLocation::ChassisSlotId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_part_instance_location_chassis_slot")
                    .table(PartInstanceLocation::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_cable_catalog_vendor")
                    .table(CableCatalog::Table)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum CableCatalog {
    Table,
    VendorId,
}

#[derive(DeriveIden)]
enum PartInstanceLocation {
    Table,
    ChassisSlotId,
}
