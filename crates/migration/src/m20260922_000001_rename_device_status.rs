//! 機器・部品の状態の語彙を改める（#173）。
//!
//! | 旧 | 新 | 理由 |
//! |---|---|---|
//! | `plan` | `planned` | 名詞で状態を表していない。`WORK_ORDER`・`MILESTONE` は既に `planned` |
//! | `building` | `provisioning` | **この製品では「建物」と読めてしまう。**設置場所を扱う台帳である |
//! | `repair` | `repairing` | 名詞。進行中の状態としては進行形が自然 |
//! | `broken` | `failed` | 口語的。機器の文脈では `failed` が業界の語 |
//!
//! 語彙はDB制約にせずアプリケーションで検証する（設計書8.6）ため、表を改めた
//! だけでは既存の行が語彙外の値として残る。#125（`Vpn`→`VPN`）と同じ形である。

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// （旧, 新）
const 対応: &[(&str, &str)] = &[
    ("plan", "planned"),
    ("building", "provisioning"),
    ("repair", "repairing"),
    ("broken", "failed"),
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
                .table(Device::Table)
                .value(Device::Status, to)
                .and_where(Expr::col(Device::Status).eq(from))
                .to_owned(),
        )
        .await?;
    manager
        .exec_stmt(
            Query::update()
                .table(PartInstance::Table)
                .value(PartInstance::Status, to)
                .and_where(Expr::col(PartInstance::Status).eq(from))
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum Device {
    Table,
    Status,
}

#[derive(DeriveIden)]
enum PartInstance {
    Table,
    Status,
}
