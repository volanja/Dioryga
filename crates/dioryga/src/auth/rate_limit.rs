//! ログイン試行のレート制限（設計書20.6）。
//!
//! # アカウント単位とIP単位を別々に数える
//!
//! **アカウント（ユーザー名）単位を主、IP単位を補助とする。**IPを変えながら
//! 1つのアカウントを狙う攻撃はアカウント単位で、1つのIPから多数のアカウントを
//! 試す攻撃はIP単位で拾う。「ユーザー名＋IP」の組では数えない——IPを変えれば
//! 組が変わり、分散した攻撃を止められない。
//!
//! # アカウント単位は、待ち時間を延ばす
//!
//! **受付を止めない。**待ち時間はアカウントに掛かるため、止めると他人が失敗を
//! 重ねるだけで本人も締め出される（DoS）。3回までは待ちなし、以降は失敗の
//! たびに待ち時間を倍にし、[`MAX_DELAY_SECS`] で頭打ちにする。ログインに
//! 成功したら数え直す。
//!
//! # 待ち時間はサーバで眠らせない
//!
//! 「次に受け付ける時刻」までの試行を断り、あと何秒待てばよいかを返す。
//! 眠らせると、攻撃のリクエストがサーバの処理を占有する。
//!
//! # 恒久ロックにしない
//!
//! 攻撃者が任意のユーザー名を指定して、意図的にログイン不能にできるため。

use chrono::{DateTime, Duration, Utc};
use entity::login_attempt;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

/// 待ちなしで失敗できる回数。打ち間違いで待たされないための余裕。
pub const FREE_FAILURES: u64 = 3;

/// アカウント単位の待ち時間の上限。
pub const MAX_DELAY_SECS: i64 = 60;

/// IP単位で受付を止めるまでの失敗回数。
///
/// **アカウント単位よりずっと大きくする。**社内では多数の利用者が同じ出口IP
/// （NAT）を共有しており、小さいと1人の打ち間違いで事務所全体が止まる。
pub const IP_MAX_FAILURES: u64 = 50;

/// IP単位で失敗を数える期間。
pub const IP_WINDOW_SECS: i64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 受け付ける。
    Allow,
    /// 一時的に受け付けない。あと何秒待てばよいか。
    Throttled { retry_after_secs: i64 },
}

