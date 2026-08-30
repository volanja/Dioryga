//! ログイン試行のレート制限（設計書20.6）。
//!
//! # 恒久ロックにしない
//!
//! 連続失敗時に**アカウントを恒久的にロックしない。**攻撃者が任意のユーザーの
//! メールアドレスを使って意図的にログイン不能にできるDoSになるため。
//! 一定時間の受付停止に留める。

use chrono::{DateTime, Duration, Utc};
use entity::login_attempt;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
};

/// 受付を停止するまでの連続失敗回数。
pub const MAX_FAILURES: u64 = 5;

/// 受付を停止する時間。
pub const LOCKOUT_SECS: i64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 受け付ける。
    Allow,
    /// 一時的に受け付けない。
    Throttled,
}

/// 試行を記録する。成功・失敗の双方を残す。
pub async fn record<C: ConnectionTrait>(
    db: &C,
    email: &str,
    ip_address: &str,
    succeeded: bool,
    now: DateTime<Utc>,
) -> Result<(), sea_orm::DbErr> {
    login_attempt::ActiveModel {
        email: Set(email.to_owned()),
        ip_address: Set(ip_address.to_owned()),
        succeeded: Set(succeeded),
        attempted_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

/// 試行を受け付けてよいかを判定する。
///
/// **メールアドレス単位とIPアドレス単位の双方**で数える（設計書20.6）。
/// 片方だけでは、IPを変えながら1つのアカウントを狙う攻撃と、
/// 1つのIPから多数のアカウントを試す攻撃のどちらかを見逃す。
pub async fn check<C: ConnectionTrait>(
    db: &C,
    email: &str,
    ip_address: &str,
    now: DateTime<Utc>,
) -> Result<Decision, sea_orm::DbErr> {
    let since = now - Duration::seconds(LOCKOUT_SECS);

    let by_email = 直近の失敗回数(db, login_attempt::Column::Email, email, since).await?;
    if by_email >= MAX_FAILURES {
        return Ok(Decision::Throttled);
    }

    let by_ip = 直近の失敗回数(db, login_attempt::Column::IpAddress, ip_address, since).await?;
    if by_ip >= MAX_FAILURES {
        return Ok(Decision::Throttled);
    }

    Ok(Decision::Allow)
}

async fn 直近の失敗回数<C: ConnectionTrait>(
    db: &C,
    column: login_attempt::Column,
    value: &str,
    since: DateTime<Utc>,
) -> Result<u64, sea_orm::DbErr> {
    login_attempt::Entity::find()
        .filter(column.eq(value))
        .filter(login_attempt::Column::Succeeded.eq(false))
        .filter(login_attempt::Column::AttemptedAt.gte(since))
        .count(db)
        .await
}
