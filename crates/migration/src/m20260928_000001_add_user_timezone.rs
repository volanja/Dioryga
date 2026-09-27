//! `USER` に表示タイムゾーン（`timezone`）を足す（#210、設計書24.2.3）。
//!
//! 利用者ごとに個人設定で選ぶ。**null はサーバ設定の `timezone` を既定として
//! 使う**——既存の利用者を一律に書き換えず、設定ファイルの値に従わせる。
//! 値は IANA のタイムゾーン名（`Asia/Tokyo` 等）で、検証はアプリケーション層で行う。

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
                    .add_column(string_null(AppUser::Timezone))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AppUser::Table)
                    .drop_column(AppUser::Timezone)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Timezone,
}
