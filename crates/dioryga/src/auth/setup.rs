//! 初回セットアップ（設計書20.8）。
//!
//! # 解いている問題
//!
//! まっさらなDBには誰もユーザーが存在しないが、ユーザーを登録できるのは
//! System Adminだけである、という起動時の循環（bootstrapping問題）。
//!
//! # なぜトークンを要求するか
//!
//! トークンが無ければ、**起動しただけの状態で「誰でも管理者になれる窓」**が
//! 開いてしまう。ローカル起動が主な想定とはいえ、LANに露出したポートを
//! 第三者に先取りされる可能性を消しておく。
//!
//! トークンは起動時に標準出力へ一度だけ表示する。DBには保存しない
//! （保存すると、DBを読める者が管理者を作れることになり、意味が薄れる）。

use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::Utc;
use entity::app_user;
use sea_orm::{DatabaseConnection, EntityTrait, PaginatorTrait, Set};
use tokio::sync::RwLock;

use crate::auth::password::{PasswordError, PasswordService};
use crate::repository::{Actor, AuditedTx};

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("セットアップは既に完了しています")]
    AlreadyCompleted,

    #[error("セットアップトークンが正しくありません")]
    InvalidToken,

    #[error(transparent)]
    Password(#[from] PasswordError),

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),

    #[error("乱数の生成に失敗しました: {0}")]
    Random(String),
}

/// セットアップの状態。
///
/// トークンを保持している間だけ `/setup` が開いている。完了すると `None` になり、
/// 以降は404を返す。
#[derive(Clone, Default)]
pub struct SetupState(Arc<RwLock<Option<String>>>);

impl SetupState {
    /// 起動時に状態を決める。
    ///
    /// 利用者が1人でも居ればセットアップ済みとみなし、トークンを発行しない。
    ///
    /// **生成したトークンはここでしか返さない。**以降 `SetupState` から読み出す
    /// 手段は無く、照合にのみ使える。「一度だけ表示する」という性質を、運用の
    /// 約束ではなく型で保証するため。
    pub async fn initialize(db: &DatabaseConnection) -> Result<(Self, Option<String>), SetupError> {
        let count = app_user::Entity::find().count(db).await?;
        if count > 0 {
            return Ok((Self(Arc::new(RwLock::new(None))), None));
        }

        let token = generate_token()?;
        Ok((
            Self(Arc::new(RwLock::new(Some(token.clone())))),
            Some(token),
        ))
    }

    /// セットアップ待ちか。
    pub async fn is_pending(&self) -> bool {
        self.0.read().await.is_some()
    }

    async fn verify(&self, provided: &str) -> Result<(), SetupError> {
        let guard = self.0.read().await;
        let Some(expected) = guard.as_ref() else {
            return Err(SetupError::AlreadyCompleted);
        };

        if crate::auth::constant_time_eq(expected, provided) {
            Ok(())
        } else {
            Err(SetupError::InvalidToken)
        }
    }

    async fn complete(&self) {
        *self.0.write().await = None;
    }
}

/// 標準出力へセットアップの案内を出す。**この1回しか表示しない。**
pub fn print_instructions(bind: &std::net::SocketAddr, token: &str) {
    println!();
    println!("  Diorygaの初回セットアップが必要です。");
    println!("  ブラウザで http://{bind}/setup を開き、以下のトークンを入力してください。");
    println!();
    println!("      {token}");
    println!();
    println!("  このトークンはここにしか表示されません。");
    println!();
}

fn generate_token() -> Result<String, SetupError> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|e| SetupError::Random(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// 最初のSystem Adminを作成する。
///
/// トークンの照合に成功した場合のみ作成し、成功したらトークンを破棄する。
pub async fn create_first_admin(
    db: &DatabaseConnection,
    state: &SetupState,
    passwords: &PasswordService,
    token: &str,
    name: &str,
    email: &str,
    password: &str,
) -> Result<app_user::Model, SetupError> {
    state.verify(token).await?;
    passwords.check_policy(password, email, name)?;

    let user = create_system_admin(db, passwords, name, email, password, false).await?;

    // 作成できた時点でトークンを無効化する。以降 /setup は開かない。
    state.complete().await;

    Ok(user)
}

/// System Adminを作成する。CLIの復旧経路からも使う（設計書20.8）。
///
/// 監査ログは**作成された当人を主体として**記録する。架空のシステムユーザーを
/// `app_user` に作らないため（20.8）。
pub async fn create_system_admin(
    db: &DatabaseConnection,
    passwords: &PasswordService,
    name: &str,
    email: &str,
    password: &str,
    must_change_password: bool,
) -> Result<app_user::Model, SetupError> {
    let hash = passwords.hash(password).await?;
    let now = Utc::now();

    let tx = AuditedTx::begin(db, Actor::SelfCreated).await?;
    let user = tx
        .insert(app_user::ActiveModel {
            name: Set(name.to_owned()),
            email: Set(email.to_owned()),
            password_hash: Set(hash),
            must_change_password: Set(must_change_password),
            is_system_admin: Set(true),
            locale: Set("ja".to_owned()),
            last_login_at: Set(None),
            disabled_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await?;
    tx.commit().await?;

    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn トークンは毎回異なる() {
        assert_ne!(generate_token().unwrap(), generate_token().unwrap());
    }

    /// テスト用に既知のトークンで状態を組み立てる。
    fn 既知のトークンで(token: &str) -> SetupState {
        SetupState(Arc::new(RwLock::new(Some(token.to_owned()))))
    }

    #[tokio::test]
    async fn 完了させると保留状態でなくなる() {
        let state = 既知のトークンで("token");
        assert!(state.is_pending().await);

        state.complete().await;
        assert!(!state.is_pending().await);
    }

    #[tokio::test]
    async fn 誤ったトークンは拒否される() {
        let state = 既知のトークンで("正しいトークン");

        assert!(matches!(
            state.verify("誤ったトークン").await,
            Err(SetupError::InvalidToken)
        ));
        assert!(state.verify("正しいトークン").await.is_ok());
    }

    #[tokio::test]
    async fn 完了後のトークン照合は拒否される() {
        let state = 既知のトークンで("token");
        state.complete().await;

        assert!(matches!(
            state.verify("token").await,
            Err(SetupError::AlreadyCompleted)
        ));
    }
}