/// 試行を記録する。成功・失敗の双方を残す。
///
/// `username` は**正規化した形**（[`crate::auth::username::正規化する`]）で渡す。
/// 大文字小文字の違いで別のアカウントとして数えないため。
pub async fn record<C: ConnectionTrait>(
    db: &C,
    username: &str,
    ip_address: &str,
    succeeded: bool,
    now: DateTime<Utc>,
) -> Result<(), sea_orm::DbErr> {
    login_attempt::ActiveModel {
        username: Set(username.to_owned()),
        ip_address: Set(ip_address.to_owned()),
        succeeded: Set(succeeded),
        attempted_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

/// 連続失敗の回数に対する待ち時間（秒）。
///
/// 3回までは0。4回目から 1, 2, 4, 8, 16, 32 秒と倍にし、60秒で頭打ちにする。
pub fn 待ち時間(連続失敗: u64) -> i64 {
    if 連続失敗 <= FREE_FAILURES {
        return 0;
    }
    let 指数 = 連続失敗 - FREE_FAILURES - 1;
    // 2^6 = 64 で上限を超える。それ以上はシフトさせない
    if 指数 >= 6 {
        return MAX_DELAY_SECS;
    }
    (1i64 << 指数).min(MAX_DELAY_SECS)
}

/// 試行を受け付けてよいかを判定する。
pub async fn check<C: ConnectionTrait>(
    db: &C,
    username: &str,
    ip_address: &str,
    now: DateTime<Utc>,
) -> Result<Decision, sea_orm::DbErr> {
    if let Some(decision) = アカウント単位(db, username, now).await? {
        return Ok(decision);
    }
    if let Some(decision) = ip単位(db, ip_address, now).await? {
        return Ok(decision);
    }
    Ok(Decision::Allow)
}

/// **最後に成功してからの連続失敗**で待ち時間を決める。
async fn アカウント単位<C: ConnectionTrait>(
    db: &C,
    username: &str,
    now: DateTime<Utc>,
) -> Result<Option<Decision>, sea_orm::DbErr> {
    let 最後の成功 = login_attempt::Entity::find()
        .filter(login_attempt::Column::Username.eq(username))
        .filter(login_attempt::Column::Succeeded.eq(true))
        .order_by_desc(login_attempt::Column::AttemptedAt)
        .one(db)
        .await?
        .map(|a| a.attempted_at);

    let mut 失敗 = login_attempt::Entity::find()
        .filter(login_attempt::Column::Username.eq(username))
        .filter(login_attempt::Column::Succeeded.eq(false));
    if let Some(at) = 最後の成功 {
        失敗 = 失敗.filter(login_attempt::Column::AttemptedAt.gt(at));
    }

    let 待ち = 待ち時間(失敗.clone().count(db).await?);
    if 待ち == 0 {
        return Ok(None);
    }

    let Some(最後の失敗) = 失敗
        .order_by_desc(login_attempt::Column::AttemptedAt)
        .one(db)
        .await?
    else {
        return Ok(None);
    };

    let 解除 = 最後の失敗.attempted_at + Duration::seconds(待ち);
    Ok((now < 解除).then(|| Decision::Throttled {
        retry_after_secs: 切り上げ秒(解除 - now),
    }))
}

/// 直近 [`IP_WINDOW_SECS`] の失敗が [`IP_MAX_FAILURES`] に達したら止める。
async fn ip単位<C: ConnectionTrait>(
    db: &C,
    ip_address: &str,
    now: DateTime<Utc>,
) -> Result<Option<Decision>, sea_orm::DbErr> {
    let since = now - Duration::seconds(IP_WINDOW_SECS);
    let 失敗 = login_attempt::Entity::find()
        .filter(login_attempt::Column::IpAddress.eq(ip_address))
        .filter(login_attempt::Column::Succeeded.eq(false))
        .filter(login_attempt::Column::AttemptedAt.gte(since));

    if 失敗.clone().count(db).await? < IP_MAX_FAILURES {
        return Ok(None);
    }

    // 最も古い失敗が期間の外に出れば、回数が上限を下回る
    let 最古 = 失敗
        .order_by_asc(login_attempt::Column::AttemptedAt)
        .one(db)
        .await?;
    let retry_after_secs = match 最古 {
        Some(a) => 切り上げ秒(a.attempted_at + Duration::seconds(IP_WINDOW_SECS) - now),
        None => IP_WINDOW_SECS,
    };
    Ok(Some(Decision::Throttled { retry_after_secs }))
}

/// 残り時間を秒に切り上げる。**0秒とは言わない**（言うと即座に再送されて断られる）。
fn 切り上げ秒(残り: Duration) -> i64 {
    let ms = 残り.num_milliseconds();
    ((ms + 999) / 1000).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 三回までは待ちなし() {
        for n in 0..=3 {
            assert_eq!(待ち時間(n), 0, "{n}回目");
        }
    }

    #[test]
    fn 四回目から倍になり六十秒で頭打ち() {
        assert_eq!(
            (4..=11).map(待ち時間).collect::<Vec<_>>(),
            vec![1, 2, 4, 8, 16, 32, 60, 60]
        );
        assert_eq!(待ち時間(u64::MAX), 60);
    }

    #[test]
    fn 残り時間は切り上げて一秒以上にする() {
        assert_eq!(切り上げ秒(Duration::milliseconds(1)), 1);
        assert_eq!(切り上げ秒(Duration::milliseconds(1001)), 2);
        assert_eq!(切り上げ秒(Duration::zero()), 1);
    }
}
