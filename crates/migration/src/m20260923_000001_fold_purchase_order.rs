//! 発注の2テーブルを `PURCHASE` 1つに畳む（#181、設計書10.2）。
//!
//! | 旧 | 新 |
//! |---|---|
//! | `PURCHASE_ORDER`（発注番号・発注日・発注先・通貨） | なし |
//! | `PURCHASE_ORDER_ITEM`（明細・数量・単価） | `PURCHASE`（品目ごと1行） |
//! | `MAINTENANCE_CONTRACT.purchase_order_id`（外部キー） | `order_number`（文字列） |
//!
//! **発注番号は自由入力で、重複してよい。**1つの注文で10台買えば同じ番号の行が
//! 10本並び、注文単位の合計は番号で寄せれば出る。親の行を持つ理由が無い。
//!
//! - **発注日を持たず、取得日（`acquired_on`）を持つ。**発注日はどの集計にも
//!   画面にも使っていなかった。償却の開始日は取得日である
//! - **数量を持たない。**台帳が数えるのは `DEVICE` / `PART_INSTANCE` という実体で
//!   あり、購入の記録が個数を持つと二重になる
//! - **通貨を持たない。**`PROJECT.currency` で表す（設計書5.4）
//! - **購入元は自由入力で、`VENDOR` を参照しない。**代理店・商社から買うのが普通で、
//!   カタログのベンダーに販売店が混ざる
//!
//! # 既存の行は移さない
//!
//! **未リリースで既存の利用者がいないため、子から順に落として作り直す。**
//! `MAINTENANCE_CONTRACT.purchase_order_id` は外部キーを持っており、**SQLiteでは
//! 外部キーのある列を `DROP COLUMN` できない**（`foreign_keys=1` の下で
//! 「unknown column in foreign key definition」になる）。行を保つにはSQLiteだけ
//! テーブルを作り直す手順が要り、DBごとの分岐になる。守る行が無い以上、両DBで同じ
//! 手順に揃える方を採った。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 子から落とす。保守契約の品目 → 保守契約 → 発注明細 → 発注
        for table in [
            MaintenanceContractItem::Table.into_iden(),
            MaintenanceContract::Table.into_iden(),
            PurchaseOrderItem::Table.into_iden(),
            PurchaseOrder::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }

        // -------------------------------------------------------------------
        // PURCHASE
        // -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Purchase::Table)
                    .if_not_exists()
                    .col(pk_auto(Purchase::Id))
                    // Device / PartInstance / SoftwareInstance。多態のため外部キーなし
                    .col(string(Purchase::ItemType))
                    .col(integer(Purchase::ItemId))
                    // **自由入力で、重複してよい。**一意の索引を張らない
                    .col(string_null(Purchase::OrderNumber))
                    // 現品を受け取った日。償却はこの日から始まる
                    .col(date_null(Purchase::AcquiredOn))
                    // **最小通貨単位の整数**（24.2.1）。その品目1つの金額
                    .col(big_integer(Purchase::Amount).default(0))
                    // 買った相手。`VENDOR` を参照しない
                    .col(string_null(Purchase::Supplier))
                    .col(timestamp_with_time_zone(Purchase::CreatedAt))
                    .col(timestamp_with_time_zone(Purchase::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        // 「この機器の購入の記録」を引く（機器詳細）。多態的参照は複合索引で（24.3）
        polymorphic_index(
            manager,
            "idx_purchase_target",
            Purchase::Table,
            Purchase::ItemType,
            Purchase::ItemId,
        )
        .await?;
        // 注文単位で寄せる。**一意ではない**
        index(
            manager,
            "idx_purchase_order_number",
            Purchase::Table,
            Purchase::OrderNumber,
        )
        .await?;

        // -------------------------------------------------------------------
        // MAINTENANCE_CONTRACT / MAINTENANCE_CONTRACT_ITEM（作り直し）
        // -------------------------------------------------------------------
        create_maintenance_contract(manager, ContractOrder::Number).await?;
        create_maintenance_contract_item(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            MaintenanceContractItem::Table.into_iden(),
            MaintenanceContract::Table.into_iden(),
            Purchase::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }

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
                    .col(string(PurchaseOrderItem::ItemType))
                    .col(integer(PurchaseOrderItem::ItemId))
                    .col(integer(PurchaseOrderItem::Quantity).default(1))
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
        polymorphic_index(
            manager,
            "idx_purchase_order_item_target",
            PurchaseOrderItem::Table,
            PurchaseOrderItem::ItemType,
            PurchaseOrderItem::ItemId,
        )
        .await?;

        create_maintenance_contract(manager, ContractOrder::ForeignKey).await?;
        create_maintenance_contract_item(manager).await
    }
}

/// 保守契約が発注をどう持つか。
enum ContractOrder {
    /// 新：発注番号の文字列
    Number,
    /// 旧：`PURCHASE_ORDER` への外部キー
    ForeignKey,
}

async fn create_maintenance_contract(
    manager: &SchemaManager<'_>,
    order: ContractOrder,
) -> Result<(), DbErr> {
    let mut table = Table::create();
    table
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
        .col(string(MaintenanceContract::FailureContact).default(""));
    match order {
        // 契約自体の発注番号。自由入力（10.2）
        ContractOrder::Number => {
            table.col(string_null(MaintenanceContract::OrderNumber));
        }
        ContractOrder::ForeignKey => {
            table
                .col(integer_null(MaintenanceContract::PurchaseOrderId))
                .foreign_key(
                    ForeignKey::create()
                        .from(
                            MaintenanceContract::Table,
                            MaintenanceContract::PurchaseOrderId,
                        )
                        .to(PurchaseOrder::Table, PurchaseOrder::Id),
                );
        }
    }
    table
        .col(timestamp_with_time_zone(MaintenanceContract::CreatedAt))
        .col(timestamp_with_time_zone(MaintenanceContract::UpdatedAt))
        .foreign_key(
            ForeignKey::create()
                .from(MaintenanceContract::Table, MaintenanceContract::VendorId)
                .to(Vendor::Table, Vendor::Id),
        );
    let with_fk = matches!(order, ContractOrder::ForeignKey);
    manager.create_table(table.to_owned()).await?;

    index(
        manager,
        "idx_maintenance_contract_vendor",
        MaintenanceContract::Table,
        MaintenanceContract::VendorId,
    )
    .await?;
    if with_fk {
        index(
            manager,
            "idx_maintenance_contract_purchase_order",
            MaintenanceContract::Table,
            MaintenanceContract::PurchaseOrderId,
        )
        .await?;
    }
    // **期限間近の契約を引く**（プロジェクトダッシュボード、通知）
    index(
        manager,
        "idx_maintenance_contract_end_date",
        MaintenanceContract::Table,
        MaintenanceContract::EndDate,
    )
    .await
}

async fn create_maintenance_contract_item(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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
    .await
}

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
enum Vendor {
    Table,
    Id,
}

// --- 本マイグレーションで作成する ---

#[derive(DeriveIden)]
enum Purchase {
    Table,
    Id,
    ItemType,
    ItemId,
    OrderNumber,
    AcquiredOn,
    Amount,
    Supplier,
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
    OrderNumber,
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

// --- 本マイグレーションで落とす（down で作り直す） ---

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

#[allow(clippy::enum_variant_names)]
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
