//! 共有カタログのテーブルを作成する（設計書6章、8章、9章、18章）。
//!
//! # プロジェクトに属さない
//!
//! カタログはプロジェクト横断の共有マスタであり、`project_id` を持たない（18.1）。
//! 「同じ型番の機器を各プロジェクトが別々に登録する」ことを避けるための構造である。
//!
//! # 語彙をDB制約にしない
//!
//! `device_category` や `slot_type` などの語彙は `CHECK` 制約にしない。値を1つ
//! 増やすたびにマイグレーションが要る形を避けるため、検証はアプリケーション層で
//! 行う（`vocabularies.md` の原則、設計書8.6）。
//!
//! # v1で画面を作らないテーブルも含む
//!
//! `CABLE_CATALOG` 等はv1の画面・取込の対象外だが、テーブル定義は作る。後から
//! 足すと既存データの作り直しが必要になるため（CLAUDE.md）。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // -------------------------------------------------------------------
        // VENDOR（18.3）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Vendor::Table)
                    .if_not_exists()
                    .col(pk_auto(Vendor::Id))
                    // 表記ゆれを防ぐためのマスタなので、名称の重複を許さない（18.3）
                    .col(string(Vendor::Name).unique_key())
                    .col(integer(Vendor::CreatedBy))
                    .col(timestamp_with_time_zone(Vendor::CreatedAt))
                    .col(timestamp_with_time_zone(Vendor::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Vendor::Table, Vendor::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // CHASSIS_MODEL / CHASSIS_SLOT（6.2、8.6）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(ChassisModel::Table)
                    .if_not_exists()
                    .col(pk_auto(ChassisModel::Id))
                    .col(integer(ChassisModel::VendorId))
                    .col(string(ChassisModel::ModelName))
                    .col(string(ChassisModel::DeviceCategory))
                    // mount_form=Surface のときは意味を持たない
                    .col(integer(ChassisModel::HeightU).default(0))
                    .col(string(ChassisModel::MountForm))
                    // mount_form=RackU のときのみ意味を持つ
                    .col(string_null(ChassisModel::RackWidth))
                    .col(integer(ChassisModel::CreatedBy))
                    .col(timestamp_with_time_zone(ChassisModel::CreatedAt))
                    .col(timestamp_with_time_zone(ChassisModel::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ChassisModel::Table, ChassisModel::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ChassisModel::Table, ChassisModel::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 自然キー（設計書6.2）
        manager
            .create_index(
                Index::create()
                    .name("uq_chassis_model_vendor_model_name")
                    .table(ChassisModel::Table)
                    .col(ChassisModel::VendorId)
                    .col(ChassisModel::ModelName)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ChassisSlot::Table)
                    .if_not_exists()
                    .col(pk_auto(ChassisSlot::Id))
                    .col(integer(ChassisSlot::ChassisModelId))
                    .col(string(ChassisSlot::SlotType))
                    .col(string(ChassisSlot::SlotLabel))
                    .col(timestamp_with_time_zone(ChassisSlot::CreatedAt))
                    .col(timestamp_with_time_zone(ChassisSlot::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ChassisSlot::Table, ChassisSlot::ChassisModelId)
                            .to(ChassisModel::Table, ChassisModel::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_chassis_slot_model",
            ChassisSlot::Table,
            ChassisSlot::ChassisModelId,
        )
        .await?;

        // -------------------------------------------------------------------
        // PART_CATALOG / PART_PORT_SLOT（6.2、6.4、8.3、8.7）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(PartCatalog::Table)
                    .if_not_exists()
                    .col(pk_auto(PartCatalog::Id))
                    .col(string(PartCatalog::Category))
                    .col(integer(PartCatalog::VendorId))
                    .col(string(PartCatalog::PartNumber))
                    // 集計に使うためカラム化する。該当しない部品ではnull（6.4）
                    .col(integer_null(PartCatalog::CoreCount))
                    .col(integer_null(PartCatalog::CapacityGb))
                    // 集計しない仕様はJSONへ。両DBともTEXT（24.2.3）
                    .col(text(PartCatalog::SpecJson).default("{}"))
                    .col(integer(PartCatalog::CreatedBy))
                    .col(timestamp_with_time_zone(PartCatalog::CreatedAt))
                    .col(timestamp_with_time_zone(PartCatalog::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(PartCatalog::Table, PartCatalog::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(PartCatalog::Table, PartCatalog::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_part_catalog_vendor_part_number")
                    .table(PartCatalog::Table)
                    .col(PartCatalog::VendorId)
                    .col(PartCatalog::PartNumber)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PartPortSlot::Table)
                    .if_not_exists()
                    .col(pk_auto(PartPortSlot::Id))
                    .col(integer(PartPortSlot::PartCatalogId))
                    .col(string(PartPortSlot::PortKind))
                    .col(string(PartPortSlot::PortLabel))
                    // コネクタ形状は端ごとに異なりうる（8.7）
                    .col(string(PartPortSlot::ConnectorType))
                    // port_kind=Network のときのみ意味を持つ
                    .col(string_null(PartPortSlot::PortSpeed))
                    .col(timestamp_with_time_zone(PartPortSlot::CreatedAt))
                    .col(timestamp_with_time_zone(PartPortSlot::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(PartPortSlot::Table, PartPortSlot::PartCatalogId)
                            .to(PartCatalog::Table, PartCatalog::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_part_port_slot_part_catalog",
            PartPortSlot::Table,
            PartPortSlot::PartCatalogId,
        )
        .await?;

        // -------------------------------------------------------------------
        // CONFIGURATION / CONFIGURATION_PART（6.2）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Configuration::Table)
                    .if_not_exists()
                    .col(pk_auto(Configuration::Id))
                    .col(integer(Configuration::ChassisModelId))
                    .col(string(Configuration::Name))
                    .col(integer(Configuration::CreatedBy))
                    .col(timestamp_with_time_zone(Configuration::CreatedAt))
                    .col(timestamp_with_time_zone(Configuration::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Configuration::Table, Configuration::ChassisModelId)
                            .to(ChassisModel::Table, ChassisModel::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Configuration::Table, Configuration::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_configuration_chassis_model",
            Configuration::Table,
            Configuration::ChassisModelId,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(ConfigurationPart::Table)
                    .if_not_exists()
                    .col(pk_auto(ConfigurationPart::Id))
                    .col(integer(ConfigurationPart::ConfigurationId))
                    .col(integer(ConfigurationPart::PartCatalogId))
                    .col(integer(ConfigurationPart::Quantity).default(1))
                    .col(timestamp_with_time_zone(ConfigurationPart::CreatedAt))
                    .col(timestamp_with_time_zone(ConfigurationPart::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ConfigurationPart::Table, ConfigurationPart::ConfigurationId)
                            .to(Configuration::Table, Configuration::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ConfigurationPart::Table, ConfigurationPart::PartCatalogId)
                            .to(PartCatalog::Table, PartCatalog::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 同じ構成に同じ部品が2行現れないようにする。数量はquantityで表す
        manager
            .create_index(
                Index::create()
                    .name("uq_configuration_part")
                    .table(ConfigurationPart::Table)
                    .col(ConfigurationPart::ConfigurationId)
                    .col(ConfigurationPart::PartCatalogId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // CABLE_CATALOG / CABLE_END_SLOT（8.3、8.7）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(CableCatalog::Table)
                    .if_not_exists()
                    .col(pk_auto(CableCatalog::Id))
                    .col(string(CableCatalog::CableType))
                    // **ミリメートルの整数。**SQLiteにDECIMALが無いため（24.2.1）。
                    // 単位を列名に含めているのは1000倍の取り違えを防ぐため
                    .col(integer_null(CableCatalog::LengthMm))
                    .col(string(CableCatalog::Color).default(""))
                    .col(integer_null(CableCatalog::VendorId))
                    .col(string_null(CableCatalog::PartNumber))
                    .col(integer(CableCatalog::CreatedBy))
                    .col(timestamp_with_time_zone(CableCatalog::CreatedAt))
                    .col(timestamp_with_time_zone(CableCatalog::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableCatalog::Table, CableCatalog::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableCatalog::Table, CableCatalog::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(CableEndSlot::Table)
                    .if_not_exists()
                    .col(pk_auto(CableEndSlot::Id))
                    .col(integer(CableEndSlot::CableCatalogId))
                    // A/B、またはブレイクアウトの Trunk/Branch1..N
                    .col(string(CableEndSlot::EndLabel))
                    // **端ごとに異なるコネクタを持てる。**NEMA 5-15P と C13、
                    // LC と SC のような非対称なケーブルを表すため（8.7）
                    .col(string(CableEndSlot::ConnectorType))
                    .col(string_null(CableEndSlot::PortSpeed))
                    .col(timestamp_with_time_zone(CableEndSlot::CreatedAt))
                    .col(timestamp_with_time_zone(CableEndSlot::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(CableEndSlot::Table, CableEndSlot::CableCatalogId)
                            .to(CableCatalog::Table, CableCatalog::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_cable_end_slot_catalog_label")
                    .table(CableEndSlot::Table)
                    .col(CableEndSlot::CableCatalogId)
                    .col(CableEndSlot::EndLabel)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // -------------------------------------------------------------------
        // SOFTWARE_CATALOG（9.4）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(SoftwareCatalog::Table)
                    .if_not_exists()
                    .col(pk_auto(SoftwareCatalog::Id))
                    .col(string(SoftwareCatalog::Name))
                    .col(integer_null(SoftwareCatalog::VendorId))
                    // バージョンごとに別レコード（9.4）
                    .col(string(SoftwareCatalog::Version))
                    .col(string(SoftwareCatalog::Category))
                    .col(string_null(SoftwareCatalog::Purl))
                    .col(string(SoftwareCatalog::LicenseExpression).default(""))
                    .col(text(SoftwareCatalog::SpecJson).default("{}"))
                    .col(integer(SoftwareCatalog::CreatedBy))
                    .col(timestamp_with_time_zone(SoftwareCatalog::CreatedAt))
                    .col(timestamp_with_time_zone(SoftwareCatalog::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(SoftwareCatalog::Table, SoftwareCatalog::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SoftwareCatalog::Table, SoftwareCatalog::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // purl があればそれが一意。**NULL同士は重複と見なされない**ため、
        // purl を持たない行がいくつあっても衝突しない（両DB共通の挙動）
        manager
            .create_index(
                Index::create()
                    .name("uq_software_catalog_purl")
                    .table(SoftwareCatalog::Table)
                    .col(SoftwareCatalog::Purl)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq_software_catalog_name_vendor_version")
                    .table(SoftwareCatalog::Table)
                    .col(SoftwareCatalog::Name)
                    .col(SoftwareCatalog::VendorId)
                    .col(SoftwareCatalog::Version)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 外部キーの依存があるため作成と逆順に落とす
        for table in [
            SoftwareCatalog::Table.into_iden(),
            CableEndSlot::Table.into_iden(),
            CableCatalog::Table.into_iden(),
            ConfigurationPart::Table.into_iden(),
            Configuration::Table.into_iden(),
            PartPortSlot::Table.into_iden(),
            PartCatalog::Table.into_iden(),
            ChassisSlot::Table.into_iden(),
            ChassisModel::Table.into_iden(),
            Vendor::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}

/// 外部キー列へのインデックス。
///
/// **両DBともFK列に自動でインデックスを張らない**（設計書24.3）。カタログは
/// 親から子を引く参照が多いため、作成時に併せて張る。
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

/// 既存のマイグレーションで作成済み。外部キーの参照先としてのみ使う。
#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Vendor {
    Table,
    Id,
    Name,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ChassisModel {
    Table,
    Id,
    VendorId,
    ModelName,
    DeviceCategory,
    HeightU,
    MountForm,
    RackWidth,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ChassisSlot {
    Table,
    Id,
    ChassisModelId,
    SlotType,
    SlotLabel,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PartCatalog {
    Table,
    Id,
    Category,
    VendorId,
    PartNumber,
    CoreCount,
    CapacityGb,
    SpecJson,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PartPortSlot {
    Table,
    Id,
    PartCatalogId,
    PortKind,
    PortLabel,
    ConnectorType,
    PortSpeed,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Configuration {
    Table,
    Id,
    ChassisModelId,
    Name,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ConfigurationPart {
    Table,
    Id,
    ConfigurationId,
    PartCatalogId,
    Quantity,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum CableCatalog {
    Table,
    Id,
    CableType,
    LengthMm,
    Color,
    VendorId,
    PartNumber,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum CableEndSlot {
    Table,
    Id,
    CableCatalogId,
    EndLabel,
    ConnectorType,
    PortSpeed,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum SoftwareCatalog {
    Table,
    Id,
    Name,
    VendorId,
    Version,
    Category,
    Purl,
    LicenseExpression,
    SpecJson,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}
