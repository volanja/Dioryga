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
//!
//! 利用者を作ると、この先へ `Viewer` として加える（#218）。画面（System Admin の
//! ユーザー登録）と組織データの取込の両方が [`既定の参加先に加える`] を通る。
//!
//! # 倉庫プロジェクトの判別
//!
//! 倉庫にある機器・部品の `status` には意味が無い（#221、設計書6.3）。画面は
//! `status` を出さずに「保管中」と出し、払い出しで `planned` にする。**倉庫
//! プロジェクトかどうかは [`倉庫プロジェクトか`] だけで判別する**——既定の
//! 参加先を倉庫とみなす、という読み替えを1か所に閉じ込める。

use chrono::Utc;
use entity::{app_setting, app_user, project, project_member};
use sea_orm::{ConnectionTrait, DbErr, EntityTrait, Set};

use crate::auth::authorization::VIEWER;
use crate::repository::AuditedTx;

/// 新しい利用者を加える先のプロジェクト。初回セットアップの前は `None`。
pub async fn 既定の参加先<C: ConnectionTrait>(db: &C) -> Result<Option<i32>, DbErr> {
    Ok(app_setting::Entity::find_by_id(app_setting::ID)
        .one(db)
        .await?
        .map(|s| s.default_project_id))
}

/// 倉庫プロジェクトか（#221、設計書6.3）。**判別はここだけで行う。**
///
/// 区別の印は持たず、新しい利用者の既定の参加先を倉庫とみなす（#217）。
pub async fn 倉庫プロジェクトか<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> Result<bool, DbErr> {
    Ok(既定の参加先(db).await? == Some(project_id))
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

/// 新しく作った利用者を、既定の参加先に `Viewer` として加える（#218、設計書16.1）。
///
/// **在庫を全員に見せるため。**倉庫の予備は倉庫用のプロジェクトにあり、
/// メンバーにしか見えない。「予備はあるか」に全員が答えられるようにする。
///
/// - **System Admin は加えない。**プロジェクトのメンバーになれない（3章）
/// - **利用者を作ったときだけ呼ぶ。**作る前からいる利用者を加え直すことはしない
/// - 監査ログは `by` を主体に**行ごとに残す。**取込でも同じ（23.8、24.4の例外）
///
/// 加えた先のプロジェクトを返す。既定の参加先が無いDB（初回セットアップ前）や
/// System Admin では `None`。
pub async fn 既定の参加先に加える(
    tx: &AuditedTx,
    user: &app_user::Model,
    by: i32,
) -> Result<Option<project::Model>, DbErr> {
    if user.is_system_admin {
        return Ok(None);
    }
    let Some(project_id) = 既定の参加先(tx.reader()).await? else {
        return Ok(None);
    };
    let Some(先) = project::Entity::find_by_id(project_id)
        .one(tx.reader())
        .await?
    else {
        return Ok(None);
    };

    let now = Utc::now();
    tx.insert_recorded(
        project_member::ActiveModel {
            user_id: Set(user.id),
            project_id: Set(先.id),
            role: Set(VIEWER.to_owned()),
            admin_rank: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        },
        by,
    )
    .await?;
    Ok(Some(先))
}
