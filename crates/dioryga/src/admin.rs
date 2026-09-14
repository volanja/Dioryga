//! `dioryga admin` サブコマンドの実装（設計書20.8）。
//!
//! セットアップ画面が使えない場合の**復旧経路**。DBへアクセスできる者だけが
//! 実行できる、という点を権限の根拠としている。
//!
//! パスワードはコマンドライン引数では受け取らない。引数に書くとシェルの履歴や
//! プロセス一覧に平文が残るため、**対話的なプロンプトか、標準入力**で受け取る。
//!
//! 標準入力（`--password-stdin`）は、構築手順や動作確認用データの投入を
//! スクリプトにするための経路である（#129）。

use std::io::BufRead;

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
///
/// `name` を省略すると表示名を対話で聞く。`password_stdin` なら標準入力の
/// 1行目をパスワードとし、そうでなければ対話で2回聞く。
pub async fn create(
    config: &Config,
    email: &str,
    name: Option<&str>,
    password_stdin: bool,
) -> anyhow::Result<()> {
    let conn = db::connect(&config.database).await?;
    let passwords = PasswordService::new(config.password.clone())?;

    // **入力を求める前に確かめる。**パスワードまで入れさせてから断らない
    if 既に存在する(&conn, email).await? {
        anyhow::bail!("このメールアドレスの利用者は既に存在します: {email}");
    }

    let name = match name {
        Some(name) => name.trim().to_owned(),
        None => prompt("表示名: ")?,
    };
    let password = if password_stdin {
        標準入力のパスワード(std::io::stdin().lock())?
    } else {
        prompt_password_twice()?
    };

    let user = 作成する(&conn, &passwords, email, &name, &password).await?;

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

/// System Adminを作成する本体。**対話でも標準入力でも、ここを通る。**
///
/// 入力の受け取り方によって検査が変わらないようにする（重複・表示名・
/// パスワードポリシー）。
pub async fn 作成する(
    db: &DatabaseConnection,
    passwords: &PasswordService,
    email: &str,
    name: &str,
    password: &str,
) -> anyhow::Result<app_user::Model> {
    if 既に存在する(db, email).await? {
        anyhow::bail!("このメールアドレスの利用者は既に存在します: {email}");
    }
    if name.trim().is_empty() {
        anyhow::bail!("表示名が空です");
    }
    passwords.check_policy(password, email, name)?;

    Ok(setup::create_system_admin(db, passwords, name, email, password, false).await?)
}

/// 標準入力の1行目をパスワードとして読む。
///
/// **末尾の改行だけを取り除く。**前後の空白はパスワードの一部でありうるので
/// `trim` しない。`printf '%s\n'` でも `echo` でも、Windowsの CRLF でも同じ値になる。
pub fn 標準入力のパスワード(mut reader: impl BufRead) -> anyhow::Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let password = line.strip_suffix('\n').unwrap_or(&line);
    let password = password.strip_suffix('\r').unwrap_or(password);
    if password.is_empty() {
        anyhow::bail!("標準入力からパスワードを読めませんでした（空です）");
    }
    Ok(password.to_owned())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn 読む(input: &str) -> anyhow::Result<String> {
        標準入力のパスワード(input.as_bytes())
    }

    #[test]
    fn 末尾の改行を取り除く() {
        assert_eq!(
            読む("Tanigawa-Bridge-7391\n").unwrap(),
            "Tanigawa-Bridge-7391"
        );
    }

    #[test]
    fn crlfでも同じ値になる() {
        assert_eq!(
            読む("Tanigawa-Bridge-7391\r\n").unwrap(),
            "Tanigawa-Bridge-7391"
        );
    }

    #[test]
    fn 改行が無くても読める() {
        assert_eq!(
            読む("Tanigawa-Bridge-7391").unwrap(),
            "Tanigawa-Bridge-7391"
        );
    }

    /// **前後の空白はパスワードの一部として残す。**`trim` すると、空白を含む
    /// パスワードで作った管理者が、画面からログインできなくなる。
    #[test]
    fn 前後の空白は残す() {
        assert_eq!(読む("  滝見 石垣  \n").unwrap(), "  滝見 石垣  ");
    }

    #[test]
    fn 読むのは1行目だけ() {
        assert_eq!(
            読む("first-line-pass\nsecond\n").unwrap(),
            "first-line-pass"
        );
    }

    #[test]
    fn 空なら拒否する() {
        assert!(読む("").is_err());
        assert!(読む("\n").is_err());
        assert!(読む("\r\n").is_err());
    }
}
