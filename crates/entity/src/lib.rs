//! sea-ormのエンティティ定義。
//!
//! 型の対応は設計書24.2に従う。特に注意すべき点は以下。
//!
//! - **UUIDは文字列で持つ。** PostgreSQLの `uuid` 型を使うとSQLiteと分岐するため。
//!   結合のホットパスに乗らない（主キーは整数の代理キー）ので影響は小さい
//! - **日時は常にUTC。** `DateTimeUtc` を用い、表示時にのみローカル変換する
//! - **JSONは文字列で持つ。** JSON内部を検索しない方針のため `jsonb` の利点がない
//! - **金額は最小通貨単位の整数（i64）で持つ。** SQLiteに `DECIMAL` が無く、
//!   `REAL` では丸め誤差が出るため。本モジュールの対象テーブルには該当列はまだ無い

pub mod app_user;
pub mod audit_log;
pub mod login_attempt;
pub mod project;
pub mod project_member;
pub mod session;

pub use app_user::Entity as AppUser;
pub use audit_log::Entity as AuditLog;
pub use login_attempt::Entity as LoginAttempt;
pub use project::Entity as Project;
pub use project_member::Entity as ProjectMember;
pub use session::Entity as Session;
