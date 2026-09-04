//! QCD（費用・マイルストーン）のテーブルを作成する（設計書10章）。
//!
//! # 金額は最小通貨単位の整数で持つ
//!
//! **`DECIMAL` を使わない**（24.2.1）。SQLiteに `DECIMAL` 型が存在せず、
//! `REAL` に落とすと丸め誤差が生じる。10.3の年間コストダッシュボードは按分した
//! 金額を多数合算するため、誤差が累積する。
//!
//! 小数点以下の桁数は `PROJECT.currency` から決まる（`currency` モジュール）。
//! `docs/spec/schema.md` は `(decimal)` と書いていたが、24.2.1より前の記述
//! であり、本マイグレーションに合わせて修正した。
//!
//! # 保存しないもの
//!
//! - **`PURCHASE_ORDER.amount`**：明細の合計から計算する（旧B-5）
//! - **`FIXED_ASSET.disposal_date` と簿価**：廃棄は `DEVICE_ASSIGNMENT` の
//!   `Disposed` 行から、簿価は取得価額と経過期間から計算する（旧B-1、不変条件2）
//!
//! # 日付は `date` のまま
//!
//! `MILESTONE` の粒度は意図的に `date` とする（C-5）。他の履歴テーブルを
//! `datetime` 化した理由（同日内の順序が必要）が当てはまらない。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // -------------------------------------------------------------------
        // PURCHASE_ORDER / PURCHASE_ORDER_ITEM
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(PurchaseOrder::Table)
                    .if_not_exists()
                    .col(pk_auto(PurchaseOrder::Id))
                    .col(string(PurchaseOrder::OrderNumber))
                    .col(date(PurchaseOrder::OrderDate))
                    .col(integer(PurchaseOrder::VendorId))
                    .col(string(PurchaseOrder::Currency))
                    // **`amount` を持たない。**明細合計から計算する（旧B-5）
                    .col(timestamp_with_time_zone(PurchaseOrder::CreatedAt))
                    .col(timestamp_with_time_zone(PurchaseOrder::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(PurchaseOrder::Table, PurchaseOrder::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_purchase_order_vendor",
            PurchaseOrder::Table,
            PurchaseOrder::VendorId,
        )
        .await?;
        index(
            manager,
            "idx_purchase_order_number",
            PurchaseOrder::Table,
            PurchaseOrder::OrderNumber,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(PurchaseOrderItem::Table)
                    .if_not_exists()
                    .col(pk_auto(PurchaseOrderItem::Id))
                    .col(integer(PurchaseOrderItem::PurchaseOrderId))
                    // Device / PartInstance / SoftwareInstance。多態のため外部キーなし
                    .col(string(PurchaseOrderItem::ItemType))
                    .col(integer(PurchaseOrderItem::ItemId))
                    .col(integer(PurchaseOrderItem::Quantity).default(1))
                    // **最小通貨単位の整数**（24.2.1）。DECIMALにしない
                    .col(big_integer(PurchaseOrderItem::UnitPrice).default(0))
                    .col(timestamp_with_time_zone(PurchaseOrderItem::CreatedAt))
                    .col(timestamp_with_time_zone(PurchaseOrderItem::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(PurchaseOrderItem::Table, PurchaseOrderItem::PurchaseOrderId)
                            .to(PurchaseOrder::Table, PurchaseOrder::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_purchase_order_item_order",
            PurchaseOrderItem::Table,
            PurchaseOrderItem::PurchaseOrderId,
        )
        .await?;
        // 「この機器の発注明細」を引く（機器詳細）。多態的参照は複合索引で（24.3）
        polymorphic_index(
            manager,
            "idx_purchase_order_item_target",
            PurchaseOrderItem::Table,
            PurchaseOrderItem::ItemType,
            PurchaseOrderItem::ItemId,
        )
        .await?;

        // -------------------------------------------------------------------
        // FIXED_ASSET
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(FixedAsset::Table)
                    .if_not_exists()
                    .col(pk_auto(FixedAsset::Id))
                    .col(string(FixedAsset::ItemType))
                    .col(integer(FixedAsset::ItemId))
                    .col(big_integer(FixedAsset::AcquisitionCost).default(0))
                    // straight_line / declining_balance
                    .col(string(FixedAsset::DepreciationMethod))
                    .col(integer(FixedAsset::UsefulLifeYears))
                    .col(date(FixedAsset::AcquisitionDate))
                    // **`disposal_date` と簿価を持たない。**廃棄は
                    // DEVICE_ASSIGNMENT の Disposed 行から、簿価は計算で求める
                    .col(timestamp_with_time_zone(FixedAsset::CreatedAt))
                    .col(timestamp_with_time_zone(FixedAsset::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        polymorphic_index(
            manager,
            "idx_fixed_asset_target",
            FixedAsset::Table,
            FixedAsset::ItemType,
            FixedAsset::ItemId,
        )
        .await?;

        // -------------------------------------------------------------------
        // MAINTENANCE_CONTRACT / MAINTENANCE_CONTRACT_ITEM
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(MaintenanceContract::Table)
                    .if_not_exists()
                    .col(pk_auto(MaintenanceContract::Id))
                    .col(string(MaintenanceContract::ContractNumber))
                    .col(integer(MaintenanceContract::VendorId))
                    .col(date(MaintenanceContract::StartDate))
                    // **保守期限はこの列のみが正**（旧B-2）
                    .col(date(MaintenanceContract::EndDate))
                    .col(big_integer(MaintenanceContract::Amount).default(0))
                    .col(string(MaintenanceContract::QuoteContact).default(""))
                    .col(string(MaintenanceContract::FailureContact).default(""))
                    .col(integer_null(MaintenanceContract::PurchaseOrderId))
                    .col(timestamp_with_time_zone(MaintenanceContract::CreatedAt))
                    .col(timestamp_with_time_zone(MaintenanceContract::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(MaintenanceContract::Table, MaintenanceContract::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                MaintenanceContract::Table,
                                MaintenanceContract::PurchaseOrderId,
                            )
                            .to(PurchaseOrder::Table, PurchaseOrder::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_maintenance_contract_vendor",
            MaintenanceContract::Table,
            MaintenanceContract::VendorId,
        )
        .await?;
        index(
            manager,
            "idx_maintenance_contract_purchase_order",
            MaintenanceContract::Table,
            MaintenanceContract::PurchaseOrderId,
        )
        .await?;
        // **期限間近の契約を引く**（プロジェクトダッシュボード、通知）。
        // この索引が無いと、契約が増えたときに全件走査になる
        index(
            manager,
            "idx_maintenance_contract_end_date",
            MaintenanceContract::Table,
            MaintenanceContract::EndDate,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(MaintenanceContractItem::Table)
                    .if_not_exists()
                    .col(pk_auto(MaintenanceContractItem::Id))
                    .col(integer(MaintenanceContractItem::MaintenanceContractId))
                    .col(string(MaintenanceContractItem::ItemType))
                    .col(integer(MaintenanceContractItem::ItemId))
                    .col(timestamp_with_time_zone(MaintenanceContractItem::CreatedAt))
                    .col(timestamp_with_time_zone(MaintenanceContractItem::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                MaintenanceContractItem::Table,
                                MaintenanceContractItem::MaintenanceContractId,
                            )
                            .to(MaintenanceContract::Table, MaintenanceContract::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_maintenance_contract_item_contract",
            MaintenanceContractItem::Table,
            MaintenanceContractItem::MaintenanceContractId,
        )
        .await?;
        polymorphic_index(
            manager,
            "idx_maintenance_contract_item_target",
            MaintenanceContractItem::Table,
            MaintenanceContractItem::ItemType,
            MaintenanceContractItem::ItemId,
        )
        .await?;

        // -------------------------------------------------------------------
        // RECURRING_COST
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(RecurringCost::Table)
                    .if_not_exists()
                    .col(pk_auto(RecurringCost::Id))
                    // MountContainer / Project。ラック料金や回線費用など
                    .col(string(RecurringCost::ItemType))
                    .col(integer(RecurringCost::ItemId))
                    .col(string(RecurringCost::CostType))
                    .col(integer_null(RecurringCost::VendorId))
                    .col(big_integer(RecurringCost::Amount).default(0))
                    // Monthly / Annual
                    .col(string(RecurringCost::BillingCycle))
                    .col(date(RecurringCost::StartDate))
                    // null = 継続中
                    .col(date_null(RecurringCost::EndDate))
                    .col(integer(RecurringCost::CreatedBy))
                    .col(timestamp_with_time_zone(RecurringCost::CreatedAt))
                    .col(timestamp_with_time_zone(RecurringCost::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(RecurringCost::Table, RecurringCost::VendorId)
                            .to(Vendor::Table, Vendor::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(RecurringCost::Table, RecurringCost::CreatedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        polymorphic_index(
            manager,
            "idx_recurring_cost_target",
            RecurringCost::Table,
            RecurringCost::ItemType,
            RecurringCost::ItemId,
        )
        .await?;
        index(
            manager,
            "idx_recurring_cost_vendor",
            RecurringCost::Table,
            RecurringCost::VendorId,
        )
        .await?;
        index(
            manager,
            "idx_recurring_cost_created_by",
            RecurringCost::Table,
            RecurringCost::CreatedBy,
        )
        .await?;

        // -------------------------------------------------------------------
        // MILESTONE / MILESTONE_DEVICE（10.4）
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Milestone::Table)
                    .if_not_exists()
                    .col(pk_auto(Milestone::Id))
                    .col(integer(Milestone::ProjectId))
                    // ServiceStart / ServiceUpdate / ServiceMaintenance / ServiceEnd
                    .col(string(Milestone::MilestoneType))
                    // **`date` のまま。**同日内の順序を問わないため（C-5）
                    .col(date(Milestone::PlannedDate))
                    // **予定と実績を別の列で持つ。**片方に上書きすると
                    // 「当初いつの予定だったか」が失われ、QCDのDが測れない（5.1）
                    .col(date_null(Milestone::ActualDate))
                    .col(string(Milestone::Status))
                    .col(string(Milestone::Description).default(""))
                    .col(timestamp_with_time_zone(Milestone::CreatedAt))
                    .col(timestamp_with_time_zone(Milestone::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Milestone::Table, Milestone::ProjectId)
                            .to(Project::Table, Project::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_milestone_project",
            Milestone::Table,
            Milestone::ProjectId,
        )
        .await?;
        // 期限間近のマイルストーンを引く（プロジェクトダッシュボード）
        index(
            manager,
            "idx_milestone_planned_date",
            Milestone::Table,
            Milestone::PlannedDate,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(MilestoneDevice::Table)
                    .if_not_exists()
                    .col(pk_auto(MilestoneDevice::Id))
                    .col(integer(MilestoneDevice::MilestoneId))
                    .col(integer(MilestoneDevice::DeviceId))
                    // addition / relocation / removal
                    .col(string(MilestoneDevice::ChangeType))
                    .col(integer_null(MilestoneDevice::WorkOrderId))
                    .col(timestamp_with_time_zone(MilestoneDevice::CreatedAt))
                    .col(timestamp_with_time_zone(MilestoneDevice::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(MilestoneDevice::Table, MilestoneDevice::MilestoneId)
                            .to(Milestone::Table, Milestone::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(MilestoneDevice::Table, MilestoneDevice::DeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_milestone_device_milestone",
            MilestoneDevice::Table,
            MilestoneDevice::MilestoneId,
        )
        .await?;
        index(
            manager,
            "idx_milestone_device_device",
            MilestoneDevice::Table,
            MilestoneDevice::DeviceId,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            MilestoneDevice::Table.into_iden(),
            Milestone::Table.into_iden(),
            RecurringCost::Table.into_iden(),
            MaintenanceContractItem::Table.into_iden(),
            MaintenanceContract::Table.into_iden(),
            FixedAsset::Table.into_iden(),
            PurchaseOrderItem::Table.into_iden(),
            PurchaseOrder::Table.into_iden(),
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

/// 多態的参照への複合インデックス（設計書24.3）。
///
/// `item_type`/`item_id` は外部キーにできないため、**索引だけが逆引きの手段**に
/// なる。「この機器の発注明細・固定資産・保守契約」は機器詳細で必ず引く。
async fn polymorphic_index<T, A, B>(
    manager: &SchemaManager<'_>,
    name: &str,
    table: T,
    item_type: A,
    item_id: B,
) -> Result<(), DbErr>
where
    T: IntoIden + 'static,
    A: IntoIden + 'static,
    B: IntoIden + 'static,
{
    manager
        .create_index(
            Index::create()
                .name(name)
                .table(table)
                .col(item_type)
                .col(item_id)
                .to_owned(),
        )
        .await
}

// --- 既存のマイグレーションで作成済み。外部キーの参照先としてのみ使う ---

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Project {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Vendor {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Device {
    Table,
    Id,
}

// --- 本マイグレーションで作成する ---

/// 列名をそのまま識別子にするため接頭辞が重なる。改名するとDBの列名が変わる。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum PurchaseOrder {
    Table,
    Id,
    OrderNumber,
    OrderDate,
    VendorId,
    Currency,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PurchaseOrderItem {
    Table,
    Id,
    PurchaseOrderId,
    ItemType,
    ItemId,
    Quantity,
    UnitPrice,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum FixedAsset {
    Table,
    Id,
    ItemType,
    ItemId,
    AcquisitionCost,
    DepreciationMethod,
    UsefulLifeYears,
    AcquisitionDate,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MaintenanceContract {
    Table,
    Id,
    ContractNumber,
    VendorId,
    StartDate,
    EndDate,
    Amount,
    QuoteContact,
    FailureContact,
    PurchaseOrderId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MaintenanceContractItem {
    Table,
    Id,
    MaintenanceContractId,
    ItemType,
    ItemId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum RecurringCost {
    Table,
    Id,
    ItemType,
    ItemId,
    CostType,
    VendorId,
    Amount,
    BillingCycle,
    StartDate,
    EndDate,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

/// 列名をそのまま識別子にするため `MilestoneType` の接頭辞が重なる。
/// 改名するとDBの列名が変わる。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum Milestone {
    Table,
    Id,
    ProjectId,
    MilestoneType,
    PlannedDate,
    ActualDate,
    Status,
    Description,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MilestoneDevice {
    Table,
    Id,
    MilestoneId,
    DeviceId,
    ChangeType,
    WorkOrderId,
    CreatedAt,
    UpdatedAt,
}
