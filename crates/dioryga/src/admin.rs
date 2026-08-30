//! `dioryga admin` サブコマンドの実装（設計書20.8）。
//!
//! セットアップ画面が使えない場合の**復旧経路**。DBへアクセスできる者だけが
//! 実行できる、という点を権限の根拠としている。
//!
//! パスワードはコマンドライン引数では受け取らない。引数に書くとシェルの履歴や
//! プロセス一覧に平文が残るため、対話的なプロンプトで入力させる。

use chrono::Utc;
use entity::app_user;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::auth::password::{self, PasswordService};
use crate::auth::session;
use crate::auth::setup;
use crate::config::Config;
use crate::db;
use crate::repository::{Actor, AuditedTx};

/// System Adminを作成する。
pub async fn create(config: &Config, email: &str) -> anyhow::Result<()> {
    let conn = db::connect(&config.database).await?;
    let passwords = PasswordService::new(config.password.clone())?;

    if 既に存在する(&conn, email).await? {
        anyhow::bail!("このメールアドレスの利用者は既に存在します: {email}");
    }

    let name = prompt("表示名: ")?;
    let password = prompt_password_twice()?;

    passwords.check_policy(&password, email, &name)?;

    let user =
        setup::create_system_admin(&conn, &passwords, &name, email, &password, false).await?;

    println!(
        "System Adminを作成しました（id={}, email={}）",
        user.id, user.email
    );
    Ok(())
}

/// パスワードをリセットする。
///
/// 設計書20.7の通り、**一時パスワードを生成して一度だけ表示する。**
/// 平文はDBに保存せず、対象の利用者は次回ログイン時に変更を強制される。
/// あわせて対象利用者の既存セッションをすべて失効させる。
pub async fn reset_password(config: &Config, email: &str) -> anyhow::Result<()> {
    let conn = db::connect(&config.database).await?;
    let passwords = PasswordService::new(config.password.clone())?;

    let Some(user) = app_user::Entity::find()
        .filter(app_user::Column::Email.eq(email))
        .one(&conn)
        .await?
    else {
        anyhow::bail!("該当する利用者が見つかりません: {email}");
    };

    let temporary = password::generate_temporary()?;
    let hash = passwords.hash(&temporary).await?;
    let now = Utc::now();

    let tx = AuditedTx::begin(&conn, Actor::User(user.id)).await?;
    let mut active: app_user::ActiveModel = user.clone().into();
    active.password_hash = Set(hash);
    // 次回ログイン時に変更を強制する（設計書20.6）
    active.must_change_password = Set(true);
    active.updated_at = Set(now);
    tx.update(&user, active).await?;
    tx.commit().await?;

    // 既存セッションをすべて失効させる（設計書20.7）
    let 失効数 = session::revoke_all_except(&conn, user.id, None, now).await?;

    println!();
    println!("  {email} の一時パスワードを発行しました。");
    println!();
    println!("      {temporary}");
    println!();
    println!("  この値はここにしか表示されません。口頭等の別経路で本人へ伝えてください。");
    println!("  本人は次回ログイン時にパスワードの変更を求められます。");
    if 失効数 > 0 {
        println!("  既存のセッション{失効数}件を失効させました。");
    }
    println!();

    Ok(())
}

async fn 既に存在する(db: &DatabaseConnection, email: &str) -> Result<bool, sea_orm::DbErr> {
    Ok(app_user::Entity::find()
        .filter(app_user::Column::Email.eq(email))
        .one(db)
        .await?
        .is_some())
}

fn prompt(label: &str) -> anyhow::Result<String> {
    use std::io::{stdin, stdout, Write};

    print!("{label}");
    stdout().flush()?;

    let mut buf = String::new();
    stdin().read_line(&mut buf)?;
    Ok(buf.trim().to_owned())
}

/// パスワードを2回入力させ、一致を確かめる。入力は画面に表示しない。
fn prompt_password_twice() -> anyhow::Result<String> {
    let first = rpassword::prompt_password("パスワード: ")?;
    let second = rpassword::prompt_password("パスワード（確認）: ")?;

    if first != second {
        anyhow::bail!("パスワードが一致しません");
    }
    Ok(first)
}

// 一時パスワードの生成そのものは `auth::password` 側で検証している。
