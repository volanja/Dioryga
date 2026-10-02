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
use crate::auth::username;
use crate::config::Config;
use crate::repository::{Actor, AuditedTx};
use crate::{console, db};
use rust_i18n::t;

/// System Adminを作成する。
///
/// `name` を省略すると表示名を対話で聞く。`password_stdin` なら標準入力の
/// 1行目をパスワードとし、そうでなければ対話で2回聞く。
///
/// ログインIDはユーザー名で、メールアドレスは任意（設計書20.1、#136）。
pub async fn create(
    config: &Config,
    username: &str,
    email: Option<&str>,
    name: Option<&str>,
    password_stdin: bool,
) -> anyhow::Result<()> {
    let l = console::言語();
    let conn = db::connect(&config.database).await?;
    let passwords = PasswordService::new(config.password.clone())?;

    // **入力を求める前に確かめる。**パスワードまで入れさせてから断らない
    let username = username::検証する(username)?;
    if 使われている(&conn, app_user::Column::Username, &username).await? {
        anyhow::bail!(
            "{}",
            t!(
                "console.admin_duplicate_username",
                locale = l,
                username = username
            )
        );
    }

    let name = match name {
        Some(name) => name.trim().to_owned(),
        None => prompt(&t!("console.admin_name_prompt", locale = l))?,
    };
    let password = if password_stdin {
        標準入力のパスワード(std::io::stdin().lock())?
    } else {
        prompt_password_twice()?
    };

    let user = 作成する(&conn, &passwords, &username, email, &name, &password).await?;

    println!(
        "{}",
        t!(
            "console.admin_created",
            locale = l,
            id = user.id,
            username = user.username
        )
    );
    Ok(())
}

/// パスワードをリセットする。
///
/// 設計書20.7の通り、**一時パスワードを生成して一度だけ表示する。**
/// 平文はDBに保存せず、対象の利用者は次回ログイン時に変更を強制される。
/// あわせて対象利用者の既存セッションをすべて失効させる。
pub async fn reset_password(config: &Config, username: &str) -> anyhow::Result<()> {
    let l = console::言語();
    let conn = db::connect(&config.database).await?;
    let passwords = PasswordService::new(config.password.clone())?;
    let username = username::正規化する(username);

    let Some(user) = app_user::Entity::find()
        .filter(app_user::Column::Username.eq(&username))
        .one(&conn)
        .await?
    else {
        anyhow::bail!(
            "{}",
            t!("console.reset_not_found", locale = l, username = username)
        );
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
    println!(
        "  {}",
        t!("console.reset_issued", locale = l, username = username)
    );
    println!();
    println!("      {temporary}");
    println!();
    println!("  {}", t!("console.reset_once", locale = l));
    println!("  {}", t!("console.reset_must_change", locale = l));
    if 失効数 > 0 {
        println!(
            "  {}",
            t!("console.reset_revoked", locale = l, count = 失効数)
        );
    }
    println!();

    Ok(())
}

/// System Adminを作成する本体。**対話でも標準入力でも、ここを通る。**
///
/// 入力の受け取り方によって検査が変わらないようにする（ユーザー名の規則と重複・
/// メールアドレスの重複・表示名・パスワードポリシー）。
pub async fn 作成する(
    db: &DatabaseConnection,
    passwords: &PasswordService,
    username: &str,
    email: Option<&str>,
    name: &str,
    password: &str,
) -> anyhow::Result<app_user::Model> {
    let l = console::言語();
    let username = username::検証する(username)?;
    if 使われている(db, app_user::Column::Username, &username).await? {
        anyhow::bail!(
            "{}",
            t!(
                "console.admin_duplicate_username",
                locale = l,
                username = username
            )
        );
    }
    // メールアドレスは任意だが、値がある場合は一意（設計書20.1）
    let email = email.and_then(username::任意のメールアドレス);
    if let Some(email) = &email {
        if 使われている(db, app_user::Column::Email, email).await? {
            anyhow::bail!(
                "{}",
                t!("console.admin_duplicate_email", locale = l, email = email)
            );
        }
    }
    if name.trim().is_empty() {
        anyhow::bail!("{}", t!("console.admin_name_empty", locale = l));
    }
    passwords.check_policy(password, &username, name)?;

    Ok(setup::create_system_admin(
        db,
        passwords,
        name,
        &username,
        email.as_deref(),
        password,
        false,
        // 最初の管理者なら、倉庫用のプロジェクトの名前に使う（#217）
        l,
    )
    .await?)
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
        anyhow::bail!(
            "{}",
            t!("console.admin_stdin_empty", locale = console::言語())
        );
    }
    Ok(password.to_owned())
}

async fn 使われている(
    db: &DatabaseConnection,
    column: app_user::Column,
    value: &str,
) -> Result<bool, sea_orm::DbErr> {
    Ok(app_user::Entity::find()
        .filter(column.eq(value))
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
    let l = console::言語();
    let first = rpassword::prompt_password(t!("console.admin_password_prompt", locale = l))?;
    let second = rpassword::prompt_password(t!("console.admin_password_confirm", locale = l))?;

    if first != second {
        anyhow::bail!("{}", t!("console.admin_password_mismatch", locale = l));
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
