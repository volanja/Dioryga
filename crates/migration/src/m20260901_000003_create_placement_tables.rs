//! 物理設置のテーブルを作成する（設計書12章、13.2）。
//!
//! # 搭載先は「什器」か「他の機器」のどちらか
//!
//! `DEVICE_MOUNT` の1行では **`container_id` と `host_device_id` のどちらか
//! 一方だけが埋まる**（12.2）。ラックに直接載るのか、棚板やハイパーバイザの
//! ような他の機器の上に載るのかを表し分けるためである。
//!
//! **この排他はDB制約にしない。**アプリケーション層で検証する（他の業務ルールと
//! 同じ理由。設計書4章）。
//!
//! # 収容能力を持つのは什器だけ
//!
//! `MOUNT_CONTAINER.capacity` はラックのU数、棚の段数にあたる。**超過は
//! エラーではなく警告**として扱う（不変条件6）。実機が仕様の想定外である
//! ことはあり、誤って拒否すると事実を記録できなくなる。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // -------------------------------------------------------------------
        // WAREHOUSE（12章）
        // -------------------------------------------------------------------
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
                    .foreign_key(
                        ForeignKey::create()
                            .from(Warehouse::Table, Warehouse::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // MOUNT_CONTAINER（12章）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(MountContainer::Table)
                    .if_not_exists()
                    .col(pk_auto(MountContainer::Id))
                    .col(string(MountContainer::Name))
                    .col(string(MountContainer::ContainerType))
                    // Warehouse / Project。多態のため外部キーなし（旧C-7）
                    .col(string(MountContainer::LocationType))
                    .col(integer(MountContainer::LocationId))
                    // ラックのU数、棚の段数。持たない什器もある
                    .col(integer_null(MountContainer::Capacity))
                    .col(integer(MountContainer::CreatedBy))
                    .col(timestamp_with_time_zone(MountContainer::CreatedAt))
                    .col(timestamp_with_time_zone(MountContainer::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(MountContainer::Table, MountContainer::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 「この倉庫／プロジェクトにある什器」を引く
        manager
            .create_index(
                Index::create()
                    .name("idx_mount_container_location")
                    .table(MountContainer::Table)
                    .col(MountContainer::LocationType)
                    .col(MountContainer::LocationId)
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // DEVICE_MOUNT（12.2、13.2）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(DeviceMount::Table)
                    .if_not_exists()
                    .col(pk_auto(DeviceMount::Id))
                    .col(integer(DeviceMount::DeviceId))
                    // 什器に直接搭載する場合。host_device_id とは排他
                    .col(integer_null(DeviceMount::ContainerId))
                    // Rackなら開始U番号、Shelvingなら段番号
                    .col(integer_null(DeviceMount::Position))
                    // ハーフラック幅の機器を左右に並べる場合に使う（12.2）
                    .col(string_null(DeviceMount::HorizontalPosition))
                    // 前面・背面の作り分け
                    .col(string_null(DeviceMount::DepthPosition))
                    // 他の機器の上に載る場合（棚板、VMの実行ホスト）
                    .col(integer_null(DeviceMount::HostDeviceId))
                    .col(integer_null(DeviceMount::WorkOrderId))
                    .col(timestamp_with_time_zone(DeviceMount::FromDate))
                    .col(timestamp_with_time_zone_null(DeviceMount::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(DeviceMount::Table, DeviceMount::DeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(DeviceMount::Table, DeviceMount::ContainerId)
                            .to(MountContainer::Table, MountContainer::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(DeviceMount::Table, DeviceMount::HostDeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 現在の搭載位置を引く（設計書24.3）
        current_index(
            manager,
            "idx_device_mount_current",
            DeviceMount::Table,
            DeviceMount::DeviceId,
            DeviceMount::ToDate,
        )
        .await?;

        // ラック図の描画は「この什器に載っているもの」を引く（12.2）
        current_index(
            manager,
            "idx_device_mount_container_current",
            DeviceMount::Table,
            DeviceMount::ContainerId,
            DeviceMount::ToDate,
        )
        .await?;

        // 「この機器の上に載っているもの」を引く。棚板やVMの一覧に使う
        current_index(
            manager,
            "idx_device_mount_host_current",
            DeviceMount::Table,
            DeviceMount::HostDeviceId,
            DeviceMount::ToDate,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            DeviceMount::Table.into_iden(),
            MountContainer::Table.into_iden(),
            Warehouse::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}

/// 履歴テーブルの「現在有効な行」を引くための部分インデックス（設計書24.3）。
async fn current_index<T, C, D>(
    manager: &SchemaManager<'_>,
    name: &str,
    table: T,
    column: C,
    to_date: D,
) -> Result<(), DbErr>
where
    T: IntoIden + 'static,
    C: IntoIden + 'static,
    D: IntoIden + 'static,
{
    manager
        .create_index(
            Index::create()
                .name(name)
                .table(table)
                .col(column)
                .and_where(Expr::col(to_date).is_null())
                .to_owned(),
        )
        .await
}

/// 既存のマイグレーションで作成済み。外部キーの参照先としてのみ使う。
#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Device {
    Table,
    Id,
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
}

#[derive(DeriveIden)]
enum MountContainer {
    Table,
    Id,
    Name,
    ContainerType,
    LocationType,
    LocationId,
    Capacity,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum DeviceMount {
    Table,
    Id,
    DeviceId,
    ContainerId,
    Position,
    HorizontalPosition,
    DepthPosition,
    HostDeviceId,
    WorkOrderId,
    FromDate,
    ToDate,
}
