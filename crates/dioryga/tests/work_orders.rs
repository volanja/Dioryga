//! 変更管理チケット画面の結合テスト（設計書11章）。
//!
//! 16.7で「v1で最も重い画面」とした箇所。**画面が出るかではなく、ワークフローの
//! 規約が守られているか**を確かめる。とくに次の3つは、破れても画面上は正常に
//! 見えてしまうため、テストでしか守れない。
//!
//! - 承認が1件でも欠けているうちは `approved` にならない（11.4-7）
//! - 自分が担当のチケットを自分で承認できない（11.4-9）
//! - ただし承認できる他のメンバーがいなければ許され、記録が残る（22章R-2）

mod support;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{app_user, audit_log, project, project_member, work_order, work_order_approval};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// 起票
// ---------------------------------------------------------------------------

/// 起票すると承認行が1件できること（設計書11.5）。
async fn 起票すると承認待ちになる(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "create@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/work-orders", p.id),
        &token,
        &[
            ("work_type", "Repair"),
            ("title", "電源ユニット交換"),
            ("description", "PSU1が故障"),
            ("due_date", "2026-10-01"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let w = 唯一のチケット(db).await;
    assert_eq!(w.status, "planned");
    assert!(w.planned_at.is_some());
    assert!(w.executed_at.is_none(), "起票の時点で実績が入っている");

    let 承認 = 承認行(db, w.id).await;
    assert_eq!(承認.len(), 1, "起票元の承認だけができるはず");
    assert_eq!(承認[0].required_project_id, p.id);
    assert_eq!(承認[0].status, "pending");
}

/// **Transferでは承認行が2件できること**（設計書11.5）。
///
/// 移譲元・移譲先の両方が承認しないと進まない。
async fn 移譲では承認が二件になる(db: &DatabaseConnection) {
    let (user, 移譲元) = 準備(db, "transfer@example.com", "Operator").await;
    let 移譲先 = プロジェクト(db, "移譲先").await;
    メンバー(db, user.id, 移譲先.id, "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/work-orders", 移譲元.id),
        &token,
        &[
            ("work_type", "Transfer"),
            ("title", "検証機をB課へ移譲"),
            ("target_project_id", &移譲先.id.to_string()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let w = 唯一のチケット(db).await;
    assert_eq!(w.target_project_id, Some(移譲先.id));

    let 承認 = 承認行(db, w.id).await;
    assert_eq!(承認.len(), 2);
    let 宛先: Vec<i32> = 承認.iter().map(|a| a.required_project_id).collect();
    assert!(宛先.contains(&移譲元.id));
    assert!(宛先.contains(&移譲先.id));
}

/// Transfer以外で移譲先を指定できないこと。
///
/// **黙って無視しない**（Q-21）。指定した側は移譲されるつもりでいるため、
/// 無視すると意図と結果がずれたまま進む。
async fn 移譲先はtransferでしか指定できない(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "notransfer@example.com", "Operator").await;
    let 他 = プロジェクト(db, "無関係").await;
    メンバー(db, user.id, 他.id, "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/work-orders", p.id),
        &token,
        &[
            ("work_type", "Repair"),
            ("title", "修理"),
            ("target_project_id", &他.id.to_string()),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "エラーとして再表示されるはず");
    assert!(body.contains("Transfer でのみ"));
    assert_eq!(チケット数(db).await, 0);
}

/// 語彙外の `work_type` を拒否すること（Q-21）。
async fn 語彙外の種別は拒否される(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "vocab@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/work-orders", p.id),
        &token,
        &[("work_type", "Renovation"), ("title", "改修")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("種別の値が不正"));
    assert_eq!(チケット数(db).await, 0);
}

/// Approverは起票できないこと（既存の `require_project_editor`）。
async fn 承認者は起票できない(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "approver-create@example.com", "Approver").await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/work-orders", p.id),
        &token,
        &[("work_type", "Repair"), ("title", "勝手に起票")],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(チケット数(db).await, 0);
}

// ---------------------------------------------------------------------------
// 承認（設計書11.4-7）
// ---------------------------------------------------------------------------

