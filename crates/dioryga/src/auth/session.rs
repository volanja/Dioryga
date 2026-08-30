//! セッション（設計書20.5）。
//!
//! # トークンの扱い
//!
//! Cookieに入れるのはCSPRNGで生成した256bitの乱数であり、**DBにはそのSHA-256を
//! 保存する。**DBが漏洩してもセッションを乗っ取れないようにするため。
//!
//! パスワードと異なりトークンは高エントロピーな乱数であり総当たりが成立しないので、
//! ここでArgon2idを使う必要はない。毎リクエストで検証するため速度が重要であり、
//! SHA-256が適切である。
//!
//! # 監査ログの対象外
//!
//! セッションは `AUDIT_LOG` に記録しない。`last_seen_at` は毎リクエスト更新される
//! ため、監査対象にすると**1リクエストにつき監査ログが1行**増える。認証イベント
//! 自体は `LOGIN_ATTEMPT` が記録しており、二重でもある。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use entity::session;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use sha2::{Digest, Sha256};

use crate::config::SessionConfig;

/// Cookieに入れる名前。
pub const COOKIE_NAME: &str = "dioryga_session";

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("乱数の生成に失敗しました: {0}")]
    Random(String),

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

/// 利用者へ渡す生のトークン。**DBには保存しない。**
#[derive(Debug, Clone)]
pub struct RawToken(String);

impl RawToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 新しいトークンを生成する。256bitの乱数。
    pub fn generate() -> Result<Self, SessionError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| SessionError::Random(e.to_string()))?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// DBに保存する形。
    pub fn hash(&self) -> String {
        hash_token(&self.0)
    }
}

fn hash_token(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// セッションを作成する。
///
/// ログイン成功時には**必ず新しいセッションを発行する**（セッション固定攻撃の
/// 防止。設計書20.5）。呼び出し側は、ログイン前のセッションがあれば破棄すること。
pub async fn create<C: ConnectionTrait>(
    db: &C,
    user_id: i32,
    ip_address: &str,
    user_agent: &str,
    config: &SessionConfig,
    now: DateTime<Utc>,
) -> Result<(session::Model, RawToken), SessionError> {
    let token = RawToken::generate()?;

    let model = session::ActiveModel {
        user_id: Set(user_id),
        token_hash: Set(token.hash()),
        created_at: Set(now),
        last_seen_at: Set(now),
        expires_at: Set(now + Duration::seconds(config.absolute_timeout_secs)),
        revoked_at: Set(None),
        ip_address: Set(ip_address.to_owned()),
        user_agent: Set(user_agent.to_owned()),
        ..Default::default()
    }
    .insert(db)
    .await?;

    Ok((model, token))
}

/// セッションが有効かを判定する。
///
/// 失効・絶対期限切れ・アイドル期限切れのいずれでも `None` を返す。
pub async fn validate<C: ConnectionTrait>(
    db: &C,
    raw: &str,
    config: &SessionConfig,
    now: DateTime<Utc>,
) -> Result<Option<session::Model>, SessionError> {
    let Some(found) = session::Entity::find()
        .filter(session::Column::TokenHash.eq(hash_token(raw)))
        .one(db)
        .await?
    else {
        return Ok(None);
    };

    if found.revoked_at.is_some() {
        return Ok(None);
    }
    if now >= found.expires_at {
        return Ok(None);
    }
    if now - found.last_seen_at >= Duration::seconds(config.idle_timeout_secs) {
        return Ok(None);
    }

    Ok(Some(found))
}

/// 最終利用時刻を更新する。アイドルタイムアウトの起点になる。
pub async fn touch<C: ConnectionTrait>(
    db: &C,
    model: session::Model,
    now: DateTime<Utc>,
) -> Result<session::Model, SessionError> {
    let mut active: session::ActiveModel = model.into();
    active.last_seen_at = Set(now);
    Ok(active.update(db).await?)
}

/// 1つのセッションを失効させる（ログアウト）。
pub async fn revoke<C: ConnectionTrait>(
    db: &C,
    model: session::Model,
    now: DateTime<Utc>,
) -> Result<(), SessionError> {
    let mut active: session::ActiveModel = model.into();
    active.revoked_at = Set(Some(now));
    active.update(db).await?;
    Ok(())
}

/// 指定したセッション以外をすべて失効させる。
///
/// パスワード変更時に使う。**現在使用中のセッションだけを残す**ことで、
/// 変更した本人はログインしたまま、他の端末やセッション乗っ取り分を切る
/// （設計書20.7）。
pub async fn revoke_all_except<C: ConnectionTrait>(
    db: &C,
    user_id: i32,
    keep_session_id: Option<i32>,
    now: DateTime<Utc>,
) -> Result<u64, SessionError> {
    let mut query = session::Entity::find()
        .filter(session::Column::UserId.eq(user_id))
        .filter(session::Column::RevokedAt.is_null());

    if let Some(keep) = keep_session_id {
        query = query.filter(session::Column::Id.ne(keep));
    }

    let targets = query.all(db).await?;
    let count = targets.len() as u64;

    for target in targets {
        revoke(db, target, now).await?;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 生成されるトークンは毎回異なる() {
        let a = RawToken::generate().unwrap();
        let b = RawToken::generate().unwrap();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn トークンは十分な長さを持つ() {
        let token = RawToken::generate().unwrap();
        // 256bit を base64url（パディング無し）にすると43文字
        assert_eq!(token.as_str().len(), 43);
    }

    #[test]
    fn 保存用のハッシュから生のトークンは分からない() {
        let token = RawToken::generate().unwrap();
        let hashed = token.hash();

        assert_ne!(hashed, token.as_str());
        assert!(!hashed.contains(token.as_str()));
        // 同じ入力からは同じハッシュが得られる（検索できる必要がある）
        assert_eq!(hashed, token.hash());
    }
}
