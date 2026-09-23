//! 変更管理のテーブルを作成する（設計書11章）。
//!
//! # `work_order_id` に外部キーは張らない
//!
//! 機器・配置・ネットワークの各履歴テーブルに `work_order_id` 列が既にあり、
//! それらのマイグレーションには「`WORK_ORDER` を作る際に外部キーを追加すること」
//! と書いてあった。**その約束は果たせないと分かったので撤回する**（24.3）。
//!
//! **SQLiteの `ALTER TABLE` は制約の追加に対応していない**（`ADD COLUMN`・
//! `DROP COLUMN`・`RENAME` のみ）。追加するには11テーブルを作り直して全行を
//! 写す必要があり、得られる保証に見合わない。PostgreSQL側にだけ張る案は、
//! 24.2の「スキーマがDBごとに分岐しない」に反するため採らない。
//!
//! したがって `work_order_id` は `item_type`/`item_id` と同じ多態的参照の扱い
//! とし、**参照整合はアプリケーション層と `dioryga check`（24.5）で担保する。**
//!
//! これは「後から制約を足せる」という前提で設計を進めると詰む、という教訓でも
//! ある。SQLiteを対象に含める以上、制約はテーブル作成時に決めきる必要がある。
//!
//! # 承認は行で持つ
//!
//! `WORK_ORDER.status` に `approved` があるが、**それは導出結果を書き戻した
//! ものではない。**承認そのものは `WORK_ORDER_APPROVAL` の行が真実の源であり、
//! 紐づく**全**行が `approved` になった時点で `WORK_ORDER` を遷移させる（11章）。
//!
//! 影響を受けるプロジェクトごとに1行を起こすため、他プロジェクトの機器を巻き
//!込む変更（移設・移譲）では承認が複数必要になる。`required_project_id` は
//! 「どのプロジェクトの承認か」を指す。
//!
//! # 自己承認はDB制約にしない
//!
//! 承認者が起票側の担当者本人であってはならない（旧C-9）が、比較対象が
//! **別テーブルの列**（`WORK_ORDER.primary_assignee_id`）になるため
//! `CHECK` 制約で書けない。登録時にアプリケーション層で確かめる。
//!
//! なお**承認者が自分しかいない場合は自己承認を許可し、監査ログに明示的に
//! 記録する**（22章）。一人で運用する現場で詰まないようにするため。
//!
//! # 予定と実績を別の列で持つ
//!
//! `due_date`（期日）と `planned_at`/`executed_at`/`completed_at` を分けて
//! 持つ。片方に上書きすると「当初いつの予定だったか」が失われ、QCDの「D」を
//! 定量的に見られなくなる（5.1）。`MILESTONE` と同じ判断である。

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
                    .table(WorkOrder::Table)
                    .if_not_exists()
                    .col(pk_auto(WorkOrder::Id))
                    // 起票元。承認の起点であり、一覧の既定の絞り込み軸でもある
                    .col(integer(WorkOrder::ProjectId))
                    // work_type=Transfer の移譲先。それ以外では NULL
                    .col(integer_null(WorkOrder::TargetProjectId))
                    .col(integer_null(WorkOrder::DeviceId))
                    .col(integer_null(WorkOrder::PartInstanceId))
                    // Repair / Addition / Relocation / Disposal / Transfer
                    .col(string(WorkOrder::WorkType))
                    .col(string(WorkOrder::Title))
                    .col(text(WorkOrder::Description))
                    // 主・副の2名体制。副は任意（11章）
                    .col(integer_null(WorkOrder::PrimaryAssigneeId))
                    .col(integer_null(WorkOrder::SecondaryAssigneeId))
                    .col(date_null(WorkOrder::DueDate))
                    // planned / approved / in_progress / completed / cancelled
                    // （当初は executing / aborted。#174 で改名した）
                    .col(string(WorkOrder::Status))
                    .col(timestamp_with_time_zone_null(WorkOrder::PlannedAt))
                    .col(timestamp_with_time_zone_null(WorkOrder::ExecutedAt))
                    .col(timestamp_with_time_zone_null(WorkOrder::CompletedAt))
                    .col(timestamp_with_time_zone_null(WorkOrder::AbortedAt))
                    .col(text_null(WorkOrder::AbortedReason))
                    .col(timestamp_with_time_zone(WorkOrder::CreatedAt))
                    .col(timestamp_with_time_zone(WorkOrder::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrder::Table, WorkOrder::ProjectId)
                            .to(Project::Table, Project::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrder::Table, WorkOrder::TargetProjectId)
                            .to(Project::Table, Project::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrder::Table, WorkOrder::DeviceId)
                            .to(Device::Table, Device::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrder::Table, WorkOrder::PartInstanceId)
                            .to(PartInstance::Table, PartInstance::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrder::Table, WorkOrder::PrimaryAssigneeId)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrder::Table, WorkOrder::SecondaryAssigneeId)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_work_order_project",
            WorkOrder::Table,
            WorkOrder::ProjectId,
        )
        .await?;
        index(
            manager,
            "idx_work_order_target_project",
            WorkOrder::Table,
            WorkOrder::TargetProjectId,
        )
        .await?;
        index(
            manager,
            "idx_work_order_device",
            WorkOrder::Table,
            WorkOrder::DeviceId,
        )
        .await?;
        index(
            manager,
            "idx_work_order_part_instance",
            WorkOrder::Table,
            WorkOrder::PartInstanceId,
        )
        .await?;
        index(
            manager,
            "idx_work_order_primary_assignee",
            WorkOrder::Table,
            WorkOrder::PrimaryAssigneeId,
        )
        .await?;
        index(
            manager,
            "idx_work_order_secondary_assignee",
            WorkOrder::Table,
            WorkOrder::SecondaryAssigneeId,
        )
        .await?;

        // 16.7の変更管理チケット画面は「自分の担当」「期限間近」で引く。
        // status を先に置くのは、完了済みを除いてから期日で絞るため
        manager
            .create_index(
                Index::create()
                    .name("idx_work_order_status_due")
                    .table(WorkOrder::Table)
                    .col(WorkOrder::Status)
                    .col(WorkOrder::DueDate)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(WorkOrderApproval::Table)
                    .if_not_exists()
                    .col(pk_auto(WorkOrderApproval::Id))
                    .col(integer(WorkOrderApproval::WorkOrderId))
                    // どのプロジェクトの承認か。影響先ごとに1行を起こす
                    .col(integer(WorkOrderApproval::RequiredProjectId))
                    // 承認前は NULL。誰が承認したかは押した時点で決まる
                    .col(integer_null(WorkOrderApproval::ApproverId))
                    // pending / approved / rejected
                    .col(string(WorkOrderApproval::Status))
                    .col(timestamp_with_time_zone_null(WorkOrderApproval::ApprovedAt))
                    .col(timestamp_with_time_zone(WorkOrderApproval::CreatedAt))
                    .col(timestamp_with_time_zone(WorkOrderApproval::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrderApproval::Table, WorkOrderApproval::WorkOrderId)
                            .to(WorkOrder::Table, WorkOrder::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                WorkOrderApproval::Table,
                                WorkOrderApproval::RequiredProjectId,
                            )
                            .to(Project::Table, Project::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkOrderApproval::Table, WorkOrderApproval::ApproverId)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        index(
            manager,
            "idx_work_order_approval_work_order",
            WorkOrderApproval::Table,
            WorkOrderApproval::WorkOrderId,
        )
        .await?;
        index(
            manager,
            "idx_work_order_approval_approver",
            WorkOrderApproval::Table,
            WorkOrderApproval::ApproverId,
        )
        .await?;

        // 「自分のプロジェクト宛で未処理の承認」を引く。承認待ちの通知に使う
        manager
            .create_index(
                Index::create()
                    .name("idx_work_order_approval_required_project_status")
                    .table(WorkOrderApproval::Table)
                    .col(WorkOrderApproval::RequiredProjectId)
                    .col(WorkOrderApproval::Status)
                    .to_owned(),
            )
            .await?;

        // 同じプロジェクトの承認行を二重に起こさない。
        // 二重にあると「全行が approved」の判定が承認の回数に依存してしまう
        manager
            .create_index(
                Index::create()
                    .name("uq_work_order_approval_project")
                    .table(WorkOrderApproval::Table)
                    .col(WorkOrderApproval::WorkOrderId)
                    .col(WorkOrderApproval::RequiredProjectId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            WorkOrderApproval::Table.into_iden(),
            WorkOrder::Table.into_iden(),
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
enum Device {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PartInstance {
    Table,
    Id,
}

// --- 本マイグレーションで作成する ---

/// 列名をそのまま識別子にするため接頭辞が重なる。改名するとDBの列名が変わる。
#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum WorkOrder {
    Table,
    Id,
    ProjectId,
    TargetProjectId,
    DeviceId,
    PartInstanceId,
    WorkType,
    Title,
    Description,
    PrimaryAssigneeId,
    SecondaryAssigneeId,
    DueDate,
    Status,
    PlannedAt,
    ExecutedAt,
    CompletedAt,
    AbortedAt,
    AbortedReason,
    CreatedAt,
    UpdatedAt,
}

#[allow(clippy::enum_variant_names)]
#[derive(DeriveIden)]
enum WorkOrderApproval {
    Table,
    Id,
    WorkOrderId,
    RequiredProjectId,
    ApproverId,
    Status,
    ApprovedAt,
    CreatedAt,
    UpdatedAt,
}
