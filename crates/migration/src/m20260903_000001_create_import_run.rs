//! 取込の実行記録を作成する（設計書23.7）。
//!
//! **「この不正なデータはどの取込で入ったか」を辿れるようにする**ためのテーブル。
//! `AUDIT_LOG.import_run_id` から参照される。
//!
//! # 取込では行ごとの監査ログを書かない
//!
//! 25万行の取込で25万行の監査ログが生まれると、22.5で指摘した肥大問題を
//! 悪化させる。取込で作られた行はすべて同じ主体・時刻・`import_run_id` を持つ
//! ため、1行ごとに複製しても情報が増えない。**追跡はこのテーブルが担う**（24.4）。
//!
//! # `AUDIT_LOG.import_run_id` に外部キーを張らない
//!
//! 列は基盤テーブルの作成時（`m20260830_000001`）に置いてあるが、SQLiteは
//! 既存テーブルへの制約追加ができない。**両DBで同じスキーマにする**方針
//! （24.2）を優先し、参照整合はアプリケーション層で担保する。

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
                    .table(ImportRun::Table)
                    .if_not_exists()
                    .col(pk_auto(ImportRun::Id))
                    // カタログ取込はプロジェクトに属さない（18.1）
                    .col(integer_null(ImportRun::ProjectId))
                    .col(string(ImportRun::Kind))
                    // 同じファイルを二度流したかを後から判別できるようにする
                    .col(string(ImportRun::FileHash))
                    // マニフェストで宣言された基準時刻。履歴行の from_date に使う（23.1）
                    .col(timestamp_with_time_zone(ImportRun::AsOf))
                    .col(integer(ImportRun::CreatedCount).default(0))
                    .col(integer(ImportRun::UpdatedCount).default(0))
                    .col(integer(ImportRun::WarningCount).default(0))
                    .col(integer(ImportRun::ImportedBy))
                    .col(timestamp_with_time_zone(ImportRun::ImportedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ImportRun::Table, ImportRun::ProjectId)
                            .to(Project::Table, Project::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ImportRun::Table, ImportRun::ImportedBy)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 「このプロジェクトへの取込履歴」を新しい順に引く
        manager
            .create_index(
                Index::create()
                    .name("idx_import_run_project_imported_at")
                    .table(ImportRun::Table)
                    .col(ImportRun::ProjectId)
                    .col(ImportRun::ImportedAt)
                    .to_owned(),
            )
            .await?;
        // 取込者による絞り込み。FK列には自動で索引が張られない（24.3）
        manager
            .create_index(
                Index::create()
                    .name("idx_import_run_imported_by")
                    .table(ImportRun::Table)
                    .col(ImportRun::ImportedBy)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ImportRun::Table).to_owned())
            .await
    }
}

/// 既存のマイグレーションで作成済み。外部キーの参照先としてのみ使う。
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
enum ImportRun {
    Table,
    Id,
    ProjectId,
    Kind,
    FileHash,
    AsOf,
    CreatedCount,
    UpdatedCount,
    WarningCount,
    ImportedBy,
    ImportedAt,
}
