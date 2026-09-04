//! 機器のテーブルを作成する（設計書6章、8.6、13.2、23章）。
//!
//! # 履歴テーブルが初めて入る
//!
//! `DEVICE_ASSIGNMENT`・`PART_INSTANCE_LOCATION`・`FIRMWARE_VERSION`・
//! `DEVICE_STACK` は `from_date`/`to_date` を持つ履歴テーブルである。
//! **`to_date IS NULL` が現在有効な行**であり、既存行を更新せず「閉じて開く」
//! （不変条件1）。
//!
//! そのため **`(親ID) WHERE to_date IS NULL` の部分インデックス**を張る。
//! 「現在の状態」を引くクエリが本設計で最も頻出するためであり、部分インデックスは
//! 両DBが対応する（設計書24.3、14.2）。
//!
//! # 多態的参照には外部キーを張れない
//!
//! `location_type`/`location_id`、`item_type`/`item_id` は参照先のテーブルが
//! 値によって変わるため、DBの外部キー制約にできない。**存在チェックは
//! アプリケーション層で行う**（設計書4章、旧C-7）。
//!
//! # `work_order_id` には外部キーを張らない
//!
//! `WORK_ORDER`（11章）は後発のマイグレーションで作成する。当初はここに
//! 「テーブルを作る際に外部キーを追加すること」と書いていたが、**SQLiteの
//! `ALTER TABLE` が制約の追加に対応していないため実現できない**（24.3）。
//! 多態的参照と同じ扱いとし、`dioryga check`（24.5）で検出する。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // -------------------------------------------------------------------
        // DEVICE（6.2、8.6、13.2、23.2、23.9）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Device::Table)
                    .if_not_exists()
                    .col(pk_auto(Device::Id))
                    // 登録時に採番する不変の識別子。取込時の突合に使う（23.2）
                    .col(string(Device::Uid).unique_key())
                    // 取込元システムでの識別子。再取込時の突合用（23.2）
                    .col(string_null(Device::ExternalId))
                    // 重複統合は削除ではなくリダイレクト（23.9）
                    .col(integer_null(Device::MergedIntoDeviceId))
                    .col(timestamp_with_time_zone_null(Device::MergedAt))
                    // Virtual/Container/Logical は構成を持たない
                    .col(integer_null(Device::ConfigurationId))
                    .col(string(Device::DeviceType))
                    // configuration_id が無い機器（仮想アプライアンス等）の分類
                    .col(string_null(Device::DeviceCategory))
                    .col(string(Device::Hostname))
                    // Virtual/Container には存在しない（6.2）
                    .col(string_null(Device::SerialNumber))
                    // **採番待ちでも登録できるようnullable**（23.2）。番号が届く
                    // まで機器を登録できないのは実務上成立しない
                    .col(string_null(Device::AssetNumber))
                    .col(integer(Device::PowerWatt).default(0))
                    // in_stock/disposed はここに持たない。DEVICE_ASSIGNMENT
                    // から導出する（旧B-1）
                    .col(string(Device::Status))
                    .col(timestamp_with_time_zone(Device::CreatedAt))
                    .col(timestamp_with_time_zone(Device::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Device::Table, Device::MergedIntoDeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Device::Table, Device::ConfigurationId)
                            .to(Configuration::Table, Configuration::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 取込時の突合で引く（23.2）。**一意にはしない**——serial_number は
        // Virtual/Container で null になり、hostname は重複しうる
        index(
            manager,
            "idx_device_serial_number",
            Device::Table,
            Device::SerialNumber,
        )
        .await?;
        index(
            manager,
            "idx_device_hostname",
            Device::Table,
            Device::Hostname,
        )
        .await?;
        index(
            manager,
            "idx_device_external_id",
            Device::Table,
            Device::ExternalId,
        )
        .await?;
        index(
            manager,
            "idx_device_merged_into",
            Device::Table,
            Device::MergedIntoDeviceId,
        )
        .await?;
        index(
            manager,
            "idx_device_configuration",
            Device::Table,
            Device::ConfigurationId,
        )
        .await?;

        // -------------------------------------------------------------------
        // DEVICE_ASSIGNMENT（6.2）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(DeviceAssignment::Table)
                    .if_not_exists()
                    .col(pk_auto(DeviceAssignment::Id))
                    .col(integer(DeviceAssignment::DeviceId))
                    // Warehouse / Project / Disposed。多態のため外部キーなし
                    .col(string(DeviceAssignment::LocationType))
                    // Disposed のときは null
                    .col(integer_null(DeviceAssignment::LocationId))
                    .col(integer_null(DeviceAssignment::WorkOrderId))
                    .col(timestamp_with_time_zone(DeviceAssignment::FromDate))
                    .col(timestamp_with_time_zone_null(DeviceAssignment::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(DeviceAssignment::Table, DeviceAssignment::DeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .to_owned(),
            )
            .await?;

        current_index(
            manager,
            "idx_device_assignment_current",
            DeviceAssignment::Table,
            DeviceAssignment::DeviceId,
            DeviceAssignment::ToDate,
        )
        .await?;

        // RBACの可視性判定は「過去に所属したことがあるか」も見る（A-6）ため、
        // 現在有効な行に限らない索引も要る
        manager
            .create_index(
                Index::create()
                    .name("idx_device_assignment_location")
                    .table(DeviceAssignment::Table)
                    .col(DeviceAssignment::LocationType)
                    .col(DeviceAssignment::LocationId)
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // PART_INSTANCE / PART_INSTANCE_LOCATION（6.2、12.4）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(PartInstance::Table)
                    .if_not_exists()
                    .col(pk_auto(PartInstance::Id))
                    .col(integer(PartInstance::PartCatalogId))
                    .col(string_null(PartInstance::SerialNumber))
                    .col(string(PartInstance::Status))
                    .col(timestamp_with_time_zone(PartInstance::CreatedAt))
                    .col(timestamp_with_time_zone(PartInstance::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(PartInstance::Table, PartInstance::PartCatalogId)
                            .to(PartCatalog::Table, PartCatalog::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_part_instance_catalog",
            PartInstance::Table,
            PartInstance::PartCatalogId,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(PartInstanceLocation::Table)
                    .if_not_exists()
                    .col(pk_auto(PartInstanceLocation::Id))
                    .col(integer(PartInstanceLocation::PartInstanceId))
                    // Warehouse / Device / Disposed
                    .col(string(PartInstanceLocation::LocationType))
                    .col(integer_null(PartInstanceLocation::LocationId))
                    // **任意項目。**どのスロットに挿さっているかまでは求めない（6.2）
                    .col(integer_null(PartInstanceLocation::ChassisSlotId))
                    .col(integer_null(PartInstanceLocation::WorkOrderId))
                    .col(timestamp_with_time_zone(PartInstanceLocation::FromDate))
                    .col(timestamp_with_time_zone_null(PartInstanceLocation::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                PartInstanceLocation::Table,
                                PartInstanceLocation::PartInstanceId,
                            )
                            .to(PartInstance::Table, PartInstance::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                PartInstanceLocation::Table,
                                PartInstanceLocation::ChassisSlotId,
                            )
                            .to(ChassisSlot::Table, ChassisSlot::Id),
                    )
                    .to_owned(),
            )
            .await?;

        current_index(
            manager,
            "idx_part_instance_location_current",
            PartInstanceLocation::Table,
            PartInstanceLocation::PartInstanceId,
            PartInstanceLocation::ToDate,
        )
        .await?;

        // 「この機器に挿さっている部品」を引く（機器詳細画面）
        manager
            .create_index(
                Index::create()
                    .name("idx_part_instance_location_target")
                    .table(PartInstanceLocation::Table)
                    .col(PartInstanceLocation::LocationType)
                    .col(PartInstanceLocation::LocationId)
                    .and_where(Expr::col(PartInstanceLocation::ToDate).is_null())
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // FIRMWARE_VERSION（6.2）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(FirmwareVersion::Table)
                    .if_not_exists()
                    .col(pk_auto(FirmwareVersion::Id))
                    // Device / PartInstance。多態のため外部キーなし
                    .col(string(FirmwareVersion::ItemType))
                    .col(integer(FirmwareVersion::ItemId))
                    .col(string(FirmwareVersion::Component))
                    .col(string(FirmwareVersion::Version))
                    .col(integer_null(FirmwareVersion::WorkOrderId))
                    .col(integer(FirmwareVersion::ChangedBy))
                    .col(timestamp_with_time_zone(FirmwareVersion::FromDate))
                    .col(timestamp_with_time_zone_null(FirmwareVersion::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(FirmwareVersion::Table, FirmwareVersion::ChangedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 対象は (item_type, item_id) の対で決まる
        manager
            .create_index(
                Index::create()
                    .name("idx_firmware_version_current")
                    .table(FirmwareVersion::Table)
                    .col(FirmwareVersion::ItemType)
                    .col(FirmwareVersion::ItemId)
                    .and_where(Expr::col(FirmwareVersion::ToDate).is_null())
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // DEVICE_STACK（8.6）— 履歴
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(DeviceStack::Table)
                    .if_not_exists()
                    .col(pk_auto(DeviceStack::Id))
                    // スタック全体を表す device_type="Logical" のDEVICE
                    .col(integer(DeviceStack::LogicalDeviceId))
                    // 実際の筐体（device_type="Physical"）
                    .col(integer(DeviceStack::MemberDeviceId))
                    .col(integer(DeviceStack::MemberNumber))
                    .col(timestamp_with_time_zone(DeviceStack::FromDate))
                    .col(timestamp_with_time_zone_null(DeviceStack::ToDate))
                    .foreign_key(
                        ForeignKey::create()
                            .from(DeviceStack::Table, DeviceStack::LogicalDeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(DeviceStack::Table, DeviceStack::MemberDeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 「このスタックの構成筐体」と「この筐体が属するスタック」の双方を引く
        current_index(
            manager,
            "idx_device_stack_logical_current",
            DeviceStack::Table,
            DeviceStack::LogicalDeviceId,
            DeviceStack::ToDate,
        )
        .await?;
        current_index(
            manager,
            "idx_device_stack_member_current",
            DeviceStack::Table,
            DeviceStack::MemberDeviceId,
            DeviceStack::ToDate,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            DeviceStack::Table.into_iden(),
            FirmwareVersion::Table.into_iden(),
            PartInstanceLocation::Table.into_iden(),
            PartInstance::Table.into_iden(),
            DeviceAssignment::Table.into_iden(),
            Device::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}

/// 外部キー列などへの通常のインデックス。両DBとも自動では張らない（24.3）。
async fn index<T, C>(
    manager: &SchemaManager<'_>,
    name: &str,
    table: T,
    column: C,
) -> Result<(), DbErr>
where
    T: IntoIden + 'static,
    C: IntoIden + 'static,
{
    manager
        .create_index(
            Index::create()
                .name(name)
                .table(table)
                .col(column)
                .to_owned(),
        )
        .await
}

/// 履歴テーブルの「現在有効な行」を引くための部分インデックス（設計書24.3）。
///
/// **`WHERE to_date IS NULL` を付けるのが要点。**本設計で最も頻出するのは
/// 「いまどうなっているか」の問い合わせであり、履歴が伸びても索引は伸びない。
/// 部分インデックスはPostgreSQL・SQLiteのどちらも対応する（14.2で確認済み）。
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
enum Configuration {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PartCatalog {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum ChassisSlot {
    Table,
    Id,
}

/// 列名をそのまま識別子にするため、`DeviceType` のように接頭辞が重なる。
/// **改名するとDBの列名が変わる**ので、lintの側を黙らせる。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum Device {
    Table,
    Id,
    Uid,
    ExternalId,
    MergedIntoDeviceId,
    MergedAt,
    ConfigurationId,
    DeviceType,
    DeviceCategory,
    Hostname,
    SerialNumber,
    AssetNumber,
    PowerWatt,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum DeviceAssignment {
    Table,
    Id,
    DeviceId,
    LocationType,
    LocationId,
    WorkOrderId,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum PartInstance {
    Table,
    Id,
    PartCatalogId,
    SerialNumber,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PartInstanceLocation {
    Table,
    Id,
    PartInstanceId,
    LocationType,
    LocationId,
    ChassisSlotId,
    WorkOrderId,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum FirmwareVersion {
    Table,
    Id,
    ItemType,
    ItemId,
    Component,
    Version,
    WorkOrderId,
    ChangedBy,
    FromDate,
    ToDate,
}

#[derive(DeriveIden)]
enum DeviceStack {
    Table,
    Id,
    LogicalDeviceId,
    MemberDeviceId,
    MemberNumber,
    FromDate,
    ToDate,
}
