//! DB基盤の結合テスト。
//!
//! **同一のテスト本体をSQLiteとPostgreSQLの双方で実行する。**v1から両DBに対応する
//! という方針（設計書2章）が、スキーマと型の対応において実際に成立していることを
//! 確認するのが目的。
//!
//! PostgreSQL側は Docker を必要とするため `#[ignore]` を付けている。
//! CIでは専用ジョブが `cargo test -- --ignored` で実行する。

use chrono::{TimeZone, Utc};
use entity::{app_user, project, project_member};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

// ---------------------------------------------------------------------------
// テスト本体（バックエンドに依存しない）
// ---------------------------------------------------------------------------

/// 日時がUTCのまま往復すること（設計書24.2.3）。
async fn 日時が協定世界時のまま往復する(db: &DatabaseConnection) {
    let 登録時刻 = Utc.with_ymd_and_hms(2026, 3, 15, 1, 23, 45).unwrap();

    let user = 利用者を作る(db, "utc@example.com", 登録時刻).await;
    let 読み直し = app_user::Entity::find_by_id(user.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(読み直し.created_at, 登録時刻);
    assert_eq!(読み直し.last_login_at, None);
}

/// UUIDを文字列として持つ方針が両DBで機能すること（設計書24.2.3）。
async fn uidが文字列として往復する(db: &DatabaseConnection) {
    let uid = uuid::Uuid::new_v4().to_string();
    let p = プロジェクトを作る(db, &uid, Some("PRJ-001"), "基幹システム更改").await;

    let 読み直し = project::Entity::find_by_id(p.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(読み直し.uid, uid);
    assert_eq!(読み直し.code.as_deref(), Some("PRJ-001"));
    // サービス上の状態は持たない。archived_at は運用上の整理の状態（設計書5.2）
    assert_eq!(読み直し.archived_at, None);
    assert_eq!(読み直し.closure_reason, None);
}

/// メールアドレスの一意制約が効くこと。
async fn メールアドレスが重複できない(db: &DatabaseConnection) {
    利用者を作る(db, "dup@example.com", Utc::now()).await;
    let 二人目 = app_user::ActiveModel {
        name: Set("二人目".to_owned()),
        username: Set(("dup@example.com".to_owned()).replace('@', "_")),
        email: Set(Some("dup@example.com".to_owned())),
        password_hash: Set("dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set("ja".to_owned()),
        last_login_at: Set(None),
        disabled_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;

    assert!(
        二人目.is_err(),
        "同じメールアドレスで登録できてしまいました"
    );
}

/// 同一ユーザーが同一プロジェクトで複数ロールを兼務でき、
/// かつ同じロールの重複は拒否されること（設計書5章、旧C-8）。
async fn 複数ロールを兼務できるが同じロールは重複できない(
    db: &DatabaseConnection,
) {
    let user = 利用者を作る(db, "member@example.com", Utc::now()).await;
    let p = プロジェクトを作る(db, &uuid::Uuid::new_v4().to_string(), None, "ロール検証").await;

    メンバーにする(db, user.id, p.id, "Operator").await.unwrap();
    メンバーにする(db, user.id, p.id, "Approver")
        .await
        .expect("OperatorとApproverの兼務ができませんでした");

    let 重複 = メンバーにする(db, user.id, p.id, "Operator").await;
    assert!(重複.is_err(), "同じロールを重複して登録できてしまいました");

    let 件数 = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(user.id))
        .all(db)
        .await
        .unwrap()
        .len();
    assert_eq!(件数, 2);
}

/// 無効化は物理削除ではなくタイムスタンプで表すこと（設計書20.11）。
async fn 無効化しても行は残る(db: &DatabaseConnection) {
    let user = 利用者を作る(db, "disabled@example.com", Utc::now()).await;

    let mut active: app_user::ActiveModel = user.clone().into();
    active.disabled_at = Set(Some(Utc::now()));
    active.update(db).await.unwrap();

    let 読み直し = app_user::Entity::find_by_id(user.id)
        .one(db)
        .await
        .unwrap()
        .expect("無効化した利用者の行が消えています");
    assert!(読み直し.disabled_at.is_some());
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 利用者を作る(
    db: &DatabaseConnection,
    email: &str,
    created_at: chrono::DateTime<Utc>,
) -> app_user::Model {
    app_user::ActiveModel {
        name: Set("検証用".to_owned()),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        password_hash: Set("dummy".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(false),
        locale: Set("ja".to_owned()),
        last_login_at: Set(None),
        disabled_at: Set(None),
        created_at: Set(created_at),
        updated_at: Set(created_at),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn プロジェクトを作る(
    db: &DatabaseConnection,
    uid: &str,
    code: Option<&str>,
    name: &str,
) -> project::Model {
    project::ActiveModel {
        uid: Set(uid.to_owned()),
        code: Set(code.map(str::to_owned)),
        name: Set(name.to_owned()),
        description: Set(String::new()),
        currency: Set("JPY".to_owned()),
        archived_at: Set(None),
        closure_reason: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn メンバーにする(
    db: &DatabaseConnection,
    user_id: i32,
    project_id: i32,
    role: &str,
) -> Result<project_member::Model, sea_orm::DbErr> {
    project_member::ActiveModel {
        user_id: Set(user_id),
        project_id: Set(project_id),
        role: Set(role.to_owned()),
        admin_rank: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
}

/// 上の検証をまとめて実行する。バックエンドごとにDBを作り直す。
macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        #[tokio::test]
        #[$属性]
        async fn 日時が協定世界時のまま往復する() {
            let db = $用意().await;
            super::日時が協定世界時のまま往復する(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn uidが文字列として往復する() {
            let db = $用意().await;
            super::uidが文字列として往復する(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn メールアドレスが重複できない() {
            let db = $用意().await;
            super::メールアドレスが重複できない(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 複数ロールを兼務できるが同じロールは重複できない() {
            let db = $用意().await;
            super::複数ロールを兼務できるが同じロールは重複できない(
                &db.conn,
            )
            .await;
        }

        #[tokio::test]
        #[$属性]
        async fn 無効化しても行は残る() {
            let db = $用意().await;
            super::無効化しても行は残る(&db.conn).await;
        }
    };
}

mod sqlite {
    全検証!(crate::support::sqlite, cfg(all()));
}

mod postgres {
    // Dockerが必要なため既定では実行しない。CIの専用ジョブが --ignored で実行する。
    全検証!(
        crate::support::postgres,
        ignore = "Dockerが必要。cargo test -- --ignored で実行する"
    );
}
