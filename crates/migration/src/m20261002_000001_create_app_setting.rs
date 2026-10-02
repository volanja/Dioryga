//! アプリ全体の設定（`APP_SETTING`）を足す（#217、設計書16.1）。
//!
//! # 1行だけの表
//!
//! 行は `id = 1` の1行だけを使う。**行が無いのは、まだ初回セットアップを
//! していないDB**である。最初の System Admin を作るときに、倉庫用のプロジェクト
//! とあわせて作る。
//!
//! # 倉庫プロジェクトの印ではない
//!
//! `default_project_id` は**新しい利用者を加える先**である（#196、#218）。
//! プロジェクトの側に「倉庫かどうか」の印を持たない。倉庫用のプロジェクトも、
//! ダッシュボード・マイルストーン・コストは通常のプロジェクトと同じに出す。

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AppSetting::Table)
                    .if_not_exists()
                    // 自動採番にしない。使うのは id = 1 の1行だけ
                    .col(integer(AppSetting::Id).primary_key())
                    .col(integer(AppSetting::DefaultProjectId))
                    .col(timestamp_with_time_zone(AppSetting::CreatedAt))
                    .col(timestamp_with_time_zone(AppSetting::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(AppSetting::Table, AppSetting::DefaultProjectId)
                            .to(Project::Table, Project::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 外部キーの索引（24.3）
        manager
            .create_index(
                Index::create()
                    .name("idx_app_setting_default_project_id")
                    .table(AppSetting::Table)
                    .col(AppSetting::DefaultProjectId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AppSetting::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum AppSetting {
    Table,
    Id,
    DefaultProjectId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Project {
    Table,
    Id,
}