/// **承認が1件でも欠けているうちは `approved` にならないこと**（設計書11.4-7）。
///
/// 承認ボタンが押すのは承認行であって、チケットの状態ではない。ここを取り違えると
/// 承認が足りないまま実行できてしまう。
async fn 全部揃うまで承認済みにならない(db: &DatabaseConnection) {
    let (起票者, 移譲元) = 準備(db, "t-owner@example.com", "Operator").await;
    let 移譲先 = プロジェクト(db, "移譲先-承認").await;
    メンバー(db, 起票者.id, 移譲先.id, "Operator").await;

    let 移譲元の承認者 = 利用者(db, "approver-a@example.com").await;
    メンバー(db, 移譲元の承認者.id, 移譲元.id, "Approver").await;
    let 移譲先の承認者 = 利用者(db, "approver-b@example.com").await;
    メンバー(db, 移譲先の承認者.id, 移譲先.id, "Approver").await;

    let w = 起票(db, 移譲元.id, Some(移譲先.id), "Transfer", None).await;
    承認行を作る(db, w.id, &[移譲元.id, 移譲先.id]).await;
    let 承認 = 承認行(db, w.id).await;

    // 1件目
    let (状態, token) = 認証済み(db, &移譲元の承認者).await;
    let (status, _) = 承認する(状態, &token, 移譲元.id, w.id, 承認[0].id, "approve").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        再取得(db, w.id).await.status,
        "planned",
        "1件しか承認されていないのに進んでいる"
    );

    // 2件目。**移譲先のメンバーとして承認する**
    let (状態, token) = 認証済み(db, &移譲先の承認者).await;
    let (status, _) = 承認する(状態, &token, 移譲先.id, w.id, 承認[1].id, "approve").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(再取得(db, w.id).await.status, "approved");
}

/// 却下があるうちは進まないこと。
async fn 却下があると進まない(db: &DatabaseConnection) {
    let (起票者, p) = 準備(db, "reject-owner@example.com", "Operator").await;
    let 承認者 = 利用者(db, "rejecter@example.com").await;
    メンバー(db, 承認者.id, p.id, "Approver").await;
    let _ = 起票者;

    let w = 起票(db, p.id, None, "Repair", None).await;
    承認行を作る(db, w.id, &[p.id]).await;
    let 承認 = 承認行(db, w.id).await;

    let (状態, token) = 認証済み(db, &承認者).await;
    承認する(状態, &token, p.id, w.id, 承認[0].id, "reject").await;

    assert_eq!(再取得(db, w.id).await.status, "planned");
    assert_eq!(承認行(db, w.id).await[0].status, "rejected");
}

/// **自分が担当のチケットは承認できないこと**（設計書11.4-9、旧C-9）。
///
/// 他に承認できるメンバーがいる場合。
async fn 担当者は自分で承認できない(db: &DatabaseConnection) {
    let (担当, p) = 準備(db, "self-deny@example.com", "Administrator").await;
    // 他に承認できる人がいる。**この存在が判定を分ける**
    let 他 = 利用者(db, "other-approver@example.com").await;
    メンバー(db, 他.id, p.id, "Approver").await;

    let w = 起票(db, p.id, None, "Repair", Some(担当.id)).await;
    承認行を作る(db, w.id, &[p.id]).await;
    let 承認 = 承認行(db, w.id).await;

    let (状態, token) = 認証済み(db, &担当).await;
    let (status, body) = 承認する(状態, &token, p.id, w.id, 承認[0].id, "approve").await;

    assert_eq!(status, StatusCode::OK, "エラーとして再表示されるはず");
    assert!(body.contains("自分が担当のチケットは承認できません"));
    assert_eq!(承認行(db, w.id).await[0].status, "pending");
    assert_eq!(再取得(db, w.id).await.status, "planned");
}

