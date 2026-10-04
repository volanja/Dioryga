//! 変更管理チケット画面の結合テスト（設計書11章）。
//!
//! 16.7で「v1で最も重い画面」とした箇所。**画面が出るかではなく、ワークフローの
//! 規約が守られているか**を確かめる。とくに次の3つは、破れても画面上は正常に
//! 見えてしまうため、テストでしか守れない。
//!
//! - 承認が1件でも欠けているうちは `approved` にならない（11.4-7）
//! - 自分が担当のチケットを自分で承認できない（11.4-9）
//! - ただし承認できる他のメンバーがいなければ許され、記録が残る（22章R-2）

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use chrono::Utc;
use dioryga::auth::password::PasswordService;
use dioryga::auth::session;
use dioryga::auth::setup::SetupState;
use dioryga::config::Config;
use dioryga::server::{router, AppState};
use entity::{
    app_user, audit_log, cable_catalog, cable_connection, cable_end_slot, cable_instance, device,
    device_assignment, device_mount, ip_address, mount_container, os_interface, part_catalog,
    part_instance, part_instance_location, part_port_slot, project, project_member, vendor,
    work_order, work_order_approval,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
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

/// **期日は見えている形（`2026/09/23`）で入れても通ること。**
///
/// `<input type="date">` は送信時には `2026-09-23` を送るが、画面に見えているのは
/// `2026/09/23` であり、日付入力に対応していないブラウザでは利用者がその形で打つ。
async fn 期日は斜線区切りでも通る(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "slash@example.com", "Operator").await;
    let (状態, token) = 認証済み(db, &user).await;

    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/work-orders", p.id),
        &token,
        &[
            ("work_type", "Repair"),
            ("title", "電源ユニット交換"),
            ("due_date", "2026/09/23"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "拒否されました: {body}");

    let w = 唯一のチケット(db).await;
    assert_eq!(w.due_date, chrono::NaiveDate::from_ymd_opt(2026, 9, 23));
}

/// **期日は任意。**空なら未設定で通り、読めなければ黙って未設定に落とさず
/// 拒否すること（Q-21）。
async fn 期日は空なら未設定で読めなければ拒否する(db: &DatabaseConnection) {
    let (user, p) = 準備(db, "due@example.com", "Operator").await;

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/work-orders", p.id),
        &token,
        &[
            ("work_type", "Repair"),
            ("title", "読めない期日"),
            ("due_date", "2026.09.23"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("期日を読めません"),
        "読めないことが伝わっていません"
    );
    assert_eq!(チケット数(db).await, 0);

    let (状態, token) = 認証済み(db, &user).await;
    let (status, body) = 送信(
        状態,
        &format!("/projects/{}/work-orders", p.id),
        &token,
        &[
            ("work_type", "Repair"),
            ("title", "期日なし"),
            ("due_date", ""),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "拒否されました: {body}");
    assert!(唯一のチケット(db).await.due_date.is_none());
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
    assert_eq!(実行中.status, "in_progress");
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
    assert_eq!(中止.status, "cancelled");
    assert_eq!(
        中止.cancelled_reason.as_deref(),
        Some("調達が間に合わず繰越")
    );
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
    assert_eq!(再取得(db, w.id).await.status, "cancelled");
}

/// 中止済みのチケットに承認ボタンを出さないこと。
///
/// 押しても弾かれるので、**出ていること自体が誤解を招く。**
async fn 中止済みには承認ボタンを出さない(db: &DatabaseConnection) {
    let (操作者, p) = 準備(db, "no-button@example.com", "Operator").await;
    let 承認者 = 利用者(db, "no-button-approver@example.com").await;
    メンバー(db, 承認者.id, p.id, "Approver").await;

    let w = 起票(db, p.id, None, "Repair", None).await;
    承認行を作る(db, w.id, &[p.id]).await;

    // 計画中なら出る
    let (状態, token) = 認証済み(db, &承認者).await;
    let (_, 計画中) = 取得(
        状態,
        &format!("/projects/{}/work-orders/{}", p.id, w.id),
        &token,
    )
    .await;
    // 見出しの「承認するプロジェクト」にも当たるため、ボタンそのものを見る
    assert!(計画中.contains(r#"name="decision" value="approve""#));

    let (状態, token) = 認証済み(db, &操作者).await;
    遷移(状態, &token, p.id, w.id, "abort", "不要になった").await;

    let (状態, token) = 認証済み(db, &承認者).await;
    let (_, 中止後) = 取得(
        状態,
        &format!("/projects/{}/work-orders/{}", p.id, w.id),
        &token,
    )
    .await;
    assert!(
        !中止後.contains(r#"name="decision""#),
        "中止済みなのにボタンが出ている"
    );
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
    assert_eq!(再取得(db, w.id).await.status, "cancelled");
}

// ---------------------------------------------------------------------------
// 予約（設計書11.6）
// ---------------------------------------------------------------------------

/// **計画の時点でラック位置を確保できること**（設計書11.6）。
///
/// Executeまで待たない。こうするとラック図がそのまま予約状況の図になる。
async fn 計画時点でラック位置を予約できる(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "reserve@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 予約(状態, &token, &場, &[("position", "10")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let 行 = 現行の搭載(db, 場.device_id).await.unwrap();
    assert_eq!(行.position, Some(10));
    // **予約であることが行に残る。**誰の計画で確保されたかを辿れる
    assert_eq!(行.work_order_id, Some(場.work_order_id));
}

/// **予約は12.3の重複配置検証にそのまま引っかかること**（設計書11.6）。
///
/// 二重予約の防止に新しい仕組みを足していない。
async fn 二重予約は既存の検証で止まる(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "double@example.com").await;

    // 先に別の機器が同じ場所を使っている
    let 先客 = 予約対象の機器(db, &場, "existing-01", "running").await;
    device_mount::ActiveModel {
        device_id: Set(先客),
        container_id: Set(Some(場.container_id)),
        position: Set(Some(10)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 予約(状態, &token, &場, &[("position", "10")]).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("既に機器があります"));
    assert!(現行の搭載(db, 場.device_id).await.is_none());
}

/// **中断すると予約が解放されること**（設計書11.6）。
///
/// 閉じ忘れるとラックが埋まったまま残り、予約という仕組みが信用されなくなる。
async fn 中断すると予約が解放される(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "release@example.com").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    予約(状態, &token, &場, &[("position", "10")]).await;
    assert!(現行の搭載(db, 場.device_id).await.is_some());

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 遷移(
        状態,
        &token,
        場.project_id,
        場.work_order_id,
        "abort",
        "調達が間に合わず繰越",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    assert!(
        現行の搭載(db, 場.device_id).await.is_none(),
        "予約が解放されていない"
    );
    // **行は消さず閉じる**（不変条件1）
    let 全部 = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(場.device_id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(全部.len(), 1, "行が消えている");
    assert!(全部[0].to_date.is_some());
}

/// **実行を開始すると予約が実機になること**（設計書16.2のフロー）。
async fn 実行すると予約が実機になる(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "execute@example.com").await;
    let 承認者 = 利用者(db, "exec-approver@example.com").await;
    メンバー(db, 承認者.id, 場.project_id, "Approver").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    予約(状態, &token, &場, &[("position", "10")]).await;

    let 承認 = 承認行(db, 場.work_order_id).await;
    let (状態, token) = 認証済み(db, &承認者).await;
    承認する(
        状態,
        &token,
        場.project_id,
        場.work_order_id,
        承認[0].id,
        "approve",
    )
    .await;

    assert_eq!(
        機器の状態(db, 場.device_id).await,
        "planned",
        "まだ予約中のはず"
    );

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 遷移(状態, &token, 場.project_id, 場.work_order_id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    assert_eq!(機器の状態(db, 場.device_id).await, "running");
    // 搭載はそのまま残る。実機になっただけ
    assert!(現行の搭載(db, 場.device_id).await.is_some());
}

/// **予約中でない機器の状態を勝手に書き換えないこと**。
///
/// 実行で `standby` が `running` に変わってはいけない。**`health` も動かさない**
/// ——実行は「直ったこと」を意味しない（設計書6.3）。
async fn 予約でない機器の状態は変えない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "keepstatus@example.com").await;
    let 承認者 = 利用者(db, "keep-approver@example.com").await;
    メンバー(db, 承認者.id, 場.project_id, "Approver").await;

    // 対象機器を、待機中で故障しているものにしておく
    device::ActiveModel {
        id: Set(場.device_id),
        status: Set("standby".to_owned()),
        health: Set("failed".to_owned()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let 承認 = 承認行(db, 場.work_order_id).await;
    let (状態, token) = 認証済み(db, &承認者).await;
    承認する(
        状態,
        &token,
        場.project_id,
        場.work_order_id,
        承認[0].id,
        "approve",
    )
    .await;

    let (状態, token) = 認証済み(db, &場.user).await;
    遷移(状態, &token, 場.project_id, 場.work_order_id, "execute", "").await;

    assert_eq!(機器の状態(db, 場.device_id).await, "standby");
    assert_eq!(機器の故障(db, 場.device_id).await, "failed");
}

/// **移譲の実行では、予約中から稼働中へ進めないこと**（設計書6.3）。
///
/// 進めるのは増設の実行だけ。倉庫からの払い出しで予約中にした機器が、
/// 同じ実行の中で稼働中になってしまう。
async fn 移譲の実行では予約中を進めない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "xfer-planned@example.com").await;
    let 移譲先 = プロジェクト(db, "予約中の受入先").await;
    let device_id = 予約対象の機器(db, &場, "xfer-planned-srv", "planned").await;
    let w = 移譲のチケット(db, 場.project_id, 移譲先.id, Some(device_id)).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    assert_eq!(現在の所属(db, device_id).await, Some(移譲先.id));
    assert_eq!(機器の状態(db, device_id).await, "planned");
}

/// **倉庫からの払い出しで、機器を予約中にすること**（#221、設計書6.3）。
///
/// 倉庫にある間の `status` には意味が無い。受け取る側が先へ進める。倉庫への
/// 入庫では `status` を動かさない。
async fn 倉庫から払い出すと予約中になる(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "payout@example.com").await;
    crate::support::倉庫プロジェクトにする(db, 場.project_id).await;
    let 受入先 = プロジェクト(db, "払い出し先").await;
    let device_id = 予約対象の機器(db, &場, "payout-srv", "running").await;
    故障させる(db, device_id).await;
    let w = 移譲のチケット(db, 場.project_id, 受入先.id, Some(device_id)).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    assert_eq!(現在の所属(db, device_id).await, Some(受入先.id));
    assert_eq!(機器の状態(db, device_id).await, "planned");
    // 故障の有無は動かさない
    assert_eq!(機器の故障(db, device_id).await, "failed");
}

/// **倉庫への入庫では、状態を動かさないこと**（#221、設計書6.3）。
async fn 倉庫へ入れても状態は変えない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "stock-in@example.com").await;
    let 倉庫 = プロジェクト(db, "入庫先の倉庫").await;
    crate::support::倉庫プロジェクトにする(db, 倉庫.id).await;
    let device_id = 予約対象の機器(db, &場, "stock-in-srv", "running").await;
    let w = 移譲のチケット(db, 場.project_id, 倉庫.id, Some(device_id)).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    assert_eq!(現在の所属(db, device_id).await, Some(倉庫.id));
    assert_eq!(機器の状態(db, device_id).await, "running");
}

// ---------------------------------------------------------------------------
// 修理と故障（#221、設計書6.3）
// ---------------------------------------------------------------------------

/// **修理の正常完了で機器の故障が戻り、中止では戻らないこと**（設計書6.3）。
///
/// 直らなかったときは中止にし、理由を書く。完了を「作業が終わった」の意味で
/// 使うと、直らなかったのに正常に戻る。
async fn 修理の完了で機器の故障を戻す(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "repair-device@example.com").await;
    let 直る = 予約対象の機器(db, &場, "repair-ok", "running").await;
    let 直らない = 予約対象の機器(db, &場, "repair-ng", "running").await;
    for id in [直る, 直らない] {
        故障させる(db, id).await;
    }

    let w = 修理のチケット(db, 場.project_id, Some(直る), None, "approved").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    // 実行では戻さない
    assert_eq!(機器の故障(db, 直る).await, "failed");
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(機器の故障(db, 直る).await, "ok");
    // status は動かさない
    assert_eq!(機器の状態(db, 直る).await, "running");

    let w = 修理のチケット(db, 場.project_id, Some(直らない), None, "in_progress").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "abort", "部材が届かない").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(機器の故障(db, 直らない).await, "failed");
}

