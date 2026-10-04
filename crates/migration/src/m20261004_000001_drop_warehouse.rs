//! 倉庫（`WAREHOUSE`）を消す（#220、#196、設計書16.1）。
//!
//! 予備の機器・部品は、最初から用意する倉庫用のプロジェクト（#217）に置く。
//! 倉庫を独立の領域として持つ仕組みは要らなくなった。
//!
//! **所在の行（`DEVICE_ASSIGNMENT` / `PART_INSTANCE_LOCATION` / `MOUNT_CONTAINER`）の
//! `location_type="Warehouse"` は移し替えない。**未リリースのため、既存の倉庫の
//! データは作り直しでよい（#220）。多態的参照で外部キーが無いので、テーブルを
//! 消してもDBは止めない。
//!
//! `down` は空の表を作り直すだけで、消した行は戻らない。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Warehouse::Table).to_owned())
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Warehouse::Table)
                    .if_not_exists()
                    .col(pk_auto(Warehouse::Id))
                    .col(string(Warehouse::Name))
                    .col(string(Warehouse::Address).default(""))
                    .col(integer(Warehouse::CreatedBy))
                    .col(timestamp_with_time_zone(Warehouse::CreatedAt))
                    .col(timestamp_with_time_zone(Warehouse::UpdatedAt))
                    .col(timestamp_with_time_zone_null(Warehouse::RetiredAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Warehouse::Table, Warehouse::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Warehouse {
    Table,
    Id,
    Name,
    Address,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
    RetiredAt,
}

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
}