/// **承認できる他のメンバーがいなければ自己承認を許すこと**（22章R-2）。
///
/// 個人利用（SQLite単一バイナリ）を想定利用形態に掲げている以上、承認者が
/// 自分しかいない環境でチケットが永久に承認されないのは避けなければならない。
/// **承認ステップは省略せず、自己承認として記録する。**
async fn 他に承認者がいなければ自己承認できる(db: &DatabaseConnection) {
    let (担当, p) = 準備(db, "solo@example.com", "Administrator").await;

    let w = 起票(db, p.id, None, "Repair", Some(担当.id)).await;
    承認行を作る(db, w.id, &[p.id]).await;
    let 承認 = 承認行(db, w.id).await;

    let (状態, token) = 認証済み(db, &担当).await;
    let (status, _) = 承認する(状態, &token, p.id, w.id, 承認[0].id, "approve").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 済 = &承認行(db, w.id).await[0];
    assert_eq!(済.status, "approved");
    assert_eq!(済.approver_id, Some(担当.id));
    assert!(済.self_approved, "自己承認として記録されていない");
    assert_eq!(再取得(db, w.id).await.status, "approved");

    // **通常の承認と区別できる形で監査ログに残ること**（11.4-9）
    let ログ = audit_log::Entity::find()
        .filter(audit_log::Column::TableName.eq("work_order_approval"))
        .all(db)
        .await
        .unwrap();
    assert!(!ログ.is_empty(), "承認が監査ログに残っていない");
    assert!(
        ログ.iter().any(|l| l
            .after_json
            .as_deref()
            .is_some_and(|j| j.contains("\"self_approved\":true"))),
        "自己承認だったことが監査ログから読み取れない"
    );
}

/// 無効化された承認者は「他の承認者」として数えないこと（設計書20.11）。
///
/// 無効化された利用者はログインできないため、いても承認は進まない。
async fn 無効化された承認者は数えない(db: &DatabaseConnection) {
    let (担当, p) = 準備(db, "disabled-peer@example.com", "Administrator").await;
    let 退職者 = 利用者(db, "retired@example.com").await;
    メンバー(db, 退職者.id, p.id, "Approver").await;
    app_user::ActiveModel {
        id: Set(退職者.id),
        disabled_at: Set(Some(Utc::now())),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let w = 起票(db, p.id, None, "Repair", Some(担当.id)).await;
    承認行を作る(db, w.id, &[p.id]).await;
    let 承認 = 承認行(db, w.id).await;

    let (状態, token) = 認証済み(db, &担当).await;
    let (status, _) = 承認する(状態, &token, p.id, w.id, 承認[0].id, "approve").await;

    assert_eq!(status, StatusCode::SEE_OTHER, "自己承認できるはず");
    assert!(承認行(db, w.id).await[0].self_approved);
}

/// Operatorは承認できないこと。承認はApprover/Administratorの役目（設計書11章）。
async fn 編集者は承認できない(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "operator-approve@example.com", "Operator").await;

    let w = 起票(db, p.id, None, "Repair", None).await;
    承認行を作る(db, w.id, &[p.id]).await;
    let 承認 = 承認行(db, w.id).await;

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, _) = 承認する(状態, &token, p.id, w.id, 承認[0].id, "approve").await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(承認行(db, w.id).await[0].status, "pending");
}

// ---------------------------------------------------------------------------
// 状態遷移（設計書11.2）
// ---------------------------------------------------------------------------

/// **承認前に実行を開始できないこと**（設計書11.2）。
///
/// 画面はボタンを出していないが、POSTは直接叩ける。
async fn 承認前は実行できない(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "premature@example.com", "Operator").await;
    let w = 起票(db, p.id, None, "Repair", None).await;
    承認行を作る(db, w.id, &[p.id]).await;

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, body) = 遷移(状態, &token, p.id, w.id, "execute", "").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("現在の状態からは行えない操作"));
    assert_eq!(再取得(db, w.id).await.status, "planned");
}