/// **部品が対象の修理は、部品の故障だけを戻すこと**（設計書6.3）。
///
/// 機器と部品の `health` は独立に持つ。載せ先の機器を添えていても、機器の値は
/// 書き換えない。
async fn 部品の修理は部品の故障だけを戻す(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "repair-part@example.com").await;
    let device_id = 予約対象の機器(db, &場, "repair-part-host", "running").await;
    故障させる(db, device_id).await;
    let psu = 部品(db, 場.user.id, "PSU-RPR").await;
    part_instance_location::ActiveModel {
        part_instance_id: Set(psu.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(device_id)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let mut active: part_instance::ActiveModel = psu.clone().into();
    active.health = Set("failed".to_owned());
    active.update(db).await.unwrap();

    let w = 修理のチケット(
        db,
        場.project_id,
        Some(device_id),
        Some(psu.id),
        "in_progress",
    )
    .await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let 後 = part_instance::Entity::find_by_id(psu.id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(後.health, "ok");
    assert_eq!(
        機器の故障(db, device_id).await,
        "failed",
        "機器の値を書き換えています"
    );
}

/// **修理の起票で部品を対象にできること**（設計書6.3）。
///
/// 部品を対象にできるのは修理と廃棄だけ。対象は今このプロジェクトにある部品。
async fn 修理の起票で部品を指定できる(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "repair-create@example.com").await;
    let device_id = 予約対象の機器(db, &場, "repair-create-host", "running").await;
    let psu = 部品(db, 場.user.id, "PSU-NEW").await;
    part_instance_location::ActiveModel {
        part_instance_id: Set(psu.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(device_id)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let よその = 部品(db, 場.user.id, "PSU-ELSEWHERE").await;
    let psu_id = psu.id.to_string();
    let よその_id = よその.id.to_string();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, form) = 取得(
        状態,
        &format!("/projects/{}/work-orders/new", 場.project_id),
        &token,
    )
    .await;
    // 他の選択肢と id が重なるので、表示名で確かめる
    assert!(form.contains("PSU-NEW（SN-PSU-NEW）"), "{form}");
    assert!(!form.contains("PSU-ELSEWHERE"), "{form}");

    for (種類, 部品id, 期待) in [
        (
            "Addition",
            psu_id.as_str(),
            "部品を対象にできるのは修理と廃棄だけです",
        ),
        (
            "Repair",
            よその_id.as_str(),
            "このプロジェクトにある部品を指定してください",
        ),
    ] {
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = 送信(
            状態,
            &format!("/projects/{}/work-orders", 場.project_id),
            &token,
            &[
                ("work_type", 種類),
                ("title", "部品の修理"),
                ("part_instance_id", 部品id),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{種類}");
        assert!(body.contains(期待), "{種類}: {body}");
    }

    let 前 = チケット数(db).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 送信(
        状態,
        &format!("/projects/{}/work-orders", 場.project_id),
        &token,
        &[
            ("work_type", "Repair"),
            ("title", "電源ユニット交換"),
            ("device_id", &device_id.to_string()),
            ("part_instance_id", &psu_id),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(チケット数(db).await, 前 + 1);
    let w = work_order::Entity::find()
        .order_by_desc(work_order::Column::Id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.part_instance_id, Some(psu.id));
    assert_eq!(w.device_id, Some(device_id));
}

/// **承認後は予約を作れないこと**（設計書11.6）。
///
/// 承認した内容と実施する内容がずれる。
async fn 承認後は予約を作れない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "afterapprove@example.com").await;
    work_order::ActiveModel {
        id: Set(場.work_order_id),
        status: Set("approved".to_owned()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 予約(状態, &token, &場, &[("position", "10")]).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("計画中のチケットだけ"));
    assert!(現行の搭載(db, 場.device_id).await.is_none());
}

/// **移設では予約を作れないこと**（設計書11.6の末尾、17章の課題）。
///
/// 稼働中の機器は移設元で動いたままである必要があり、同じ方式を当てられない。
async fn 移設では予約を作れない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "relocation@example.com").await;
    work_order::ActiveModel {
        id: Set(場.work_order_id),
        work_type: Set("Relocation".to_owned()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 予約(状態, &token, &場, &[("position", "10")]).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("増設（Addition）のチケットだけ"));
    assert!(現行の搭載(db, 場.device_id).await.is_none());

    // 画面にも理由が出ること
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, 画面) = 取得(
        状態,
        &format!(
            "/projects/{}/work-orders/{}",
            場.project_id, 場.work_order_id
        ),
        &token,
    )
    .await;
    assert!(画面.contains("移設での予約は未対応"));
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
    // 見出しの「承認するプロジェクト」にも当たるため、ボタンそのものを見る
    assert!(
        body.contains(r#"name="decision" value="approve""#),
        "移譲先から承認できない"
    );
}

/// **移譲を実行すると、機器の所属が移譲先へ移ること**（11.5、#215）。
///
/// 閉じて開く（不変条件1）。開く行に、移譲のチケットが残る。
async fn 移譲を実行すると所属が移る(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "xfer-move@example.com").await;
    let 移譲先 = プロジェクト(db, "移譲の受入先").await;
    let device_id = 予約対象の機器(db, &場, "xfer-srv", "running").await;
    let w = 移譲のチケット(db, 場.project_id, 移譲先.id, Some(device_id)).await;

    // 実行の前は動かない
    assert_eq!(現在の所属(db, device_id).await, Some(場.project_id));

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let 履歴 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .order_by_asc(device_assignment::Column::Id)
        .all(db)
        .await
        .unwrap();
    assert_eq!(履歴.len(), 2, "閉じて開いていません: {履歴:?}");
    assert_eq!(履歴[0].location_id, Some(場.project_id));
    assert!(履歴[0].to_date.is_some(), "移譲元の行が閉じていません");
    // 閉じる行の work_order_id は、その行を作ったチケットの記録なので書き換えない
    assert_eq!(履歴[0].work_order_id, None);
    assert_eq!(履歴[1].location_type, "Project");
    assert_eq!(履歴[1].location_id, Some(移譲先.id));
    assert_eq!(履歴[1].work_order_id, Some(w.id));
    assert_eq!(履歴[1].to_date, None);
    assert_eq!(
        履歴[0].to_date,
        Some(履歴[1].from_date),
        "履歴に隙間があります"
    );

    // 完了では、もう動かない
    let (状態, token) = 認証済み(db, &場.user).await;
    遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(現在の所属(db, device_id).await, Some(移譲先.id));
    let 行数 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .count(db)
        .await
        .unwrap();
    assert_eq!(行数, 2);
}

/// **外すものが残っている間は、移譲を実行させないこと**（11.5、#215）。
///
/// 設備・什器もサブネットもプロジェクトのものである。付けたまま移すと、
/// 移譲元のラック図に他のプロジェクトの機器が残る。黙って閉じることもしない。
async fn 外すものが残る移譲は実行できない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "xfer-block@example.com").await;
    let 移譲先 = プロジェクト(db, "移譲の受入先2").await;

    // 搭載している
    let 搭載中 = 予約対象の機器(db, &場, "xfer-mounted", "running").await;
    device_mount::ActiveModel {
        device_id: Set(搭載中),
        container_id: Set(Some(場.container_id)),
        position: Set(Some(10)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // 上に機器を載せている（棚板・仮想マシンのホスト）
    let 載せている = 予約対象の機器(db, &場, "xfer-host", "running").await;
    let 上の機器 = 予約対象の機器(db, &場, "xfer-guest", "running").await;
    device_mount::ActiveModel {
        device_id: Set(上の機器),
        host_device_id: Set(Some(載せている)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // IPアドレスを持っている
    let アドレス持ち = 予約対象の機器(db, &場, "xfer-ip", "running").await;
    let nic = os_interface::ActiveModel {
        device_id: Set(アドレス持ち),
        interface_type: Set("Virtual".to_owned()),
        os_interface_name: Set("lo1".to_owned()),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    ip_address::ActiveModel {
        os_interface_id: Set(nic.id),
        ip_address: Set("192.0.2.10".to_owned()),
        prefix_length: Set(24),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    for (device_id, 期待) in [
        (Some(搭載中), "搭載を外してから"),
        (Some(載せている), "載っている機器があります"),
        (Some(アドレス持ち), "IPアドレスを閉じてから"),
        // 機器を指定していない移譲
        (None, "機器が指定されていません"),
    ] {
        let w = 移譲のチケット(db, 場.project_id, 移譲先.id, device_id).await;
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
        assert_eq!(status, StatusCode::OK, "{期待}");
        assert!(body.contains(期待), "{期待}: {body}");
        // **チケットも所属も動いていないこと**
        assert_eq!(再取得(db, w.id).await.status, "approved", "{期待}");
        if let Some(id) = device_id {
            assert_eq!(現在の所属(db, id).await, Some(場.project_id), "{期待}");
        }
    }

    // **閉じた行は妨げにならない。**外したあとは実行できる
    let 搭載 = 現行の搭載(db, 搭載中).await.unwrap();
    let mut active: device_mount::ActiveModel = 搭載.into();
    active.to_date = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
    let w = 移譲のチケット(db, 場.project_id, 移譲先.id, Some(搭載中)).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(現在の所属(db, 搭載中).await, Some(移譲先.id));
}

/// **起票元にない機器は移せないこと**（#215）。
///
/// 起票の後に別のチケットで動いていれば、承認された前提がもう成り立たない。
async fn 起票元にない機器は移譲できない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "xfer-gone@example.com").await;
    let 移譲先 = プロジェクト(db, "移譲の受入先3").await;
    let device_id = 予約対象の機器(db, &場, "xfer-twice", "running").await;

    let 一つ目 = 移譲のチケット(db, 場.project_id, 移譲先.id, Some(device_id)).await;
    let 二つ目 = 移譲のチケット(db, 場.project_id, 移譲先.id, Some(device_id)).await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, _) = 遷移(状態, &token, 場.project_id, 一つ目.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, 二つ目.id, "execute", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("起票元のプロジェクトにありません"), "{body}");
    assert_eq!(再取得(db, 二つ目.id).await.status, "approved");
}

/// **廃棄を完了すると、機器の所在が `Disposed` になること**（11.4、#216）。
///
/// 閉じて開く（不変条件1）。開く行に廃棄のチケットが残り、機器は現役の一覧から消える。
/// **実行では動かさない。**作業の途中では、まだ現役の機器である。
async fn 廃棄を完了すると所在がdisposedになる(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "dispose-move@example.com").await;
    let device_id = 予約対象の機器(db, &場, "dispose-srv", "running").await;
    let w = 廃棄のチケット(db, 場.project_id, Some(device_id), None, "approved").await;

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        現在の所属(db, device_id).await,
        Some(場.project_id),
        "実行で動いています"
    );

    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(再取得(db, w.id).await.status, "completed");

    let 履歴 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .order_by_asc(device_assignment::Column::Id)
        .all(db)
        .await
        .unwrap();
    assert_eq!(履歴.len(), 2, "閉じて開いていません: {履歴:?}");
    assert_eq!(履歴[0].location_id, Some(場.project_id));
    assert_eq!(履歴[0].work_order_id, None);
    assert_eq!(
        履歴[0].to_date,
        Some(履歴[1].from_date),
        "履歴に隙間があります"
    );
    assert_eq!(履歴[1].location_type, "Disposed");
    assert_eq!(履歴[1].location_id, None, "Disposed は参照先を持たない");
    assert_eq!(履歴[1].work_order_id, Some(w.id));
    assert_eq!(履歴[1].to_date, None);

    // 現役の一覧から消え、すべての一覧には廃棄として残る
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, 現役) = 取得(
        状態,
        &format!("/projects/{}/devices", 場.project_id),
        &token,
    )
    .await;
    assert!(!現役.contains("dispose-srv"), "現役の一覧に残っています");
    let (状態, token) = 認証済み(db, &場.user).await;
    let (_, すべて) = 取得(
        状態,
        &format!("/projects/{}/devices?scope=all", 場.project_id),
        &token,
    )
    .await;
    assert!(
        すべて.contains("dispose-srv"),
        "すべての一覧から消えています"
    );
}

/// **外すものが残っている間は、廃棄を完了させないこと**（11.4、#216）。
///
/// 移譲（#215）と同じ扱い。付けたまま廃棄すると、廃棄した機器がラック図と
/// アドレスの一覧に残る。黙って閉じることもしない。
async fn 外すものが残る廃棄は完了できない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "dispose-block@example.com").await;

    let 搭載中 = 予約対象の機器(db, &場, "dispose-mounted", "running").await;
    device_mount::ActiveModel {
        device_id: Set(搭載中),
        container_id: Set(Some(場.container_id)),
        position: Set(Some(12)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let 載せている = 予約対象の機器(db, &場, "dispose-host", "running").await;
    let 上の機器 = 予約対象の機器(db, &場, "dispose-guest", "running").await;
    device_mount::ActiveModel {
        device_id: Set(上の機器),
        host_device_id: Set(Some(載せている)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let アドレス持ち = 予約対象の機器(db, &場, "dispose-ip", "running").await;
    let nic = os_interface::ActiveModel {
        device_id: Set(アドレス持ち),
        interface_type: Set("Virtual".to_owned()),
        os_interface_name: Set("lo1".to_owned()),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    ip_address::ActiveModel {
        os_interface_id: Set(nic.id),
        ip_address: Set("192.0.2.20".to_owned()),
        prefix_length: Set(24),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    // 別のプロジェクトにある機器
    let よそ = プロジェクト(db, "廃棄のよそ").await;
    let よその機器 = 予約対象の機器(db, &場, "dispose-elsewhere", "running").await;
    let 所属 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(よその機器))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut active: device_assignment::ActiveModel = 所属.into();
    active.location_id = Set(Some(よそ.id));
    active.update(db).await.unwrap();

    for (device_id, 期待) in [
        (Some(搭載中), "搭載を外してから完了"),
        (Some(載せている), "載っている機器があります"),
        (Some(アドレス持ち), "IPアドレスを閉じてから完了"),
        (Some(よその機器), "起票元のプロジェクトにありません"),
        (None, "廃棄する機器が指定されていません"),
    ] {
        let w = 廃棄のチケット(db, 場.project_id, device_id, None, "in_progress").await;
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
        assert_eq!(status, StatusCode::OK, "{期待}");
        assert!(body.contains(期待), "{期待}: {body}");
        // **チケットも所在も動いていないこと**
        assert_eq!(再取得(db, w.id).await.status, "in_progress", "{期待}");
        if let Some(id) = device_id {
            let 種別 = device_assignment::Entity::find()
                .filter(device_assignment::Column::DeviceId.eq(id))
                .filter(device_assignment::Column::ToDate.is_null())
                .one(db)
                .await
                .unwrap()
                .unwrap()
                .location_type;
            assert_eq!(種別, "Project", "{期待}");
        }
    }

    // **閉じた行は妨げにならない。**外したあとは完了できる
    let 搭載 = 現行の搭載(db, 搭載中).await.unwrap();
    let mut active: device_mount::ActiveModel = 搭載.into();
    active.to_date = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
    let w = 廃棄のチケット(db, 場.project_id, Some(搭載中), None, "in_progress").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    // 廃棄した機器をもう一度廃棄しようとしても完了できない
    let w = 廃棄のチケット(db, 場.project_id, Some(搭載中), None, "in_progress").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("起票元のプロジェクトにありません"), "{body}");
}

/// **ケーブルが挿さった機器・部品は、移譲も廃棄もできないこと**（#225）。
///
/// ケーブルは機器について移らない。挿さったまま動かすと、つながった先が別の
/// プロジェクトの機器になる。自動では外さず、利用者に外させる。
async fn ケーブルが挿さっていると動かせない(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "cable-block@example.com").await;
    let 移譲先 = プロジェクト(db, "ケーブルの受入先").await;
    let device_id = 予約対象の機器(db, &場, "cable-held", "running").await;
    let nic = 部品(db, 場.user.id, "NIC-CABLE-1").await;
    part_instance_location::ActiveModel {
        part_instance_id: Set(nic.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(device_id)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let 接続 = ケーブルを挿す(db, 場.user.id, &nic).await;

    // 移譲の実行
    let w = 移譲のチケット(db, 場.project_id, 移譲先.id, Some(device_id)).await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ケーブルが挿さっています"), "{body}");
    assert_eq!(現在の所属(db, device_id).await, Some(場.project_id));

    // 機器の廃棄の完了
    let 廃棄 = 廃棄のチケット(db, 場.project_id, Some(device_id), None, "in_progress").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, 廃棄.id, "complete", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ケーブルが挿さっています"), "{body}");
    assert_eq!(再取得(db, 廃棄.id).await.status, "in_progress");

    // 部品の廃棄の完了
    let 部品の廃棄 = 廃棄のチケット(
        db,
        場.project_id,
        Some(device_id),
        Some(nic.id),
        "in_progress",
    )
    .await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, 部品の廃棄.id, "complete", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("部品にケーブルが挿さっています"), "{body}");
    assert_eq!(再取得(db, 部品の廃棄.id).await.status, "in_progress");

    // **外したあとは妨げにならない。**
    let mut active: cable_connection::ActiveModel = 接続.into();
    active.to_date = Set(Some(Utc::now()));
    active.update(db).await.unwrap();
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "execute", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(現在の所属(db, device_id).await, Some(移譲先.id));
}

/// 部品の型にポートを1つ足し、新しいケーブルの端を挿す。
async fn ケーブルを挿す(
    db: &DatabaseConnection,
    user_id: i32,
    part: &part_instance::Model,
) -> cable_connection::Model {
    let slot = part_port_slot::ActiveModel {
        part_catalog_id: Set(part.part_catalog_id),
        port_kind: Set("Network".to_owned()),
        port_label: Set("Port1".to_owned()),
        connector_type: Set("LC".to_owned()),
        port_speed: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let c = cable_catalog::ActiveModel {
        cable_kind: Set(Some("Network".to_owned())),
        cable_type: Set("OM4".to_owned()),
        length_mm: Set(Some(2000)),
        color: Set(String::new()),
        vendor_id: Set(None),
        part_number: Set(None),
        rated_voltage: Set(None),
        rated_current_ma: Set(None),
        retired_at: Set(None),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let end = cable_end_slot::ActiveModel {
        cable_catalog_id: Set(c.id),
        end_label: Set("A".to_owned()),
        connector_type: Set("LC".to_owned()),
        port_speed: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let k = cable_instance::ActiveModel {
        cable_catalog_id: Set(c.id),
        serial_number: Set(None),
        asset_number: Set(None),
        retired_at: Set(None),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    cable_connection::ActiveModel {
        cable_instance_id: Set(k.id),
        cable_end_slot_id: Set(end.id),
        part_instance_id: Set(part.id),
        port_slot_id: Set(slot.id),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// **部品を指す廃棄は、部品の所在だけを `Disposed` にすること**（11.4、#216）。
///
/// 機器も指していても、機器は残す。部品単位の廃棄は、機器から部品を外して捨てる
/// ことである。起票元にない部品（倉庫の予備など）は捨てられない。
async fn 部品の廃棄は部品の所在だけを変える(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "dispose-part@example.com").await;
    let device_id = 予約対象の機器(db, &場, "dispose-part-host", "running").await;
    let 載っている = 部品(db, 場.user.id, "MEM-DSP-1").await;
    part_instance_location::ActiveModel {
        part_instance_id: Set(載っている.id),
        location_type: Set("Device".to_owned()),
        location_id: Set(Some(device_id)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let w = 廃棄のチケット(
        db,
        場.project_id,
        Some(device_id),
        Some(載っている.id),
        "in_progress",
    )
    .await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let 履歴 = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(載っている.id))
        .order_by_asc(part_instance_location::Column::Id)
        .all(db)
        .await
        .unwrap();
    assert_eq!(履歴.len(), 2, "閉じて開いていません: {履歴:?}");
    assert_eq!(履歴[0].to_date, Some(履歴[1].from_date));
    assert_eq!(履歴[1].location_type, "Disposed");
    assert_eq!(履歴[1].location_id, None);
    assert_eq!(履歴[1].work_order_id, Some(w.id));
    // 機器は残る
    assert_eq!(現在の所属(db, device_id).await, Some(場.project_id));

    // 起票元にない部品（倉庫用のプロジェクトの予備など）は捨てられない
    let 倉庫 = プロジェクト(db, "部品の倉庫").await;
    let 倉庫の部品 = 部品(db, 場.user.id, "MEM-DSP-2").await;
    part_instance_location::ActiveModel {
        part_instance_id: Set(倉庫の部品.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(倉庫.id)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let w = 廃棄のチケット(db, 場.project_id, None, Some(倉庫の部品.id), "in_progress").await;
    let (状態, token) = 認証済み(db, &場.user).await;
    let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("起票元のプロジェクトにありません"), "{body}");
    assert_eq!(再取得(db, w.id).await.status, "in_progress");
}

/// **起票元の設備・什器やプロジェクトに置いた部品も廃棄できること**（#219）。
///
/// 部品がどのプロジェクトのものかは置き場所からたどる。他のプロジェクトの
/// 設備・什器に置いた部品は捨てられない。
async fn 棚やプロジェクトに置いた部品も廃棄できる(db: &DatabaseConnection) {
    let 場 = 増設の舞台(db, "dispose-placed@example.com").await;
    let よそ = プロジェクト(db, "部品のよそ").await;
    let よその棚 = mount_container::ActiveModel {
        name: Set("よその棚".to_owned()),
        location_type: Set("Project".to_owned()),
        location_id: Set(よそ.id),
        created_by: Set(場.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    for (part_number, location_type, location_id, 捨てられる) in [
        ("MEM-SHELF", "MountContainer", 場.container_id, true),
        ("MEM-LOOSE", "Project", 場.project_id, true),
        ("MEM-ELSEWHERE", "MountContainer", よその棚.id, false),
    ] {
        let p = 部品(db, 場.user.id, part_number).await;
        part_instance_location::ActiveModel {
            part_instance_id: Set(p.id),
            location_type: Set(location_type.to_owned()),
            location_id: Set(Some(location_id)),
            from_date: Set(Utc::now()),
            to_date: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();

        let w = 廃棄のチケット(db, 場.project_id, None, Some(p.id), "in_progress").await;
        let (状態, token) = 認証済み(db, &場.user).await;
        let (status, body) = 遷移(状態, &token, 場.project_id, w.id, "complete", "").await;
        let 現在 = part_instance_location::Entity::find()
            .filter(part_instance_location::Column::PartInstanceId.eq(p.id))
            .filter(part_instance_location::Column::ToDate.is_null())
            .one(db)
            .await
            .unwrap()
            .unwrap();
        if 捨てられる {
            assert_eq!(status, StatusCode::SEE_OTHER, "{part_number}: {body}");
            assert_eq!(現在.location_type, "Disposed", "{part_number}");
        } else {
            assert_eq!(status, StatusCode::OK, "{part_number}");
            assert!(body.contains("起票元のプロジェクトにありません"), "{body}");
            assert_eq!(現在.location_type, location_type, "{part_number}");
        }
    }
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// 増設のチケットと、予約先のラック・予約中の機器が揃った状態。
struct 増設の舞台 {
    user: app_user::Model,
    project_id: i32,
    work_order_id: i32,
    container_id: i32,
    device_id: i32,
}

async fn 増設の舞台(db: &DatabaseConnection, email: &str) -> 増設の舞台 {
    let (user, p) = 準備(db, email, "Operator").await;

    let container = mount_container::ActiveModel {
        name: Set("Rack-01".to_owned()),
        container_model_id: Set(Some(
            crate::support::設備の型番(db, user.id, "Rack".to_owned(), Some(42)).await,
        )),
        location_type: Set("Project".to_owned()),
        location_id: Set(p.id),
        created_by: Set(user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    let mut 場 = 増設の舞台 {
        user,
        project_id: p.id,
        work_order_id: 0,
        container_id: container.id,
        device_id: 0,
    };

    // **①機器の登録は承認不要**（11.6）。status=plan で登録されている
    場.device_id = 予約対象の機器(db, &場, "new-srv", "planned").await;

    let w = work_order::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        project_id: Set(p.id),
        device_id: Set(Some(場.device_id)),
        work_type: Set("Addition".to_owned()),
        title: Set("ラックへの追加".to_owned()),
        description: Set(String::new()),
        status: Set("planned".to_owned()),
        planned_at: Set(Some(Utc::now())),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    承認行を作る(db, w.id, &[p.id]).await;
    場.work_order_id = w.id;
    場
}

async fn 予約対象の機器(
    db: &DatabaseConnection,
    場: &増設の舞台,
    hostname: &str,
    status: &str,
) -> i32 {
    let d = device::ActiveModel {
        uid: Set(uuid::Uuid::new_v4().to_string()),
        hostname: Set(hostname.to_owned()),
        device_type: Set("Physical".to_owned()),
        power_watt: Set(350),
        status: Set(status.to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    device_assignment::ActiveModel {
        device_id: Set(d.id),
        location_type: Set("Project".to_owned()),
        location_id: Set(Some(場.project_id)),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    d.id
}

async fn 予約(
    state: AppState,
    token: &str,
    場: &増設の舞台,
    extra: &[(&str, &str)],
) -> (StatusCode, String) {
    let mut fields: Vec<(&str, String)> = vec![("container_id", 場.container_id.to_string())];
    for (k, v) in extra {
        fields.push((k, (*v).to_owned()));
    }
    let borrowed: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
    送信(
        state,
        &format!(
            "/projects/{}/work-orders/{}/reserve",
            場.project_id, 場.work_order_id
        ),
        token,
        &borrowed,
    )
    .await
}

/// 承認済みの移譲のチケット。承認の流れは別のテストで確かめている。
async fn 移譲のチケット(
    db: &DatabaseConnection,
    project_id: i32,
    target_project_id: i32,
    device_id: Option<i32>,
) -> work_order::Model {
    let w = 起票(db, project_id, Some(target_project_id), "Transfer", None).await;
    let mut active: work_order::ActiveModel = w.into();
    active.device_id = Set(device_id);
    active.status = Set("approved".to_owned());
    active.update(db).await.unwrap()
}

/// 廃棄のチケット。承認の流れは別のテストで確かめている。
async fn 廃棄のチケット(
    db: &DatabaseConnection,
    project_id: i32,
    device_id: Option<i32>,
    part_instance_id: Option<i32>,
    status: &str,
) -> work_order::Model {
    let w = 起票(db, project_id, None, "Disposal", None).await;
    let mut active: work_order::ActiveModel = w.into();
    active.device_id = Set(device_id);
    active.part_instance_id = Set(part_instance_id);
    active.status = Set(status.to_owned());
    active.update(db).await.unwrap()
}

/// 部品の実物。カタログとベンダーもあわせて作る。
async fn 部品(db: &DatabaseConnection, user_id: i32, part_number: &str) -> part_instance::Model {
    let v = vendor::ActiveModel {
        name: Set(format!("ベンダー-{part_number}")),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let c = part_catalog::ActiveModel {
        category: Set("Memory".to_owned()),
        vendor_id: Set(v.id),
        part_number: Set(part_number.to_owned()),
        capacity_gb: Set(Some(32)),
        spec_json: Set("{}".to_owned()),
        created_by: Set(user_id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    part_instance::ActiveModel {
        part_catalog_id: Set(c.id),
        serial_number: Set(Some(format!("SN-{part_number}"))),
        status: Set("running".to_owned()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// 機器が今いるプロジェクト。
async fn 現在の所属(db: &DatabaseConnection, device_id: i32) -> Option<i32> {
    device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
        .and_then(|a| a.location_id)
}

async fn 現行の搭載(db: &DatabaseConnection, device_id: i32) -> Option<device_mount::Model> {
    device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(db)
        .await
        .unwrap()
}

async fn 機器の故障(db: &DatabaseConnection, device_id: i32) -> String {
    device::Entity::find_by_id(device_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .health
}

async fn 故障させる(db: &DatabaseConnection, device_id: i32) {
    device::ActiveModel {
        id: Set(device_id),
        health: Set("failed".to_owned()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

/// 修理のチケット。承認の流れは別のテストで確かめている。
async fn 修理のチケット(
    db: &DatabaseConnection,
    project_id: i32,
    device_id: Option<i32>,
    part_instance_id: Option<i32>,
    status: &str,
) -> work_order::Model {
    let w = 起票(db, project_id, None, "Repair", None).await;
    let mut active: work_order::ActiveModel = w.into();
    active.device_id = Set(device_id);
    active.part_instance_id = Set(part_instance_id);
    active.status = Set(status.to_owned());
    active.update(db).await.unwrap()
}

async fn 機器の状態(db: &DatabaseConnection, device_id: i32) -> String {
    device::Entity::find_by_id(device_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .status
}

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
        &[("to", to), ("cancelled_reason", reason)],
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
        uid: Set(uuid::Uuid::new_v4().to_string()),
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
        username: Set((email.to_owned()).replace('@', "_")),
        email: Set(Some(email.to_owned())),
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
        全検証!(@one $用意, $属性, 期日は斜線区切りでも通る);
        全検証!(@one $用意, $属性, 期日は空なら未設定で読めなければ拒否する);
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
        全検証!(@one $用意, $属性, 中止済みには承認ボタンを出さない);
        全検証!(@one $用意, $属性, 計画時点でラック位置を予約できる);
        全検証!(@one $用意, $属性, 二重予約は既存の検証で止まる);
        全検証!(@one $用意, $属性, 中断すると予約が解放される);
        全検証!(@one $用意, $属性, 実行すると予約が実機になる);
        全検証!(@one $用意, $属性, 予約でない機器の状態は変えない);
        全検証!(@one $用意, $属性, 承認後は予約を作れない);
        全検証!(@one $用意, $属性, 移設では予約を作れない);
        全検証!(@one $用意, $属性, 既定では終わったものを隠す);
        全検証!(@one $用意, $属性, 他プロジェクトのチケットは見えない);
        全検証!(@one $用意, $属性, 移譲を実行すると所属が移る);
        全検証!(@one $用意, $属性, 外すものが残る移譲は実行できない);
        全検証!(@one $用意, $属性, 起票元にない機器は移譲できない);
        全検証!(@one $用意, $属性, 廃棄を完了すると所在がdisposedになる);
        全検証!(@one $用意, $属性, 外すものが残る廃棄は完了できない);
        全検証!(@one $用意, $属性, ケーブルが挿さっていると動かせない);
        全検証!(@one $用意, $属性, 部品の廃棄は部品の所在だけを変える);
        全検証!(@one $用意, $属性, 棚やプロジェクトに置いた部品も廃棄できる);
        全検証!(@one $用意, $属性, 移譲先からもチケットが見える);
        全検証!(@one $用意, $属性, 移譲の実行では予約中を進めない);
        全検証!(@one $用意, $属性, 倉庫から払い出すと予約中になる);
        全検証!(@one $用意, $属性, 倉庫へ入れても状態は変えない);
        全検証!(@one $用意, $属性, 修理の完了で機器の故障を戻す);
        全検証!(@one $用意, $属性, 部品の修理は部品の故障だけを戻す);
        全検証!(@one $用意, $属性, 修理の起票で部品を指定できる);
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
