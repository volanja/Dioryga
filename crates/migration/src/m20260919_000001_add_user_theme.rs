//! `USER` に表示モード（ライト／ダーク）を足す（#120）。
//!
//! 設計書16.4は「ライト/ダーク両対応（トグル切替）」としているが、現状は
//! CSSの `prefers-color-scheme` だけで決まり、**OSの設定に従うしかない。**
//!
//! # なぜ利用者に持たせるか
//!
//! Cookieに置けばDBを変えずに済み、端末ごとに変えられる。それでも利用者に
//! 持たせるのは、**同じ個人設定にある言語（`locale`）が既にここにある**ためで、
//! 並んだ2つの設定が「端末をまたぐ」「またがない」で違うと説明できない。
//!
//! 既定は `system`（OSに従う）。**現状の見え方を変えない値を既定にする。**

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AppUser::Table)
                    .add_column(string(AppUser::Theme).default("system"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AppUser::Table)
                    .drop_column(AppUser::Theme)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Theme,
}