/// 承認後に実行・完了できること。**予定と実績を別に残す**（5.1）。
async fn 承認後に実行して完了できる(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "flow@example.com", "Operator").await;
    let 承認者 = 利用者(db, "flow-approver@example.com").await;
    メンバー(db, 承認者.id, p.id, "Approver").await;

    let w = 起票(db, p.id, None, "Repair", None).await;
    承認行を作る(db, w.id, &[p.id]).await;
    let 承認 = 承認行(db, w.id).await;

    let (状態, token) = 認証済み(db, &承認者).await;
    承認する(状態, &token, p.id, w.id, 承認[0].id, "approve").await;

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, _) = 遷移(状態, &token, p.id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let 実行中 = 再取得(db, w.id).await;
    assert_eq!(実行中.status, "executing");
    assert!(実行中.executed_at.is_some());
    assert!(実行中.completed_at.is_none());

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, _) = 遷移(状態, &token, p.id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let 完了 = 再取得(db, w.id).await;
    assert_eq!(完了.status, "completed");
    assert!(完了.completed_at.is_some());
    // **実行開始の時刻が上書きされていないこと**（5.1）
    assert_eq!(完了.executed_at, 実行中.executed_at);
}

/// **中止には理由が要ること**（旧C-2）。
///
/// 理由がないと「結果はどうだったのか」に答えられなくなる。
async fn 中止には理由が要る(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "abort@example.com", "Operator").await;
    let w = 起票(db, p.id, None, "Addition", None).await;
    承認行を作る(db, w.id, &[p.id]).await;

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, body) = 遷移(状態, &token, p.id, w.id, "abort", "  ").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("中止の理由を入力"));
    assert_eq!(再取得(db, w.id).await.status, "planned");

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, _) = 遷移(状態, &token, p.id, w.id, "abort", "調達が間に合わず繰越").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 中止 = 再取得(db, w.id).await;
    assert_eq!(中止.status, "aborted");
    assert_eq!(中止.aborted_reason.as_deref(), Some("調達が間に合わず繰越"));
    // 完了と混ざらないこと。納期遵守率の集計が汚れる（10.4）
    assert!(中止.completed_at.is_none());
}

/// 計画中からも中止できること（設計書11.2）。
async fn 計画中からも中止できる(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "abort-planned@example.com", "Operator").await;
    let w = 起票(db, p.id, None, "Disposal", None).await;
    承認行を作る(db, w.id, &[p.id]).await;

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, _) = 遷移(状態, &token, p.id, w.id, "abort", "対象機器が既に廃棄済み").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(再取得(db, w.id).await.status, "aborted");
}

/// 中止済みのチケットを後から承認できないこと。
async fn 中止済みは承認できない(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "abort-approve@example.com", "Operator").await;
    let 承認者 = 利用者(db, "late-approver@example.com").await;
    メンバー(db, 承認者.id, p.id, "Approver").await;

    let w = 起票(db, p.id, None, "Repair", None).await;
    承認行を作る(db, w.id, &[p.id]).await;
    let 承認 = 承認行(db, w.id).await;

    let (状態, token) = 認証済み(db, &操作者).await;
    遷移(状態, &token, p.id, w.id, "abort", "不要になった").await;

    let (状態, token) = 認証済み(db, &承認者).await;
    let (status, body) = 承認する(状態, &token, p.id, w.id, 承認[0].id, "approve").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("計画中のチケットだけ"));
    assert_eq!(再取得(db, w.id).await.status, "aborted");
}

// ---------------------------------------------------------------------------
// 一覧・詳細
// ---------------------------------------------------------------------------

/// 既定では完了・中止を隠すこと（設計書16.1）。
async fn 既定では終わったものを隠す(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "list@example.com", "Operator").await;
    起票(db, p.id, None, "Repair", None).await;
    let 済 = 起票(db, p.id, None, "Disposal", None).await;
    work_order::ActiveModel {
        id: Set(済.id),
        status: Set("completed".to_owned()),
        title: Set("完了したチケット".to_owned()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &操作者).await;
    let (status, body) = 取得(状態, &format!("/projects/{}/work-orders", p.id), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("完了したチケット"), "完了済みが出ている");

    let (状態, token) = 認証済み(db, &操作者).await;
    let (_, 全件) = 取得(
        状態,
        &format!("/projects/{}/work-orders?status=all", p.id),
        &token,
    )
    .await;
    assert!(全件.contains("完了したチケット"));
}

