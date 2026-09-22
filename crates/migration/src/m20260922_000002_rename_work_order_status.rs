//! 変更管理チケットの状態の語彙を改める（#174）。
//!
//! | 旧 | 新 | 理由 |
//! |---|---|---|
//! | `executing` | `in_progress` | 「プログラムが実行中」の含みが強い。作業の進行中は `in_progress` |
//! | `aborted` | `cancelled` | `abort` は処理を異常終了させる機械の語。人が取りやめた案件は `cancelled` |
//!
//! **`MILESTONE.status` と `PROJECT.closure_reason` は既に `cancelled` / `Cancelled`**
//! であり、チケットだけが `aborted` だった。10.4で納期遵守率を見る際に並べて
//! 集計するため、同じ事実は同じ語にする。
//!
//! # 列名も改める
//!
//! `aborted_at` / `aborted_reason` は、値だけ直すと語が食い違う。
//! **SQLiteの `RENAME COLUMN` は3.25以降で使える**（24.3の制約は制約の追加であり、
//! 列の改名は両DBで通る）。

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// （旧, 新）
const 対応: &[(&str, &str)] = &[("executing", "in_progress"), ("aborted", "cancelled")];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (旧, 新) in 対応 {
            書き換える(manager, 旧, 新).await?;
        }
        改名する(manager, WorkOrder::AbortedAt, WorkOrder::CancelledAt).await?;
        改名する(
            manager,
            WorkOrder::AbortedReason,
            WorkOrder::CancelledReason,
        )
        .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        改名する(manager, WorkOrder::CancelledAt, WorkOrder::AbortedAt).await?;
        改名する(
            manager,
            WorkOrder::CancelledReason,
            WorkOrder::AbortedReason,
        )
        .await?;
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
                .table(WorkOrder::Table)
                .value(WorkOrder::Status, to)
                .and_where(Expr::col(WorkOrder::Status).eq(from))
                .to_owned(),
        )
        .await
}

async fn 改名する(
    manager: &SchemaManager<'_>,
    from: WorkOrder,
    to: WorkOrder,
) -> Result<(), DbErr> {
    manager
        .alter_table(
            Table::alter()
                .table(WorkOrder::Table)
                .rename_column(from, to)
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum WorkOrder {
    Table,
    Status,
    AbortedAt,
    AbortedReason,
    CancelledAt,
    CancelledReason,
}
