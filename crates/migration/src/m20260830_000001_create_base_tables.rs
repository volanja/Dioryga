//! 認証（P3）が必要とする基盤テーブルを作成する。
//!
//! 型の選び方は設計書24.2に従う。
//! - UUID は両DBとも文字列（PostgreSQLの `uuid` 型を使うとSQLiteと分岐するため）
//! - 日時は常にUTC。`timestamp_with_time_zone` はSQLiteではTEXTになる
//! - 履歴テーブルには `WHERE to_date IS NULL` の部分インデックスを張る（24.3）
//!   ただし本マイグレーションの対象に履歴テーブルは含まれない

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
                    .table(AppUser::Table)
                    .if_not_exists()
                    .col(pk_auto(AppUser::Id))
                    .col(string(AppUser::Name))
                    // ログインID。小文字にそろえて保存する（設計書20.1）
                    .col(string(AppUser::Username).unique_key())
                    // 任意。ログインIDではないが、値がある場合は一意（20.1）。
                    // 一意制約は両DBともNULLどうしを重複とみなさない
                    .col(string_null(AppUser::Email).unique_key())
                    .col(string(AppUser::PasswordHash))
                    .col(boolean(AppUser::MustChangePassword).default(false))
                    .col(boolean(AppUser::IsSystemAdmin).default(false))
                    .col(string(AppUser::Locale).default("ja"))
                    .col(timestamp_with_time_zone_null(AppUser::LastLoginAt))
                    // 物理削除はしない。無効化は disabled_at で表す（設計書20.11）
                    .col(timestamp_with_time_zone_null(AppUser::DisabledAt))
                    .col(timestamp_with_time_zone(AppUser::CreatedAt))
                    .col(timestamp_with_time_zone(AppUser::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Project::Table)
                    .if_not_exists()
                    .col(pk_auto(Project::Id))
                    // 取込時の突合に使う不変の識別子（設計書5.3、23.2）
                    .col(string(Project::Uid).unique_key())
                    .col(string_null(Project::Code).unique_key())
                    // name に一意制約は張らない（年度違いで同名の案件がありうる）
                    .col(string(Project::Name))
                    .col(string(Project::Description).default(""))
                    .col(string(Project::Currency).default("JPY"))
                    // サービス上の状態ではなく、一覧から外すという運用判断（5.2）
                    .col(timestamp_with_time_zone_null(Project::ArchivedAt))
                    .col(string_null(Project::ClosureReason))
                    .col(timestamp_with_time_zone(Project::CreatedAt))
                    .col(timestamp_with_time_zone(Project::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ProjectMember::Table)
                    .if_not_exists()
                    .col(pk_auto(ProjectMember::Id))
                    .col(integer(ProjectMember::UserId))
                    .col(integer(ProjectMember::ProjectId))
                    .col(string(ProjectMember::Role))
                    .col(string_null(ProjectMember::AdminRank))
                    .col(timestamp_with_time_zone(ProjectMember::CreatedAt))
                    .col(timestamp_with_time_zone(ProjectMember::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ProjectMember::Table, ProjectMember::UserId)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ProjectMember::Table, ProjectMember::ProjectId)
                            .to(Project::Table, Project::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 複数ロールの兼務を認めるため、一意制約は (user, project) ではなく
        // (user, project, role) に張る（設計書5章、旧C-8）
        manager
            .create_index(
                Index::create()
                    .name("uq_project_member_user_project_role")
                    .table(ProjectMember::Table)
                    .col(ProjectMember::UserId)
                    .col(ProjectMember::ProjectId)
                    .col(ProjectMember::Role)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Session::Table)
                    .if_not_exists()
                    .col(pk_auto(Session::Id))
                    .col(integer(Session::UserId))
                    // 生のトークンは保存しない（設計書20.5）
                    .col(string(Session::TokenHash).unique_key())
                    .col(timestamp_with_time_zone(Session::CreatedAt))
                    .col(timestamp_with_time_zone(Session::LastSeenAt))
                    .col(timestamp_with_time_zone(Session::ExpiresAt))
                    .col(timestamp_with_time_zone_null(Session::RevokedAt))
                    .col(string(Session::IpAddress).default(""))
                    .col(string(Session::UserAgent).default(""))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Session::Table, Session::UserId)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // 両DBともFKカラムに自動でインデックスを張らないため明示する（設計書24.3）
        manager
            .create_index(
                Index::create()
                    .name("idx_session_user_id")
                    .table(Session::Table)
                    .col(Session::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(LoginAttempt::Table)
                    .if_not_exists()
                    .col(pk_auto(LoginAttempt::Id))
                    // 存在しないユーザーへの試行も記録するためFKにしない（設計書20.4）
                    .col(string(LoginAttempt::Username))
                    .col(string(LoginAttempt::IpAddress))
                    .col(boolean(LoginAttempt::Succeeded))
                    .col(timestamp_with_time_zone(LoginAttempt::AttemptedAt))
                    .to_owned(),
            )
            .await?;

        // レート制限はアカウント（ユーザー名）単位とIP単位を別々に数える（設計書20.6）
        manager
            .create_index(
                Index::create()
                    .name("idx_login_attempt_username_attempted_at")
                    .table(LoginAttempt::Table)
                    .col(LoginAttempt::Username)
                    .col(LoginAttempt::AttemptedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_login_attempt_ip_attempted_at")
                    .table(LoginAttempt::Table)
                    .col(LoginAttempt::IpAddress)
                    .col(LoginAttempt::AttemptedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .if_not_exists()
                    .col(pk_auto(AuditLog::Id))
                    .col(integer(AuditLog::UserId))
                    .col(string(AuditLog::TableName))
                    .col(integer(AuditLog::RecordId))
                    .col(string(AuditLog::Action))
                    // JSONは両DBともTEXT（設計書24.2.3）
                    .col(text_null(AuditLog::BeforeJson))
                    .col(text_null(AuditLog::AfterJson))
                    .col(integer_null(AuditLog::ImportRunId))
                    .col(timestamp_with_time_zone(AuditLog::ChangedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(AuditLog::Table, AuditLog::UserId)
                            .to(AppUser::Table, AppUser::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_audit_log_table_record")
                    .table(AuditLog::Table)
                    .col(AuditLog::TableName)
                    .col(AuditLog::RecordId)
                    .to_owned(),
            )
            .await?;
        // 保持期間による削除に使う（設計書24.3）
        manager
            .create_index(
                Index::create()
                    .name("idx_audit_log_changed_at")
                    .table(AuditLog::Table)
                    .col(AuditLog::ChangedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 外部キーの依存があるため作成と逆順に落とす
        manager
            .drop_table(Table::drop().table(AuditLog::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(LoginAttempt::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Session::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ProjectMember::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Project::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AppUser::Table).to_owned())
            .await?;
        Ok(())
    }
}

/// 設計書の `USER`。`user` はPostgreSQLの予約語のため物理名を `app_user` とする。
#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
    Name,
    Username,
    Email,
    PasswordHash,
    MustChangePassword,
    IsSystemAdmin,
    Locale,
    LastLoginAt,
    DisabledAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Project {
    Table,
    Id,
    Uid,
    Code,
    Name,
    Description,
    Currency,
    ArchivedAt,
    ClosureReason,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ProjectMember {
    Table,
    Id,
    UserId,
    ProjectId,
    Role,
    AdminRank,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Session {
    Table,
    Id,
    UserId,
    TokenHash,
    CreatedAt,
    LastSeenAt,
    ExpiresAt,
    RevokedAt,
    IpAddress,
    UserAgent,
}

#[derive(DeriveIden)]
enum LoginAttempt {
    Table,
    Id,
    Username,
    IpAddress,
    Succeeded,
    AttemptedAt,
}

#[derive(DeriveIden)]
enum AuditLog {
    Table,
    Id,
    UserId,
    TableName,
    RecordId,
    Action,
    BeforeJson,
    AfterJson,
    ImportRunId,
    ChangedAt,
}
