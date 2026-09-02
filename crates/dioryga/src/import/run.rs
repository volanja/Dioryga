//! `dioryga import` の実行（設計書23.6、23.7）。
//!
//! **既定はドライラン。**18.2により参照されたカタログ行は編集できず、誤った
//! 取込は事後修正が困難である。書き込むには `--apply` を明示させる。
//!
//! # 誰が取り込んだかを記録する
//!
//! `created_by` と `IMPORT_RUN.imported_by` には実在する利用者が要る。
//! CLIにはログインの概念がないため `--as-user` で指定させ、**その利用者が
//! 実際に取り込んでよいかを確かめる**（18.1）。確かめずに記録すると、
//! 「この取込は誰の責任か」という問いに嘘の答えを残すことになる。

use std::path::Path;

use chrono::Utc;
use entity::{app_user, import_run};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use super::{catalog, file_hash, ImportError, Outcome, Report};
use crate::auth::authorization;

/// 取込の対象と結果。
pub struct Executed {
    pub report: Report,
    /// 反映した場合のみ。ドライランでは `None`。
    pub import_run_id: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("利用者が見つかりません: {0}")]
    UnknownUser(String),

    #[error("この利用者は取込を行えません（いずれかのプロジェクトでOperator以上が要ります）")]
    NotPermitted,

    #[error(transparent)]
    Import(#[from] ImportError),

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

/// カタログYAMLを取り込む。
pub async fn catalog_file(
    db: &DatabaseConnection,
    path: &Path,
    as_user: &str,
    apply: bool,
) -> Result<Executed, RunError> {
    let actor = 取込者(db, as_user).await?;

    let bytes = std::fs::read(path).map_err(ImportError::Io)?;
    let source = String::from_utf8_lossy(&bytes);
    let file = catalog::parse(&source)?;

    let report = catalog::dry_run(db, &file).await?;

    if !apply {
        return Ok(Executed {
            report,
            import_run_id: None,
        });
    }

    // **エラーが1件でもあれば反映しない。**部分的に入ると、どこまで入ったのかを
    // 利用者が把握できない
    if report.has_error() {
        return Err(ImportError::HasErrors(report.count(Outcome::Error)).into());
    }

    let now = Utc::now();
    // 記録を先に作る。取り込んだ行から参照される（23.7）
    let run = import_run::ActiveModel {
        // カタログ取込はプロジェクトに属さない（18.1）
        project_id: Set(None),
        kind: Set("catalog".to_owned()),
        file_hash: Set(file_hash(&bytes)),
        // カタログYAMLには as_of が無い。実行時刻を基準とする（23.1）
        as_of: Set(now),
        created_count: Set(report.count(Outcome::Created) as i32),
        updated_count: Set(report.count(Outcome::Updated) as i32),
        warning_count: Set(report.count(Outcome::Warning) as i32),
        imported_by: Set(actor.id),
        imported_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await?;

    let report = catalog::apply(db, &file, actor.id, run.id).await?;

    Ok(Executed {
        report,
        import_run_id: Some(run.id),
    })
}

/// 取込を行う利用者を解決し、権限を確かめる（設計書18.1）。
async fn 取込者(db: &DatabaseConnection, email: &str) -> Result<app_user::Model, RunError> {
    let user = app_user::Entity::find()
        .filter(app_user::Column::Email.eq(email))
        .one(db)
        .await?
        .ok_or_else(|| RunError::UnknownUser(email.to_owned()))?;

    // カタログの作成・編集は「いずれか1つ以上のプロジェクトでOperator以上」。
    // **System Adminは含まれない**（3章）
    authorization::require_catalog_editor(db, &user)
        .await
        .map_err(|_| RunError::NotPermitted)?;

    Ok(user)
}

/// 差分レポートを標準出力へ書く。
pub fn print_report(executed: &Executed, apply: bool) {
    println!();
    println!("  {}", executed.report);
    println!();

    for entry in executed.report.errors() {
        println!("  エラー  {} — {}", entry.target, entry.detail);
    }
    for entry in executed.report.warnings() {
        println!("  警告    {} — {}", entry.target, entry.detail);
    }

    if executed.report.errors().count() > 0 || executed.report.warnings().count() > 0 {
        println!();
    }

    match executed.import_run_id {
        Some(id) => println!("  反映しました（IMPORT_RUN #{id}）"),
        None if apply => println!("  反映していません"),
        None => println!("  ドライランです。反映するには --apply を付けてください"),
    }
    println!();
}
