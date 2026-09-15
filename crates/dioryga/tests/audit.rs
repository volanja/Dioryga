//! リポジトリ層と監査ログの結合テスト。
//!
//! DB基盤のテスト（`db.rs`）と同様、**同一のテスト本体をSQLiteとPostgreSQLの
//! 双方で実行する。**

mod support;

use chrono::Utc;
use dioryga::repository::{Actor, AuditedTx};
use entity::{app_user, audit_log, project};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
};

// ---------------------------------------------------------------------------
// テスト本体
// ---------------------------------------------------------------------------

/// 追加・更新・削除のそれぞれが監査ログに残ること。
async fn 変更が監査ログに自動で記録される(db: &DatabaseConnection) {
    let tx = AuditedTx::begin(db, Actor::SelfCreated).await.unwrap();
    let user = tx.insert(新規利用者("audit@example.com")).await.unwrap();
    tx.commit().await.unwrap();

    // 以降はこの利用者の操作として記録する
    let tx = AuditedTx::begin(db, Actor::User(user.id)).await.unwrap();
    let p = tx.insert(新規プロジェクト("監査検証")).await.unwrap();

    let mut 変更 = p.clone().into_active_model();
    変更.name = Set("監査検証（改称）".to_owned());
    let 更新後 = tx.update(&p, 変更).await.unwrap();

    tx.delete(更新後).await.unwrap();
    tx.commit().await.unwrap();

    let 記録 = 監査ログ(db, "project").await;
    let 操作 = 記録.iter().map(|r| r.action.as_str()).collect::<Vec<_>>();
    assert_eq!(操作, vec!["insert", "update", "delete"]);

    // insert は変更前が無く、delete は変更後が無い
    assert!(記録[0].before_json.is_none());
    assert!(記録[0].after_json.is_some());
    assert!(記録[1].before_json.as_ref().unwrap().contains("監査検証"));
    assert!(記録[1].after_json.as_ref().unwrap().contains("改称"));
    assert!(記録[2].before_json.is_some());
    assert!(記録[2].after_json.is_none());

    assert!(記録.iter().all(|r| r.user_id == user.id));
}

/// 本来の変更と同一トランザクションで書かれること（設計書15.2）。
async fn ロールバックすると監査ログも残らない(db: &DatabaseConnection) {
    let tx = AuditedTx::begin(db, Actor::SelfCreated).await.unwrap();
    let user = tx.insert(新規利用者("rollback@example.com")).await.unwrap();
    tx.commit().await.unwrap();

    let tx = AuditedTx::begin(db, Actor::User(user.id)).await.unwrap();
    tx.insert(新規プロジェクト("巻き戻す")).await.unwrap();
    tx.rollback().await.unwrap();

    let 件数 = project::Entity::find()
        .filter(project::Column::Name.eq("巻き戻す"))
        .all(db)
        .await
        .unwrap()
        .len();
    assert_eq!(件数, 0, "ロールバックしたのに行が残っています");

    assert!(
        監査ログ(db, "project").await.is_empty(),
        "ロールバックしたのに監査ログが残っています"
    );
}

/// 機微なカラムが平文で記録されないこと（設計書21.7）。
async fn パスワードハッシュは監査ログに残らない(db: &DatabaseConnection) {
    let tx = AuditedTx::begin(db, Actor::SelfCreated).await.unwrap();
    tx.insert(新規利用者("secret@example.com")).await.unwrap();
    tx.commit().await.unwrap();

    let 記録 = 監査ログ(db, "app_user").await;
    let after = 記録[0].after_json.as_ref().unwrap();

    assert!(
        !after.contains("argon2id"),
        "パスワードハッシュが監査ログに漏れています: {after}"
    );
    assert!(after.contains("***"));
    // 伏せる対象でないカラムは記録される
    assert!(after.contains("secret@example.com"));
}

/// 初回セットアップでは、作成された当人が主体として記録されること（設計書20.8）。
async fn 自己作成では当人が主体になる(db: &DatabaseConnection) {
    let tx = AuditedTx::begin(db, Actor::SelfCreated).await.unwrap();
    let user = tx.insert(新規利用者("setup@example.com")).await.unwrap();
    tx.commit().await.unwrap();

    let 記録 = 監査ログ(db, "app_user").await;
    assert_eq!(記録[0].user_id, user.id);
    assert_eq!(記録[0].record_id, user.id);
}

/// 一括取込では行ごとの監査ログを書かないこと（設計書24.4）。
async fn 一括取込では監査ログを書かない(db: &DatabaseConnection) {
    let tx = AuditedTx::begin(db, Actor::SelfCreated).await.unwrap();
    tx.insert(新規利用者("importer@example.com")).await.unwrap();
    tx.commit().await.unwrap();
    let 取込前 = 監査ログ(db, "project").await.len();

    let tx = AuditedTx::begin(db, Actor::Import { import_run_id: 1 })
        .await
        .unwrap();
    tx.insert(新規プロジェクト("取込で作られた")).await.unwrap();
    tx.commit().await.unwrap();

    // 行そのものは作られる
    let 件数 = project::Entity::find()
        .filter(project::Column::Name.eq("取込で作られた"))
        .all(db)
        .await
        .unwrap()
        .len();
    assert_eq!(件数, 1);

    // 監査ログは増えない（追跡は IMPORT_RUN が担う）
    assert_eq!(監査ログ(db, "project").await.len(), 取込前);
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 新規利用者(email: &str) -> app_user::ActiveModel {
    app_user::ActiveModel {
        name: Set("検証用".to_owned()),
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        password_hash: Set("$argon2id$v=19$m=19456,t=2,p=1$abc$def".to_owned()),
        must_change_password: Set(false),
        is_system_admin: Set(true),
        locale: Set("ja".to_owned()),
        last_login_at: Set(None),
        disabled_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
}

fn 新規プロジェクト(name: &str) -> project::ActiveModel {
    project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(None),
        name: Set(name.to_owned()),
        description: Set(String::new()),
        currency: Set("JPY".to_owned()),
        archived_at: Set(None),
        closure_reason: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
}

async fn 監査ログ(db: &DatabaseConnection, table: &str) -> Vec<audit_log::Model> {
    audit_log::Entity::find()
        .filter(audit_log::Column::TableName.eq(table))
        .order_by_asc(audit_log::Column::Id)
        .all(db)
        .await
        .unwrap()
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        #[tokio::test]
        #[$属性]
        async fn 変更が監査ログに自動で記録される() {
            let db = $用意().await;
            super::変更が監査ログに自動で記録される(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn ロールバックすると監査ログも残らない() {
            let db = $用意().await;
            super::ロールバックすると監査ログも残らない(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn パスワードハッシュは監査ログに残らない() {
            let db = $用意().await;
            super::パスワードハッシュは監査ログに残らない(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 自己作成では当人が主体になる() {
            let db = $用意().await;
            super::自己作成では当人が主体になる(&db.conn).await;
        }

        #[tokio::test]
        #[$属性]
        async fn 一括取込では監査ログを書かない() {
            let db = $用意().await;
            super::一括取込では監査ログを書かない(&db.conn).await;
        }
    };
}

mod sqlite {
    全検証!(crate::support::sqlite, cfg(all()));
}

mod postgres {
    全検証!(
        crate::support::postgres,
        ignore = "Dockerが必要。cargo test -- --ignored で実行する"
    );
}