/// **他プロジェクトのチケットは見えないこと**（設計書3章）。
async fn 他プロジェクトのチケットは見えない(db: &DatabaseConnection) {
    let (よそ者, 自分の) = 準備(db, "outsider@example.com", "Operator").await;
    let 他人の = プロジェクト(db, "他人のプロジェクト").await;
    let w = 起票(db, 他人の.id, None, "Repair", None).await;

    let (状態, token) = 認証済み(db, &よそ者).await;
    let (status, _) = 取得(
        状態,
        &format!("/projects/{}/work-orders/{}", 他人の.id, w.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 自分のプロジェクトのURLに他人のチケットIDを混ぜても引けない
    let (状態, token) = 認証済み(db, &よそ者).await;
    let (status, _) = 取得(
        状態,
        &format!("/projects/{}/work-orders/{}", 自分の.id, w.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 移譲先のメンバーからもチケットが見えること（設計書11.5）。
///
/// 見えないと、自分のプロジェクトへ来る予定の機器が分からず承認もできない。
async fn 移譲先からもチケットが見える(db: &DatabaseConnection) {
    let (_, 移譲元) = 準備(db, "src@example.com", "Operator").await;
    let 移譲先 = プロジェクト(db, "受入先").await;
    let 受入担当 = 利用者(db, "dest@example.com").await;
    メンバー(db, 受入担当.id, 移譲先.id, "Approver").await;

    let w = 起票(db, 移譲元.id, Some(移譲先.id), "Transfer", None).await;
    承認行を作る(db, w.id, &[移譲元.id, 移譲先.id]).await;

    let (状態, token) = 認証済み(db, &受入担当).await;
    let (status, body) = 取得(
        状態,
        &format!("/projects/{}/work-orders/{}", 移譲先.id, w.id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // 移譲先の承認者として、承認ボタンが出ること
    assert!(body.contains("承認する"), "移譲先から承認できない");
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 承認する(
    state: AppState,
    token: &str,
    project_id: i32,
    work_order_id: i32,
    approval_id: i32,
    decision: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/projects/{project_id}/work-orders/{work_order_id}/approve"),
        token,
        &[
            ("approval_id", &approval_id.to_string()),
            ("decision", decision),
        ],
    )
    .await
}

async fn 遷移(
    state: AppState,
    token: &str,
    project_id: i32,
    work_order_id: i32,
    to: &str,
    reason: &str,
) -> (StatusCode, String) {
    送信(
        state,
        &format!("/projects/{project_id}/work-orders/{work_order_id}/transition"),
        token,
        &[("to", to), ("aborted_reason", reason)],
    )
    .await
}

async fn 起票(
    db: &DatabaseConnection,
    project_id: i32,
    target_project_id: Option<i32>,
    work_type: &str,
    primary: Option<i32>,
) -> work_order::Model {
    work_order::ActiveModel {
        project_id: Set(project_id),
        target_project_id: Set(target_project_id),
        work_type: Set(work_type.to_owned()),
        title: Set(format!("{work_type}のチケット")),
        description: Set(String::new()),
        primary_assignee_id: Set(primary),
        status: Set("planned".to_owned()),
        planned_at: Set(Some(Utc::now())),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

async fn 承認行を作る(db: &DatabaseConnection, work_order_id: i32, projects: &[i32]) {
    for pid in projects {
        work_order_approval::ActiveModel {
            work_order_id: Set(work_order_id),
            required_project_id: Set(*pid),
            approver_id: Set(None),
            status: Set("pending".to_owned()),
            approved_at: Set(None),
            self_approved: Set(false),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }
}

async fn 承認行(db: &DatabaseConnection, work_order_id: i32) -> Vec<work_order_approval::Model> {
    work_order_approval::Entity::find()
        .filter(work_order_approval::Column::WorkOrderId.eq(work_order_id))
        .order_by_asc(work_order_approval::Column::Id)
        .all(db)
        .await
        .unwrap()
}

async fn 再取得(db: &DatabaseConnection, id: i32) -> work_order::Model {
    work_order::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

async fn 唯一のチケット(db: &DatabaseConnection) -> work_order::Model {
    let all = work_order::Entity::find().all(db).await.unwrap();
    assert_eq!(all.len(), 1);
    all.into_iter().next().unwrap()
}

async fn チケット数(db: &DatabaseConnection) -> usize {
    work_order::Entity::find().all(db).await.unwrap().len()
}

async fn 準備(
    db: &DatabaseConnection,
    email: &str,
    role: &str,
) -> (app_user::Model, project::Model) {
    let user = 利用者(db, email).await;
    let p = プロジェクト(db, &format!("{role}のプロジェクト")).await;
    メンバー(db, user.id, p.id, role).await;
    (user, p)
}

async fn 認証済み(db: &DatabaseConnection, user: &app_user::Model) -> (AppState, String) {
    let config = 設定();
    let (setup, _) = SetupState::initialize(db).await.unwrap();
    let state = AppState {
        passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
        setup,
        config: Arc::new(config),
        db: db.clone(),
        staged: Default::default(),
    };
    let (_, token) = session::create(
        db,
        user.id,
        "127.0.0.1",
        "test",
        &state.config.session,
        Utc::now(),
    )
    .await
    .unwrap();
    (state, token.as_str().to_owned())
}

fn 設定() -> Config {
    let mut config = Config::default();
    config.password.memory_kib = 8;
    config.password.iterations = 1;
    config
}

fn cookie_header(token: &str) -> String {
    format!("{}={token}", session::COOKIE_NAME)
}

async fn 取得(state: AppState, uri: &str, token: &str) -> (StatusCode, String) {
    let res = router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie_header(token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn 送信(
    state: AppState,
    uri: &str,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, String) {
    let csrf = dioryga::auth::csrf::derive(token);
    let mut pairs: Vec<(String, String)> = vec![(dioryga::auth::csrf::FIELD_NAME.to_owned(), csrf)];
    for (key, value) in fields {
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }
    let body = serde_urlencoded::to_string(&pairs).unwrap();

    let res = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie_header(token))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    分解(res).await
}

async fn 分解(res: Response<Body>) -> (StatusCode, String) {
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn 利用者(db: &DatabaseConnection, email: &str) -> app_user::Model {
    app_user::ActiveModel {
        name: Set(email.to_owned()),
        email: Set(email.to_owned()),
        password_hash: Set("$argon2id$dummy".to_owned()),
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
    .await
    .unwrap()
}

async fn プロジェクト(db: &DatabaseConnection, name: &str) -> project::Model {
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
    .insert(db)
    .await
    .unwrap()
}

async fn メンバー(db: &DatabaseConnection, user_id: i32, project_id: i32, role: &str) {
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
    .unwrap();
}

macro_rules! 全検証 {
    ($用意:path, $属性:meta) => {
        全検証!(@one $用意, $属性, 起票すると承認待ちになる);
        全検証!(@one $用意, $属性, 移譲では承認が二件になる);
        全検証!(@one $用意, $属性, 移譲先はtransferでしか指定できない);
        全検証!(@one $用意, $属性, 語彙外の種別は拒否される);
        全検証!(@one $用意, $属性, 承認者は起票できない);
        全検証!(@one $用意, $属性, 全部揃うまで承認済みにならない);
        全検証!(@one $用意, $属性, 却下があると進まない);
        全検証!(@one $用意, $属性, 担当者は自分で承認できない);
        全検証!(@one $用意, $属性, 他に承認者がいなければ自己承認できる);
        全検証!(@one $用意, $属性, 無効化された承認者は数えない);
        全検証!(@one $用意, $属性, 編集者は承認できない);
        全検証!(@one $用意, $属性, 承認前は実行できない);
        全検証!(@one $用意, $属性, 承認後に実行して完了できる);
        全検証!(@one $用意, $属性, 中止には理由が要る);
        全検証!(@one $用意, $属性, 計画中からも中止できる);
        全検証!(@one $用意, $属性, 中止済みは承認できない);
        全検証!(@one $用意, $属性, 既定では終わったものを隠す);
        全検証!(@one $用意, $属性, 他プロジェクトのチケットは見えない);
        全検証!(@one $用意, $属性, 移譲先からもチケットが見える);
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
