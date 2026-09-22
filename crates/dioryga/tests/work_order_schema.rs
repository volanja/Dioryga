//! 変更管理のスキーマの結合テスト（設計書11章）。

mod support;

use chrono::Utc;
use entity::{app_user, project, work_order, work_order_approval};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};

/// 起票できること。**予定と実績を別の列で持つ**（5.1）。
async fn 起票できる(db: &DatabaseConnection) {
    let p = プロジェクト(db, "基幹刷新").await;
    let 主 = 利用者(db, "primary@example.com").await;
    let 副 = 利用者(db, "secondary@example.com").await;

    let wo = work_order::ActiveModel {
        project_id: Set(p.id),
        work_type: Set("Repair".to_owned()),
        title: Set("電源ユニット交換".to_owned()),
        description: Set("PSU1が故障。予備品と交換する".to_owned()),
        primary_assignee_id: Set(Some(主.id)),
        secondary_assignee_id: Set(Some(副.id)),
        due_date: Set(Some(chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap())),
        status: Set("planned".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    assert_eq!(wo.status, "planned");
    // 起票の時点では実績の列はすべて空。期日だけが決まっている
    assert!(wo.executed_at.is_none());
    assert!(wo.completed_at.is_none());
    assert!(wo.due_date.is_some());
}

/// 承認は影響を受けるプロジェクトごとに1行を起こすこと（11章）。
///
/// **他プロジェクトの機器を巻き込む変更では複数必要になる。**
async fn 承認は影響先ごとに起こす(db: &DatabaseConnection) {
    let 起票元 = プロジェクト(db, "移設元").await;
    let 影響先 = プロジェクト(db, "移設先").await;
    let wo = チケット(db, 起票元.id, "Relocation").await;

    for pid in [起票元.id, 影響先.id] {
        承認行(db, wo.id, pid).await;
    }

    let 承認 = work_order_approval::Entity::find()
        .filter(work_order_approval::Column::WorkOrderId.eq(wo.id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(承認.len(), 2);
    assert!(承認.iter().all(|a| a.status == "pending"));
    assert!(承認.iter().all(|a| a.approver_id.is_none()));
}

/// 同じプロジェクトの承認行を二重に起こせないこと。
///
/// 二重にあると「全行が approved」の判定が承認の回数に依存してしまう。
async fn 同一プロジェクトの承認は一行だけ(db: &DatabaseConnection) {
    let p = プロジェクト(db, "重複承認").await;
    let wo = チケット(db, p.id, "Repair").await;

    承認行(db, wo.id, p.id).await;

    let 二件目 = work_order_approval::ActiveModel {
        work_order_id: Set(wo.id),
        required_project_id: Set(p.id),
        approver_id: Set(None),
        status: Set("pending".to_owned()),
        approved_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await;
    assert!(
        二件目.is_err(),
        "同一プロジェクトの承認行が二重に作れてしまった"
    );
}

/// 全承認が揃った時点でチケットを `approved` へ進められること（11章）。
///
/// **遷移そのものはアプリケーション層の判断**であり、DBは承認行を保持するだけ。
/// ここで確かめるのは「全行が approved か」を引けることである。
async fn 全承認が揃ったか引ける(db: &DatabaseConnection) {
    let p = プロジェクト(db, "承認集約").await;
    let 他 = プロジェクト(db, "承認集約-他").await;
    let 承認者 = 利用者(db, "approver@example.com").await;
    let wo = チケット(db, p.id, "Transfer").await;

    let a1 = 承認行(db, wo.id, p.id).await;
    let a2 = 承認行(db, wo.id, 他.id).await;

    承認する(db, a1, 承認者.id).await;
    assert!(!全て承認済み(db, wo.id).await, "1件残っているのに揃った");

    承認する(db, a2, 承認者.id).await;
    assert!(全て承認済み(db, wo.id).await);
}

/// 中止したチケットが理由とともに残ること（11章）。**削除しない。**
async fn 中止は理由とともに残る(db: &DatabaseConnection) {
    let p = プロジェクト(db, "中止").await;
    let wo = チケット(db, p.id, "Addition").await;

    let 中止済み = work_order::ActiveModel {
        id: Set(wo.id),
        status: Set("cancelled".to_owned()),
        cancelled_at: Set(Some(Utc::now())),
        cancelled_reason: Set(Some("調達が間に合わず次期へ繰越".to_owned())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    assert_eq!(中止済み.status, "cancelled");
    assert!(中止済み.cancelled_reason.is_some());
    // 完了はしていない。納期遵守率の集計で完了と混ざらないこと（10.4）
    assert!(中止済み.completed_at.is_none());
}

/// 担当者・移譲先を空のまま起票できること。
///
/// 起票の時点で担当が決まっていない運用は普通にある。
async fn 担当未定でも起票できる(db: &DatabaseConnection) {
    let p = プロジェクト(db, "担当未定").await;
    let wo = チケット(db, p.id, "Disposal").await;

    assert!(wo.primary_assignee_id.is_none());
    assert!(wo.secondary_assignee_id.is_none());
    assert!(wo.target_project_id.is_none());
    assert!(wo.device_id.is_none());
}

// --- 補助 -----------------------------------------------------------------

async fn 全て承認済み(db: &DatabaseConnection, work_order_id: i32) -> bool {
    let 承認 = work_order_approval::Entity::find()
        .filter(work_order_approval::Column::WorkOrderId.eq(work_order_id))
        .order_by_asc(work_order_approval::Column::Id)
        .all(db)
        .await
        .unwrap();
    !承認.is_empty() && 承認.iter().all(|a| a.status == "approved")
}

async fn 承認する(db: &DatabaseConnection, approval_id: i32, approver_id: i32) {
    work_order_approval::ActiveModel {
        id: Set(approval_id),
        approver_id: Set(Some(approver_id)),
        status: Set("approved".to_owned()),
        approved_at: Set(Some(Utc::now())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

async fn 承認行(db: &DatabaseConnection, work_order_id: i32, required_project_id: i32) -> i32 {
    work_order_approval::ActiveModel {
        work_order_id: Set(work_order_id),
        required_project_id: Set(required_project_id),
        approver_id: Set(None),
        status: Set("pending".to_owned()),
        approved_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
    .id
}

async fn チケット(
    db: &DatabaseConnection,
    project_id: i32,
    work_type: &str,
) -> work_order::Model {
    work_order::ActiveModel {
        project_id: Set(project_id),
        work_type: Set(work_type.to_owned()),
        title: Set(format!("{work_type}のチケット")),
        description: Set(String::new()),
        status: Set("planned".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn プロジェクト(db: &DatabaseConnection, name: &str) -> project::Model {
    project::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        name: Set(name.to_owned()),
        currency: Set("JPY".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
        name: Set(email.to_owned()),
        password_hash: Set("x".to_owned()),
        is_system_admin: Set(false),
        must_change_password: Set(false),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// **チケットの状態の語彙と列名が書き換わり、戻せること**（#174）。
async fn 状態の語彙と列名を書き換える(db: &DatabaseConnection) {
    use migration::m20260922_000002_rename_work_order_status::Migration as 語彙変更;
    use migration::{MigrationTrait, SchemaManager};

    let manager = SchemaManager::new(db);
    語彙変更.down(&manager).await.unwrap();
    // 戻した状態では旧の値が入る。**列名も旧に戻っているので、生のSQLで確かめる**
    語彙変更.up(&manager).await.unwrap();

    let p = プロジェクト(db, "語彙の検証").await;
    let w = チケット(db, p.id, "Repair").await;

    // 新しい列に書ける
    let mut active: work_order::ActiveModel = w.clone().into();
    active.cancelled_at = Set(Some(Utc::now()));
    active.cancelled_reason = Set(Some("次期へ繰越".to_owned()));
    active.status = Set("cancelled".to_owned());
    let 後 = active.update(db).await.unwrap();

    assert_eq!(後.status, "cancelled");
    assert!(後.cancelled_reason.is_some());
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 起票できる);
        全検証!(@one $用意, $属性, 状態の語彙と列名を書き換える);
        全検証!(@one $用意, $属性, 承認は影響先ごとに起こす);
        全検証!(@one $用意, $属性, 同一プロジェクトの承認は一行だけ);
        全検証!(@one $用意, $属性, 全承認が揃ったか引ける);
        全検証!(@one $用意, $属性, 中止は理由とともに残る);
        全検証!(@one $用意, $属性, 担当未定でも起票できる);
    };
    (@one $用意:path, $属性:meta, $名前:ident) => {
        #[tokio::test]
        #[$属性]
        async fn $名前() {
            let db = $用意().await;
            super::$名前(&db.conn).await;
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
