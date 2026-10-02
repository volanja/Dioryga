//! アプリ全体の設定（`APP_SETTING`、#217、設計書16.1）。
//!
//! # 倉庫用のプロジェクト
//!
//! 予備の機器・部品は、アプリが最初から用意する倉庫用のプロジェクトに置く
//! （#196）。**最初の System Admin を作るときに作る**——セットアップ画面でも
//! CLI（`dioryga admin create`）でも同じ。後から作る運用は無い。
//!
//! **名前は作った時点の言語で決め、以後は訳さない。**プロジェクト名は利用者の
//! データであり、改名もできる。画面の言語で訳すと、改名した名前と食い違う。
//!
//! # 印ではなく参加先
//!
//! 倉庫用のプロジェクトかどうかの印は持たない。アプリが持つのは**新しい利用者
//! を加える先**（`default_project_id`）だけで、それ以外の画面は通常の
//! プロジェクトと同じに扱う。

use chrono::Utc;
use entity::{app_setting, project};
use sea_orm::{ConnectionTrait, DbErr, EntityTrait, Set};

use crate::repository::AuditedTx;

/// 新しい利用者を加える先のプロジェクト。初回セットアップの前は `None`。
pub async fn 既定の参加先<C: ConnectionTrait>(db: &C) -> Result<Option<i32>, DbErr> {
    Ok(app_setting::Entity::find_by_id(app_setting::ID)
        .one(db)
        .await?
        .map(|s| s.default_project_id))
}

/// 倉庫用のプロジェクトを作り、新しい利用者の既定の参加先にする。
///
/// **既に設定があれば何もしない。**`by` は作成者として監査ログに残す利用者
/// （最初の System Admin）。`locale` はプロジェクトの名前と説明の言語。
pub async fn 倉庫プロジェクトを用意する(
    tx: &AuditedTx,
    by: i32,
    locale: &str,
) -> Result<Option<project::Model>, DbErr> {
    if 既定の参加先(tx.reader()).await?.is_some() {
        return Ok(None);
    }

    let now = Utc::now();
    let 倉庫 = tx
        .insert_recorded(
            project::ActiveModel {
                uid: Set(uuid::Uuid::new_v4().to_string()),
                code: Set(None),
                name: Set(rust_i18n::t!("setup.warehouse_project_name", locale = locale).into()),
                description: Set(rust_i18n::t!(
                    "setup.warehouse_project_description",
                    locale = locale
                )
                .into()),
                currency: Set(crate::currency::DEFAULT.to_owned()),
                archived_at: Set(None),
                closure_reason: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            },
            by,
        )
        .await?;

    tx.insert_recorded(
        app_setting::ActiveModel {
            id: Set(app_setting::ID),
            default_project_id: Set(倉庫.id),
            created_at: Set(now),
            updated_at: Set(now),
        },
        by,
    )
    .await?;

    Ok(Some(倉庫))
}
