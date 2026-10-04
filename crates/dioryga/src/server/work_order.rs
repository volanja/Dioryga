//! 変更管理チケット（設計書11章、16.1のB領域）。
//!
//! 16.7で「**v1で最も重い画面**」と見積もった箇所。画面というより、状態遷移と
//! 承認のワークフローの実装である。
//!
//! # `approved` は画面から直接立てない
//!
//! 11.4-7の通り、**紐づく全 `WORK_ORDER_APPROVAL` が `approved` になった時点で**
//! `WORK_ORDER` が `approved` へ遷移する。承認ボタンが押すのは承認行であって、
//! チケットの状態ではない。ここを取り違えると、承認が1件足りないまま
//! `approved` にできてしまう。
//!
//! したがって遷移の判定は [`承認が揃ったか`] に集約し、承認・却下のどちらの
//! 経路からも必ずそこを通す。
//!
//! # 自己承認（11.4-9、22章R-2）
//!
//! `approver_id` は担当者（主・副）と同一人物であってはならない。**ただし
//! 承認できる他のメンバーがいない場合に限り許す。**個人利用を想定利用形態に
//! 掲げている以上（1.3）、承認者が自分しかいない環境でチケットが永久に承認
//! されないのは避けなければならない。
//!
//! **承認ステップ自体は省略しない。**一人で作業する状況ではむしろ、変更計画を
//! 立てて自分でレビューしてから進む手順の価値が高い。承認を飛ばすのではなく、
//! 承認者が自分自身であることを `self_approved` に明示的に記録する。
//!
//! # 計画は予約として実データに載せる（11.6）
//!
//! `planned` の時点で `DEVICE_MOUNT` を **`work_order_id` 付きで実際に作る。**
//! Executeまで待たない。こうするとラック図がそのまま予約状況の図になり、
//! 他の担当者が計画中の場所を誤って使うことを防げる（1.1、11.6）。
//!
//! **中断したら解放する。**閉じ忘れるとラックが埋まったまま残り、予約という
//! 仕組みそのものが信用されなくなる。
//!
//! **検証は [`crate::server::rack`] と同じ経路を通す。**予約用に別の検証を
//! 持つと、片方だけ直したときに規則が食い違う。
//!
//! # 誰が何をできるか
//!
//! | 操作 | ロール |
//! |---|---|
//! | 閲覧 | メンバー（`require_project_member`） |
//! | 起票・実行・完了・中止 | Administrator / Operator（`require_project_editor`） |
//! | 承認・却下 | **承認先プロジェクトの** Administrator / Approver |
//!
//! **承認の権限は `required_project_id` のプロジェクトで判定する。**起票元では
//! ない。移譲（Transfer）では移譲先の承認者が承認するため、ここを起票元で見ると
//! 移譲先の承認ができなくなる。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::{NaiveDate, Utc};
use entity::{
    app_user, device, device_assignment, device_mount, ip_address, mount_container, os_interface,
    part_catalog, part_instance, part_instance_location, project, project_member, work_order,
    work_order_approval,
};
use sea_orm::prelude::DateTimeUtc;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;

use crate::auth::authorization::{self, ADMINISTRATOR, APPROVER};
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{
    render, チケットの状態の表示, チケット番号, Chrome, Locale
};
use crate::server::AppState;

/// 語彙（`vocabularies.md`、設計書11.3）。DB制約にはせず、画面はリストから選ばせる。
const WORK_TYPES: &[&str] = &["Repair", "Addition", "Relocation", "Disposal", "Transfer"];

/// 移譲。**このときだけ承認行が2つになる**（11.5）。
const TRANSFER: &str = "Transfer";

/// 廃棄。**完了で所在が `Disposed` になる**（11.4）。
const DISPOSAL: &str = "Disposal";

/// `DEVICE_ASSIGNMENT.location_type`（6.2）。
const PROJECT: &str = "Project";
/// `DEVICE_ASSIGNMENT` / `PART_INSTANCE_LOCATION` の `location_type`。参照先を持たない（6.2）。
const DISPOSED: &str = "Disposed";

/// 増設。**予約レコードを作れるのはこれだけ**（11.6）。実行で予約中の機器を
/// 稼働中へ進めるのもこれだけ（6.3）。
const ADDITION: &str = "Addition";

/// 修理。**正常に完了すると、対象の `health` を `ok` に戻す**（6.3）。
const REPAIR: &str = "Repair";

/// 部品を対象にできる作業種別。修理（6.3）と廃棄（11.4）だけが部品を扱う。
const PART_WORK_TYPES: &[&str] = &[REPAIR, DISPOSAL];

/// 予約中の機器（11.6）。ラック図が破線で描く。
/// 機器の状態（8.6）。チケットの `planned` とは別の列である
const DEVICE_PLANNED: &str = crate::device_state::PLANNED;
const RUNNING: &str = crate::device_state::RUNNING;

const PLANNED: &str = "planned";
const APPROVED: &str = "approved";
const IN_PROGRESS: &str = "in_progress";
const COMPLETED: &str = "completed";
const CANCELLED: &str = "cancelled";

const PENDING: &str = "pending";
const REJECTED: &str = "rejected";

// ---------------------------------------------------------------------------
// 予約（設計書11.6）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ReserveForm {
    pub container_id: i32,
    #[serde(default)]
    pub position: String,
    #[serde(default)]
    pub horizontal_position: String,
    #[serde(default)]
    pub depth_position: String,
}

/// ラック位置を予約する（設計書11.6）。
///
/// **Executeまで待たずに `DEVICE_MOUNT` を作る。**こうするとラック図がそのまま
/// 予約状況の図になり、二重予約は12.3の重複配置検証がそのまま止める——予約用の
/// 新しい仕組みを足していない。
pub async fn reserve(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, work_order_id)): Path<(i32, i32)>,
    Form(form): Form<ReserveForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let l = Locale::parse(&current.user.locale).as_str();
    let 訳 = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    let w = 取得(&state, project_id, work_order_id).await?;

    // **予約は計画中の増設にしか作れない**（11.6）。
    //
    // - 承認後に場所が変わっては、承認した内容と実施する内容がずれる
    // - **移設への適用は未検討**（11.6の末尾、17章の課題）。稼働中の機器は
    //   移設元で status="running" のまま稼働している必要があり、同じ仕組みを
    //   そのまま当てられない
    let 拒否 = if w.status != PLANNED {
        Some("work_orders.error_reserve_not_planned")
    } else if w.work_type != ADDITION {
        Some("work_orders.error_reserve_work_type")
    } else if w.device_id.is_none() {
        Some("work_orders.error_reserve_no_device")
    } else {
        None
    };
    if let Some(key) = 拒否 {
        return 詳細を描く(
            &state,
            &current,
            project_id,
            work_order_id,
            Some(訳(key)),
            None,
        )
        .await;
    }
    let device_id = w.device_id.expect("上で確認済み");

    // **検証はrackと同じ経路を通す**（重複配置・半width・0Uサイドマウント）
    let 結果 = crate::server::rack::搭載を試みる(
        &state,
        current.user.id,
        crate::server::rack::搭載要求 {
            project_id,
            container_id: form.container_id,
            device_id,
            position: &form.position,
            horizontal: &form.horizontal_position,
            depth: &form.depth_position,
            host_device_id: "",
            work_order_id: Some(w.id),
        },
    )
    .await?;

    match 結果 {
        Err(key) => {
            詳細を描く(
                &state,
                &current,
                project_id,
                work_order_id,
                Some(訳(key)),
                None,
            )
            .await
        }
        Ok(warnings) => {
            let notice = (!warnings.is_empty())
                .then(|| warnings.iter().map(|k| 訳(k)).collect::<Vec<_>>().join(" "));
            if notice.is_some() {
                return 詳細を描く(&state, &current, project_id, work_order_id, None, notice).await;
            }
            Ok(Redirect::to(&format!(
                "/projects/{project_id}/work-orders/{work_order_id}"
            ))
            .into_response())
        }
    }
}

// ---------------------------------------------------------------------------
// 状態遷移（設計書11.2）
// ---------------------------------------------------------------------------

/// 画面から起こせる遷移。
///
/// **`approved` は含まない。**承認が揃った結果として自動的に遷移するものであり、
/// 人が直接押すものではない（11.4-7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    Execute,
    Complete,
    Abort,
}

impl Transition {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "execute" => Some(Self::Execute),
            "complete" => Some(Self::Complete),
            "abort" => Some(Self::Abort),
            _ => None,
        }
    }

    /// この遷移を起こせる元の状態（11.2の状態遷移図）。
    fn 遷移元(self) -> &'static [&'static str] {
        match self {
            Self::Execute => &[APPROVED],
            Self::Complete => &[IN_PROGRESS],
            // **中止はどの途中状態からもできる。**「結果はどうだったのか。
            // 中断、完了。」という要件に対応する（11.2）
            Self::Abort => &[PLANNED, APPROVED, IN_PROGRESS],
        }
    }

    fn 遷移先(self) -> &'static str {
        match self {
            Self::Execute => IN_PROGRESS,
            Self::Complete => COMPLETED,
            Self::Abort => CANCELLED,
        }
    }
}

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct WorkOrderRow {
    id: i32,
    /// 人が連絡に使う番号（11.4-11）。`id` から作る
    number: String,
    title: String,
    work_type: String,
    status: String,
    assignee: String,
    due_date: String,
    /// 期限を過ぎていて、まだ終わっていない（11.4-6）。
    overdue: bool,
    /// 承認が1件でも自己承認だった（11.4-9）。
    self_approved: bool,
    approvals: String,
}

#[derive(askama::Template)]
#[template(path = "work_orders.html")]
struct WorkOrdersPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_new: String,
    t_keyword: String,
    t_search: String,
    t_status: String,
    t_status_open: String,
    t_status_all: String,
    t_mine: String,
    t_number: String,
    t_ticket: String,
    t_work_type: String,
    t_assignee: String,
    t_due_date: String,
    t_approvals: String,
    t_actions: String,
    t_detail: String,
    t_empty: String,
    t_overdue: String,
    t_self_approved: String,
    q: String,
    status: String,
    mine: bool,
    rows: Vec<WorkOrderRow>,
    can_edit: bool,
}

struct ApprovalRow {
    id: i32,
    project_name: String,
    status: String,
    approver: String,
    approved_at: String,
    self_approved: bool,
    /// 閲覧者がこの行を承認できる。
    actionable: bool,
}

struct Labeled {
    label: String,
    value: String,
}

#[derive(askama::Template)]
#[template(path = "work_order_detail.html")]
struct WorkOrderDetailPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    work_order_id: i32,
    /// 見出しに添える番号（11.4-11）。
    number: String,
    t_back: String,
    t_basic: String,
    t_approvals: String,
    t_project: String,
    t_status: String,
    t_approver: String,
    t_approved_at: String,
    t_approve: String,
    t_reject: String,
    t_self_approved: String,
    t_self_approved_hint: String,
    t_transitions: String,
    t_execute: String,
    t_complete: String,
    t_abort: String,
    t_abort_reason: String,
    t_none: String,
    t_all_approved_hint: String,
    title: String,
    status: String,
    basic: Vec<Labeled>,
    approvals: Vec<ApprovalRow>,
    /// この閲覧者が起こせる遷移。空なら操作欄を出さない。
    can_execute: bool,
    can_complete: bool,
    can_abort: bool,
    // --- 予約（設計書11.6） ---
    t_reservations: String,
    t_reservations_hint: String,
    t_reserve: String,
    t_container: String,
    t_position: String,
    t_position_hint: String,
    t_horizontal: String,
    t_depth: String,
    t_released: String,
    t_relocation_hint: String,
    /// この画面から予約を作れる。`planned` かつ `Addition` のときだけ（11.6）。
    can_reserve: bool,
    /// 移設は「Plan時点で正式レコードを作る」方式を当てられない（11.6、17章）。
    relocation_unsupported: bool,
    containers: Vec<Labeled>,
    reservations: Vec<ReservationRow>,
    horizontals: Vec<&'static str>,
    depths: Vec<&'static str>,
    error: Option<String>,
    notice: Option<String>,
}

/// このチケットが確保している配置（設計書11.6）。
struct ReservationRow {
    container: String,
    place: String,
    /// 中断で解放済み。**行は消さず閉じる**ため、閉じたものも見える
    released: bool,
}

#[derive(askama::Template)]
#[template(path = "work_order_form.html")]
struct WorkOrderFormPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_work_type: String,
    t_work_type_hint: String,
    t_ticket_title: String,
    t_description: String,
    t_target_project: String,
    t_target_project_hint: String,
    t_device: String,
    t_device_hint: String,
    t_part: String,
    t_part_hint: String,
    t_primary: String,
    t_secondary: String,
    t_secondary_hint: String,
    t_due_date: String,
    t_unset: String,
    t_submit: String,
    work_type: String,
    work_types: Vec<&'static str>,
    title: String,
    description: String,
    target_project_id: String,
    projects: Vec<Labeled>,
    device_id: String,
    devices: Vec<Labeled>,
    part_instance_id: String,
    parts: Vec<Labeled>,
    primary_assignee_id: String,
    secondary_assignee_id: String,
    members: Vec<Labeled>,
    due_date: String,
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub mine: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    // 既定は「まだ終わっていないもの」。16.1の「完了済みを隠せるフィルタは必須」
    let 全件 = query.status.as_deref() == Some("all");
    let 自分のみ = query.mine.as_deref() == Some("1");
    let keyword = query.q.clone().unwrap_or_default();

    let mut tickets = work_order::Entity::find()
        // **起票元と移譲先の両方を出す。**移譲先のメンバーから見えないと、
        // 自分のプロジェクトへ来る予定の機器が分からない（11.5）
        .filter(
            Condition::any()
                .add(work_order::Column::ProjectId.eq(project_id))
                .add(work_order::Column::TargetProjectId.eq(project_id)),
        )
        .order_by_desc(work_order::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let 語 = keyword.trim().to_lowercase();
    tickets.retain(|w| {
        if !全件 && (w.status == COMPLETED || w.status == CANCELLED) {
            return false;
        }
        if 自分のみ
            && w.primary_assignee_id != Some(current.user.id)
            && w.secondary_assignee_id != Some(current.user.id)
        {
            return false;
        }
        語.is_empty() || w.title.to_lowercase().contains(&語)
    });

    let 承認 = 承認をまとめて引く(&state.db, &tickets).await?;
    let 氏名 = 利用者名(&state.db, &tickets).await?;
    // **利用者のタイムゾーンの今日**（24.2.3、#210）
    let 今日 = crate::tz::今日(state.タイムゾーン(&current.user));

    let rows = tickets
        .into_iter()
        .map(|w| {
            let 行 = 承認.iter().filter(|a| a.work_order_id == w.id);
            let 済 = 行.clone().filter(|a| a.status == APPROVED).count();
            let 総数 = 行.clone().count();

            WorkOrderRow {
                overdue: w
                    .due_date
                    .is_some_and(|d| d < 今日 && w.status != COMPLETED && w.status != CANCELLED),
                self_approved: 行.clone().any(|a| a.self_approved),
                approvals: format!("{済} / {総数}"),
                assignee: w
                    .primary_assignee_id
                    .and_then(|id| 氏名.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone()))
                    .unwrap_or_else(|| "—".to_owned()),
                due_date: w.due_date.map(|d| d.to_string()).unwrap_or_default(),
                number: チケット番号(w.id),
                id: w.id,
                title: w.title,
                work_type: w.work_type,
                status: チケットの状態の表示(&w.status, l),
            }
        })
        .collect();

    render(&WorkOrdersPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "work_orders",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("work_orders.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("work_orders.lead", locale = l).to_string(),
        t_new: rust_i18n::t!("work_orders.new", locale = l).to_string(),
        t_keyword: rust_i18n::t!("work_orders.keyword", locale = l).to_string(),
        t_search: rust_i18n::t!("common.search", locale = l).to_string(),
        t_status: rust_i18n::t!("work_orders.status", locale = l).to_string(),
        t_status_open: rust_i18n::t!("work_orders.status_open", locale = l).to_string(),
        t_status_all: rust_i18n::t!("work_orders.status_all", locale = l).to_string(),
        t_mine: rust_i18n::t!("work_orders.mine", locale = l).to_string(),
        t_number: rust_i18n::t!("work_orders.number", locale = l).to_string(),
        t_ticket: rust_i18n::t!("work_orders.ticket", locale = l).to_string(),
        t_work_type: rust_i18n::t!("work_orders.work_type", locale = l).to_string(),
        t_assignee: rust_i18n::t!("work_orders.assignee", locale = l).to_string(),
        t_due_date: rust_i18n::t!("work_orders.due_date", locale = l).to_string(),
        t_approvals: rust_i18n::t!("work_orders.approvals", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_empty: rust_i18n::t!("work_orders.empty", locale = l).to_string(),
        t_overdue: rust_i18n::t!("work_orders.overdue", locale = l).to_string(),
        t_self_approved: rust_i18n::t!("work_orders.self_approved", locale = l).to_string(),
        q: keyword,
        status: if 全件 { "all" } else { "open" }.to_owned(),
        mine: 自分のみ,
        rows,
        can_edit,
    })
}

// ---------------------------------------------------------------------------
// 詳細
// ---------------------------------------------------------------------------

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, work_order_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    詳細を描く(&state, &current, project_id, work_order_id, None, None).await
}

async fn 詳細を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    work_order_id: i32,
    error: Option<String>,
    notice: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let w = 取得(state, project_id, work_order_id).await?;
    let 承認 = work_order_approval::Entity::find()
        .filter(work_order_approval::Column::WorkOrderId.eq(w.id))
        .order_by_asc(work_order_approval::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut approvals = Vec::new();
    for a in &承認 {
        let p = project::Entity::find_by_id(a.required_project_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

        // **承認の可否は承認先プロジェクトで判定する。**起票元ではない（11.5）。
        // 中止・完了済みのチケットにボタンを出さない——押しても弾かれるので、
        // 出ていること自体が誤解を招く
        let 承認できる = a.status == PENDING
            && w.status == PLANNED
            && authorization::require_project_role(
                &state.db,
                &current.user,
                a.required_project_id,
                &[ADMINISTRATOR, APPROVER],
            )
            .await
            .is_ok();

        approvals.push(ApprovalRow {
            id: a.id,
            project_name: p.map(|p| p.name).unwrap_or_default(),
            status: a.status.clone(),
            approver: match a.approver_id {
                Some(id) => 氏名を引く(&state.db, id).await?,
                None => "—".to_owned(),
            },
            approved_at: a
                .approved_at
                .map(|t| crate::tz::日時(t, state.タイムゾーン(&current.user)))
                .unwrap_or_default(),
            self_approved: a.self_approved,
            actionable: 承認できる,
        });
    }

    let 未完了 = w.status != COMPLETED && w.status != CANCELLED;
    let basic = 基本情報(state, &w, l, state.タイムゾーン(&current.user)).await?;

    render(&WorkOrderDetailPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "work_orders",
        )
        .await,
        project_id,
        project_name: project.name,
        work_order_id: w.id,
        t_back: rust_i18n::t!("work_orders.back", locale = l).to_string(),
        t_basic: rust_i18n::t!("devices.basic", locale = l).to_string(),
        t_approvals: rust_i18n::t!("work_orders.approvals", locale = l).to_string(),
        t_project: rust_i18n::t!("work_orders.required_project", locale = l).to_string(),
        t_status: rust_i18n::t!("work_orders.status", locale = l).to_string(),
        t_approver: rust_i18n::t!("work_orders.approver", locale = l).to_string(),
        t_approved_at: rust_i18n::t!("work_orders.approved_at", locale = l).to_string(),
        t_approve: rust_i18n::t!("work_orders.approve", locale = l).to_string(),
        t_reject: rust_i18n::t!("work_orders.reject", locale = l).to_string(),
        t_self_approved: rust_i18n::t!("work_orders.self_approved", locale = l).to_string(),
        t_self_approved_hint: rust_i18n::t!("work_orders.self_approved_hint", locale = l)
            .to_string(),
        t_transitions: rust_i18n::t!("work_orders.transitions", locale = l).to_string(),
        t_execute: rust_i18n::t!("work_orders.execute", locale = l).to_string(),
        t_complete: rust_i18n::t!("work_orders.complete", locale = l).to_string(),
        t_abort: rust_i18n::t!("work_orders.abort", locale = l).to_string(),
        t_abort_reason: rust_i18n::t!("work_orders.abort_reason", locale = l).to_string(),
        t_none: rust_i18n::t!("devices.none", locale = l).to_string(),
        t_all_approved_hint: rust_i18n::t!("work_orders.all_approved_hint", locale = l).to_string(),
        title: w.title.clone(),
        number: チケット番号(w.id),
        // 訳すのは表示だけ。保存する値は語彙のまま（#172）
        status: チケットの状態の表示(&w.status, l),
        basic,
        approvals,
        can_execute: can_edit && w.status == APPROVED,
        can_complete: can_edit && w.status == IN_PROGRESS,
        can_abort: can_edit && 未完了,
        // --- 予約（設計書11.6） ---
        t_reservations: rust_i18n::t!("work_orders.reservations", locale = l).to_string(),
        t_reservations_hint: rust_i18n::t!("work_orders.reservations_hint", locale = l).to_string(),
        t_reserve: rust_i18n::t!("work_orders.reserve", locale = l).to_string(),
        t_container: rust_i18n::t!("work_orders.container", locale = l).to_string(),
        t_position: rust_i18n::t!("rack.position", locale = l).to_string(),
        t_position_hint: rust_i18n::t!("rack.position_hint", locale = l).to_string(),
        t_horizontal: rust_i18n::t!("rack.horizontal", locale = l).to_string(),
        t_depth: rust_i18n::t!("rack.depth", locale = l).to_string(),
        t_released: rust_i18n::t!("work_orders.released", locale = l).to_string(),
        t_relocation_hint: rust_i18n::t!("work_orders.relocation_hint", locale = l).to_string(),
        // **予約を作れるのは計画中の増設だけ**（11.6）
        can_reserve: can_edit && w.status == PLANNED && w.work_type == ADDITION,
        relocation_unsupported: w.work_type == "Relocation",
        containers: 什器の候補(&state.db, project_id).await?,
        reservations: 予約一覧(&state.db, w.id).await?,
        horizontals: vec!["Left", "Right", "Full"],
        depths: vec!["Front", "Rear", "Full"],
        error,
        notice,
    })
}

/// このチケットが確保している配置（設計書11.6）。
///
/// **閉じたものも出す。**中断で解放された事実が見えないと、何が起きたのか
/// 追えない（不変条件1で行を消さないのと同じ理由）。
async fn 予約一覧<C: ConnectionTrait>(
    db: &C,
    work_order_id: i32,
) -> AppResult<Vec<ReservationRow>> {
    let mounts = device_mount::Entity::find()
        .filter(device_mount::Column::WorkOrderId.eq(work_order_id))
        .order_by_asc(device_mount::Column::Id)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for m in mounts {
        let container = match m.container_id {
            Some(id) => mount_container::Entity::find_by_id(id)
                .one(db)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .map(|c| c.name)
                .unwrap_or_default(),
            None => String::new(),
        };

        let mut place = Vec::new();
        if let Some(p) = m.position {
            place.push(format!("{p}U"));
        }
        if let Some(h) = &m.horizontal_position {
            place.push(h.clone());
        }
        if let Some(d) = &m.depth_position {
            place.push(d.clone());
        }

        rows.push(ReservationRow {
            container,
            place: place.join(" / "),
            released: m.to_date.is_some(),
        });
    }
    Ok(rows)
}

async fn 什器の候補<C: ConnectionTrait>(db: &C, project_id: i32) -> AppResult<Vec<Labeled>> {
    // **撤去した設備は予約先の候補に出さない**（12.10、#204）
    Ok(mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq("Project"))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .filter(mount_container::Column::RetiredAt.is_null())
        .order_by_asc(mount_container::Column::Name)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|c| Labeled {
            label: c.name,
            value: c.id.to_string(),
        })
        .collect())
}

async fn 基本情報(
    state: &AppState,
    w: &work_order::Model,
    l: &str,
    tz: chrono_tz::Tz,
) -> AppResult<Vec<Labeled>> {
    let mut basic = vec![
        Labeled {
            label: rust_i18n::t!("work_orders.work_type", locale = l).to_string(),
            value: w.work_type.clone(),
        },
        Labeled {
            label: rust_i18n::t!("work_orders.status", locale = l).to_string(),
            value: w.status.clone(),
        },
    ];

    if !w.description.is_empty() {
        basic.push(Labeled {
            label: rust_i18n::t!("work_orders.description", locale = l).to_string(),
            value: w.description.clone(),
        });
    }

    if let Some(id) = w.target_project_id {
        if let Some(p) = project::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            basic.push(Labeled {
                label: rust_i18n::t!("work_orders.target_project", locale = l).to_string(),
                value: p.name,
            });
        }
    }

    if let Some(id) = w.device_id {
        if let Some(d) = device::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            basic.push(Labeled {
                label: rust_i18n::t!("work_orders.device", locale = l).to_string(),
                value: d.hostname,
            });
        }
    }

    if let Some(id) = w.part_instance_id {
        if let Some(p) = part_instance::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            basic.push(Labeled {
                label: rust_i18n::t!("work_orders.part", locale = l).to_string(),
                value: 部品の表示名(&state.db, &p).await?,
            });
        }
    }

    for (key, id) in [
        ("work_orders.primary_assignee", w.primary_assignee_id),
        ("work_orders.secondary_assignee", w.secondary_assignee_id),
    ] {
        if let Some(id) = id {
            basic.push(Labeled {
                label: rust_i18n::t!(key, locale = l).to_string(),
                value: 氏名を引く(&state.db, id).await?,
            });
        }
    }

    if let Some(d) = w.due_date {
        basic.push(Labeled {
            label: rust_i18n::t!("work_orders.due_date", locale = l).to_string(),
            value: d.to_string(),
        });
    }

    // **予定と実績を別に見せる**（5.1）。片方に上書きしていないことが分かる
    for (key, at) in [
        ("work_orders.executed_at", w.executed_at),
        ("work_orders.completed_at", w.completed_at),
        ("work_orders.cancelled_at", w.cancelled_at),
    ] {
        if let Some(at) = at {
            basic.push(Labeled {
                label: rust_i18n::t!(key, locale = l).to_string(),
                value: crate::tz::日時(at, tz),
            });
        }
    }

    // 中止は理由とともに残す。**削除しない**（旧C-2）
    if let Some(reason) = &w.cancelled_reason.clone().filter(|r| !r.is_empty()) {
        basic.push(Labeled {
            label: rust_i18n::t!("work_orders.abort_reason", locale = l).to_string(),
            value: reason.clone(),
        });
    }

    Ok(basic)
}

// ---------------------------------------------------------------------------
// 起票
// ---------------------------------------------------------------------------

pub async fn new_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    let form = CreateForm::default();
    起票フォームを描く(&state, &current, project_id, &form, None).await
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateForm {
    #[serde(default)]
    pub work_type: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub target_project_id: String,
    #[serde(default)]
    pub device_id: String,
    /// 対象の部品。修理と廃棄のときだけ指定できる（6.3、11.4）
    #[serde(default)]
    pub part_instance_id: String,
    #[serde(default)]
    pub primary_assignee_id: String,
    #[serde(default)]
    pub secondary_assignee_id: String,
    #[serde(default)]
    pub due_date: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<CreateForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let l = Locale::parse(&current.user.locale).as_str();

    let 中止 = |key: &str| -> AppResult<Option<String>> {
        Ok(Some(rust_i18n::t!(key, locale = l).to_string()))
    };

    if form.title.trim().is_empty() {
        let e = 中止("work_orders.error_title")?;
        return 起票フォームを描く(&state, &current, project_id, &form, e).await;
    }
    if !WORK_TYPES.contains(&form.work_type.as_str()) {
        // **語彙外は既定へ寄せず拒否する**（Q-21）。黙って別の意味に変わるより、
        // 弾いて直させるほうがよい
        let e = 中止("work_orders.error_work_type")?;
        return 起票フォームを描く(&state, &current, project_id, &form, e).await;
    }

    let target_project_id = 数値(&form.target_project_id);
    if form.work_type == TRANSFER && target_project_id.is_none() {
        let e = 中止("work_orders.error_target_project")?;
        return 起票フォームを描く(&state, &current, project_id, &form, e).await;
    }
    if form.work_type != TRANSFER && target_project_id.is_some() {
        let e = 中止("work_orders.error_target_project_unused")?;
        return 起票フォームを描く(&state, &current, project_id, &form, e).await;
    }
    if target_project_id == Some(project_id) {
        let e = 中止("work_orders.error_target_project_same")?;
        return 起票フォームを描く(&state, &current, project_id, &form, e).await;
    }

    let due_date = match 日付(&form.due_date) {
        Ok(d) => d,
        Err(()) => {
            let e = 中止("work_orders.error_due_date")?;
            return 起票フォームを描く(&state, &current, project_id, &form, e).await;
        }
    };

    // **部品を対象にできるのは修理と廃棄だけ**（6.3、11.4）。対象は今このプロジェクトにある部品
    let part_instance_id = 数値(&form.part_instance_id);
    if let Some(part_id) = part_instance_id {
        if !PART_WORK_TYPES.contains(&form.work_type.as_str()) {
            let e = 中止("work_orders.error_part_work_type")?;
            return 起票フォームを描く(&state, &current, project_id, &form, e).await;
        }
        if 部品の現在のプロジェクト(&state.db, part_id).await? != Some(project_id) {
            let e = 中止("work_orders.error_part_invalid")?;
            return 起票フォームを描く(&state, &current, project_id, &form, e).await;
        }
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let now = Utc::now();
    let created = tx
        .insert(work_order::ActiveModel {
            // **画面から起票した行にも採番する。**取込が突合に使う（23.5）
            uid: Set(uuid::Uuid::new_v4().to_string()),
            external_id: Set(None),
            project_id: Set(project_id),
            target_project_id: Set(target_project_id),
            device_id: Set(数値(&form.device_id)),
            part_instance_id: Set(part_instance_id),
            work_type: Set(form.work_type.clone()),
            title: Set(form.title.trim().to_owned()),
            description: Set(form.description.trim().to_owned()),
            primary_assignee_id: Set(数値(&form.primary_assignee_id)),
            secondary_assignee_id: Set(数値(&form.secondary_assignee_id)),
            due_date: Set(due_date),
            status: Set(PLANNED.to_owned()),
            planned_at: Set(Some(now)),
            executed_at: Set(None),
            completed_at: Set(None),
            cancelled_at: Set(None),
            cancelled_reason: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **承認行を自動生成する**（11.5）。通常は起票元の1行、Transferは移譲先も
    let mut 承認先 = vec![project_id];
    if let Some(t) = target_project_id {
        承認先.push(t);
    }
    for pid in 承認先 {
        tx.insert(work_order_approval::ActiveModel {
            work_order_id: Set(created.id),
            required_project_id: Set(pid),
            approver_id: Set(None),
            status: Set(PENDING.to_owned()),
            approved_at: Set(None),
            self_approved: Set(false),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!(
        "/projects/{project_id}/work-orders/{}",
        created.id
    ))
    .into_response())
}

async fn 起票フォームを描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    form: &CreateForm,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, _) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    render(&WorkOrderFormPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "work_orders",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("work_orders.new", locale = l).to_string(),
        t_lead: rust_i18n::t!("work_orders.new_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("work_orders.back", locale = l).to_string(),
        t_work_type: rust_i18n::t!("work_orders.work_type", locale = l).to_string(),
        t_work_type_hint: rust_i18n::t!("work_orders.work_type_hint", locale = l).to_string(),
        t_ticket_title: rust_i18n::t!("work_orders.ticket_title", locale = l).to_string(),
        t_description: rust_i18n::t!("work_orders.description", locale = l).to_string(),
        t_target_project: rust_i18n::t!("work_orders.target_project", locale = l).to_string(),
        t_target_project_hint: rust_i18n::t!("work_orders.target_project_hint", locale = l)
            .to_string(),
        t_device: rust_i18n::t!("work_orders.device", locale = l).to_string(),
        t_device_hint: rust_i18n::t!("work_orders.device_hint", locale = l).to_string(),
        t_part: rust_i18n::t!("work_orders.part", locale = l).to_string(),
        t_part_hint: rust_i18n::t!("work_orders.part_hint", locale = l).to_string(),
        t_primary: rust_i18n::t!("work_orders.primary_assignee", locale = l).to_string(),
        t_secondary: rust_i18n::t!("work_orders.secondary_assignee", locale = l).to_string(),
        t_secondary_hint: rust_i18n::t!("work_orders.secondary_hint", locale = l).to_string(),
        t_due_date: rust_i18n::t!("work_orders.due_date", locale = l).to_string(),
        t_unset: rust_i18n::t!("work_orders.unset", locale = l).to_string(),
        t_submit: rust_i18n::t!("work_orders.submit", locale = l).to_string(),
        work_type: form.work_type.clone(),
        work_types: WORK_TYPES.to_vec(),
        title: form.title.clone(),
        description: form.description.clone(),
        target_project_id: form.target_project_id.clone(),
        projects: 他のプロジェクト(state, current, project_id).await?,
        device_id: form.device_id.clone(),
        devices: このプロジェクトの機器(state, project_id).await?,
        part_instance_id: form.part_instance_id.clone(),
        parts: このプロジェクトの部品(state, project_id).await?,
        primary_assignee_id: form.primary_assignee_id.clone(),
        secondary_assignee_id: form.secondary_assignee_id.clone(),
        members: メンバー(&state.db, project_id).await?,
        due_date: form.due_date.clone(),
        error,
    })
}

// ---------------------------------------------------------------------------
// 承認（設計書11.4-7、11.4-9）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ApproveForm {
    pub approval_id: i32,
    /// `approve` または `reject`。
    pub decision: String,
}

pub async fn approve(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, work_order_id)): Path<(i32, i32)>,
    Form(form): Form<ApproveForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let w = 取得(&state, project_id, work_order_id).await?;

    let approval = work_order_approval::Entity::find_by_id(form.approval_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|a| a.work_order_id == w.id)
        .ok_or(AppError::NotFound)?;

    // **承認先プロジェクトで判定する。**起票元ではない（11.5）
    authorization::require_project_role(
        &state.db,
        &current.user,
        approval.required_project_id,
        &[ADMINISTRATOR, APPROVER],
    )
    .await
    .map_err(|_| AppError::Forbidden)?;

    if approval.status != PENDING {
        let e = rust_i18n::t!("work_orders.error_already_decided", locale = l).to_string();
        return 詳細を描く(&state, &current, project_id, work_order_id, Some(e), None).await;
    }
    // 中止・完了済みのチケットを後から承認できてしまうと、状態が意味を失う
    if w.status != PLANNED {
        let e = rust_i18n::t!("work_orders.error_not_planned", locale = l).to_string();
        return 詳細を描く(&state, &current, project_id, work_order_id, Some(e), None).await;
    }

    let 却下 = form.decision == "reject";

    // **自己承認の判定**（11.4-9、22章R-2）
    let 本人 = w.primary_assignee_id == Some(current.user.id)
        || w.secondary_assignee_id == Some(current.user.id);
    let mut self_approved = false;
    if 本人 && !却下 {
        if 他に承認できる人がいる(
            &state.db,
            approval.required_project_id,
            current.user.id,
        )
        .await?
        {
            let e = rust_i18n::t!("work_orders.error_self_approval", locale = l).to_string();
            return 詳細を描く(&state, &current, project_id, work_order_id, Some(e), None).await;
        }
        // 承認者が自分しかいない。**承認ステップは省略せず、その旨を記録して通す**
        self_approved = true;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let now = Utc::now();
    tx.update(
        &approval,
        work_order_approval::ActiveModel {
            id: Set(approval.id),
            approver_id: Set(Some(current.user.id)),
            status: Set(if 却下 { REJECTED } else { APPROVED }.to_owned()),
            approved_at: Set(if 却下 { None } else { Some(now) }),
            self_approved: Set(self_approved),
            updated_at: Set(now),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **全承認が揃った時点でチケットを approved へ進める**（11.4-7）。
    // 承認ボタンが押したのは承認行であって、チケットの状態ではない
    if 承認が揃ったか(tx.reader(), w.id).await? {
        tx.update(
            &w,
            work_order::ActiveModel {
                id: Set(w.id),
                status: Set(APPROVED.to_owned()),
                updated_at: Set(now),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!(
        "/projects/{project_id}/work-orders/{work_order_id}"
    ))
    .into_response())
}

/// 紐づく**全**承認行が `approved` か（設計書11.4-7）。
///
/// **1件でも `pending` や `rejected` が残っていれば偽。**却下された行がある
/// うちは、他が全部承認済みでも進まない。
async fn 承認が揃ったか<C: ConnectionTrait>(db: &C, work_order_id: i32) -> AppResult<bool> {
    let 行 = work_order_approval::Entity::find()
        .filter(work_order_approval::Column::WorkOrderId.eq(work_order_id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(!行.is_empty() && 行.iter().all(|a| a.status == APPROVED))
}

/// そのプロジェクトに、承認できる**他の**メンバーがいるか（設計書11.4-9）。
///
/// 判定は「`Administrator` または `Approver` を持つ、無効化されていない他のUser」。
/// **いなければ自己承認を許す。**
async fn 他に承認できる人がいる<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    自分: i32,
) -> AppResult<bool> {
    let 候補 = project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .filter(project_member::Column::Role.is_in([ADMINISTRATOR, APPROVER]))
        .filter(project_member::Column::UserId.ne(自分))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    for m in 候補 {
        let u = app_user::Entity::find_by_id(m.user_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        // 無効化された利用者は数えない。承認できないため（20.11）
        if u.is_some_and(|u| u.disabled_at.is_none()) {
            return Ok(true);
        }
    }

    Ok(false)
}

// ---------------------------------------------------------------------------
// 状態遷移（設計書11.2）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TransitionForm {
    pub to: String,
    #[serde(default)]
    pub cancelled_reason: String,
}

pub async fn transition(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, work_order_id)): Path<(i32, i32)>,
    Form(form): Form<TransitionForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let l = Locale::parse(&current.user.locale).as_str();
    let w = 取得(&state, project_id, work_order_id).await?;

    let Some(t) = Transition::parse(&form.to) else {
        let e = rust_i18n::t!("work_orders.error_transition", locale = l).to_string();
        return 詳細を描く(&state, &current, project_id, work_order_id, Some(e), None).await;
    };

    // **遷移元を必ず確かめる。**画面が出していなくてもPOSTは直接叩ける
    if !t.遷移元().contains(&w.status.as_str()) {
        let e = rust_i18n::t!("work_orders.error_transition_from", locale = l).to_string();
        return 詳細を描く(&state, &current, project_id, work_order_id, Some(e), None).await;
    }

    let 理由 = form.cancelled_reason.trim().to_owned();
    if t == Transition::Abort && 理由.is_empty() {
        // 中止の理由は必須。**「結果はどうだったのか」に答えられなくなる**（旧C-2）
        let e = rust_i18n::t!("work_orders.error_abort_reason", locale = l).to_string();
        return 詳細を描く(&state, &current, project_id, work_order_id, Some(e), None).await;
    }

    // **移譲と廃棄は、外すものが残っている間は進めさせない**（11.4、11.5）。
    // トランザクションを開く前に読む（SQLiteで自分の書き込みロックを待って止まる）
    let 妨げ = match (t, w.work_type.as_str()) {
        (Transition::Execute, TRANSFER) => 移譲を妨げるもの(&state.db, &w).await?,
        (Transition::Complete, DISPOSAL) => 廃棄を妨げるもの(&state.db, &w).await?,
        _ => None,
    };
    if let Some(key) = 妨げ {
        let e = rust_i18n::t!(key, locale = l).to_string();
        return 詳細を描く(&state, &current, project_id, work_order_id, Some(e), None).await;
    }

    // **倉庫からの払い出し**（6.3）。これもトランザクションの前に読む
    let 払い出し = t == Transition::Execute
        && w.work_type == TRANSFER
        && crate::setting::倉庫プロジェクトか(&state.db, w.project_id)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let now = Utc::now();
    let mut model = work_order::ActiveModel {
        id: Set(w.id),
        status: Set(t.遷移先().to_owned()),
        updated_at: Set(now),
        ..Default::default()
    };
    match t {
        Transition::Execute => model.executed_at = Set(Some(now)),
        Transition::Complete => model.completed_at = Set(Some(now)),
        Transition::Abort => {
            model.cancelled_at = Set(Some(now));
            model.cancelled_reason = Set(Some(理由));
        }
    }

    tx.update(&w, model)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    match t {
        // **中断したら予約を解放する**（設計書11.6）。閉じ忘れるとラックが
        // 埋まったまま残り、予約という仕組みそのものが信用されなくなる
        Transition::Abort => 予約を解放する(&tx, w.id, now).await?,
        // **実行したら予約が実機になる**（16.2のフロー）。status=planned のまま
        // だとラック図に予約中として描かれ続ける。進めるのは増設だけ（6.3）
        Transition::Execute => {
            予約を実機にする(&tx, &w, now).await?;
            // **移譲は実行で所属が移る**（11.5）
            if w.work_type == TRANSFER {
                移譲する(&tx, &w, now).await?;
            }
            if 払い出し {
                払い出す(&tx, &w, now).await?;
            }
        }
        // **廃棄は完了で所在が Disposed になる**（11.4）。移譲と違い実行では
        // 動かさない——作業の途中では、まだ現役の機器として扱う
        Transition::Complete => {
            if w.work_type == DISPOSAL {
                廃棄する(&tx, &w, now).await?;
            }
            // **修理の正常完了で故障を戻す**（6.3）。中止では戻さない——
            // 直らなかったときは中止にし、理由を書く
            if w.work_type == REPAIR {
                修理を終える(&tx, &w, now).await?;
            }
        }
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!(
        "/projects/{project_id}/work-orders/{work_order_id}"
    ))
    .into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// 中断時に予約を解放する（設計書11.6）。
///
/// **行は消さず `to_date` を閉じる**（不変条件1）。「いつまで確保していたか」も
/// 事実であり、後から経緯を追えなくなる。
async fn 予約を解放する(
    tx: &AuditedTx,
    work_order_id: i32,
    at: DateTimeUtc,
) -> AppResult<()> {
    let 予約 = device_mount::Entity::find()
        .filter(device_mount::Column::WorkOrderId.eq(work_order_id))
        .filter(device_mount::Column::ToDate.is_null())
        .all(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    for row in 予約 {
        tx.update(
            &row,
            device_mount::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(at)),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    Ok(())
}

/// 実行開始時に、予約中の機器を稼働中にする（設計書16.2のフロー）。
///
/// **増設（`Addition`）のときだけ動かす**（6.3）。移譲で進めると、倉庫からの
/// 払い出しで予約中にした機器が、同じ実行の中で稼働中になってしまう。
///
/// **`planned` のときだけ動かす。**既に `running` の機器（移設等）を勝手に
/// 書き換えない。**`health` も動かさない。**実行は「直ったこと」を意味しない。
async fn 予約を実機にする(
    tx: &AuditedTx,
    w: &work_order::Model,
    at: DateTimeUtc,
) -> AppResult<()> {
    if w.work_type != ADDITION {
        return Ok(());
    }
    let Some(device_id) = w.device_id else {
        return Ok(());
    };

    let Some(d) = device::Entity::find_by_id(device_id)
        .one(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok(());
    };

    if d.status != DEVICE_PLANNED {
        return Ok(());
    }

    tx.update(
        &d,
        device::ActiveModel {
            id: Set(d.id),
            status: Set(RUNNING.to_owned()),
            updated_at: Set(at),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(())
}

/// 修理の正常完了で、対象の `health` を `ok` に戻す（設計書6.3）。
///
/// **対象は部品があれば部品、無ければ機器。**部品の修理に載せ先の機器を
/// 添えていても、機器の値は書き換えない（機器と部品は独立に持つ）。
/// 既に `ok` なら書かない（監査ログに空の変更を残さない）。
async fn 修理を終える(
    tx: &AuditedTx,
    w: &work_order::Model,
    at: DateTimeUtc,
) -> AppResult<()> {
    use crate::device_state::OK;

    if let Some(part_id) = w.part_instance_id {
        let Some(p) = part_instance::Entity::find_by_id(part_id)
            .one(tx.reader())
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            return Ok(());
        };
        if p.health != OK {
            tx.update(
                &p,
                part_instance::ActiveModel {
                    id: Set(p.id),
                    health: Set(OK.to_owned()),
                    updated_at: Set(at),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
        return Ok(());
    }

    let Some(device_id) = w.device_id else {
        return Ok(());
    };
    let Some(d) = device::Entity::find_by_id(device_id)
        .one(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok(());
    };
    if d.health != OK {
        tx.update(
            &d,
            device::ActiveModel {
                id: Set(d.id),
                health: Set(OK.to_owned()),
                updated_at: Set(at),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    Ok(())
}

/// 機器を起票元から動かせない理由。移譲と廃棄で共通（11.4、11.5、#215、#216）。
///
/// **搭載とIPアドレスは、外すまで動かさない。**設備・什器もサブネットも
/// プロジェクトのものであり、付けたまま移すと、移譲元のラック図に他の
/// プロジェクトの機器が残り、移譲元のサブネットのアドレスを移譲先の機器が持つ。
/// 廃棄でも、廃棄した機器がラック図とアドレスの一覧に残る。
/// 黙って閉じることもしない——利用者が気づかないまま配置とアドレスが外れる。
///
/// 載っている部品は、所在が機器そのものなので、機器について動く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum 動かせない理由 {
    /// 機器がいま起票元のプロジェクトにない。
    ここにない,
    /// 設備・什器に搭載されている（予約を含む）。
    搭載,
    /// 上に他の機器が載っている。
    載せている,
    /// 現行のIPアドレスがある。
    アドレス,
    /// 載っている部品にケーブルが挿さっている（#225）。
    ケーブル,
}

async fn 機器を動かせない理由<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
    project_id: i32,
) -> AppResult<Option<動かせない理由>> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));

    // **起票元にある機器だけを動かせる。**起票の後に別のチケットで動いていれば、
    // このチケットが承認されたのはもう成り立たない前提である
    if 機器の現在のプロジェクト(db, device_id).await? != Some(project_id) {
        return Ok(Some(動かせない理由::ここにない));
    }

    // 予約（`work_order_id` つきの搭載）も現行行なので、ここで止まる
    let 搭載 = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .count(db)
        .await
        .map_err(内部)?;
    if 搭載 > 0 {
        return Ok(Some(動かせない理由::搭載));
    }

    // 棚板の上の機器や、物理ホストの上の仮想マシン（12.3、13章）
    let 載せている = device_mount::Entity::find()
        .filter(device_mount::Column::HostDeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .count(db)
        .await
        .map_err(内部)?;
    if 載せている > 0 {
        return Ok(Some(動かせない理由::載せている));
    }

    let インタフェース: Vec<i32> = os_interface::Entity::find()
        .filter(os_interface::Column::DeviceId.eq(device_id))
        .filter(os_interface::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .map(|i| i.id)
        .collect();
    if !インタフェース.is_empty() {
        let アドレス = ip_address::Entity::find()
            .filter(ip_address::Column::OsInterfaceId.is_in(インタフェース))
            .filter(ip_address::Column::ToDate.is_null())
            .count(db)
            .await
            .map_err(内部)?;
        if アドレス > 0 {
            return Ok(Some(動かせない理由::アドレス));
        }
    }

    // ケーブルは機器について移らない。挿さったまま動かすと、つながった先が
    // 別のプロジェクトの機器になる
    if super::connection::機器にケーブルがある(db, device_id).await? {
        return Ok(Some(動かせない理由::ケーブル));
    }

    Ok(None)
}

/// 移譲を実行できない理由（i18n のキー）。実行できるなら `None`（11.5、#215）。
async fn 移譲を妨げるもの<C: ConnectionTrait>(
    db: &C,
    w: &work_order::Model,
) -> AppResult<Option<&'static str>> {
    // 部品だけの移譲は、部品をプロジェクトに置けるようになってから（#219）
    let Some(device_id) = w.device_id else {
        return Ok(Some("work_orders.error_transfer_no_device"));
    };
    Ok(機器を動かせない理由(db, device_id, w.project_id)
        .await?
        .map(|r| match r {
            動かせない理由::ここにない => "work_orders.error_transfer_not_here",
            動かせない理由::搭載 => "work_orders.error_transfer_mounted",
            動かせない理由::載せている => "work_orders.error_transfer_hosting",
            動かせない理由::アドレス => "work_orders.error_transfer_has_ip",
            動かせない理由::ケーブル => "work_orders.error_transfer_has_cable",
        }))
}

/// 廃棄を完了できない理由（i18n のキー）。完了できるなら `None`（11.4、#216）。
///
/// **部品を指していれば部品を廃棄する。**機器も指していても、機器は残す——
/// 部品単位の廃棄は、機器から部品を外して捨てることである。
async fn 廃棄を妨げるもの<C: ConnectionTrait>(
    db: &C,
    w: &work_order::Model,
) -> AppResult<Option<&'static str>> {
    if let Some(part_instance_id) = w.part_instance_id {
        // **起票元にある部品だけを捨てられる。**起票元の機器に載っているか、
        // 起票元の設備・什器やプロジェクトに置いてある部品（#219）。倉庫の部品の
        // 廃棄は、倉庫用のプロジェクトで起票する（#196）
        let ここにある =
            部品の現在のプロジェクト(db, part_instance_id).await? == Some(w.project_id);
        if !ここにある {
            return Ok(Some("work_orders.error_disposal_part_not_here"));
        }
        // 挿さったままの部品は捨てられない（#225）
        if super::connection::部品にケーブルがある(db, part_instance_id).await? {
            return Ok(Some("work_orders.error_disposal_part_has_cable"));
        }
        return Ok(None);
    }

    let Some(device_id) = w.device_id else {
        return Ok(Some("work_orders.error_disposal_no_target"));
    };
    Ok(機器を動かせない理由(db, device_id, w.project_id)
        .await?
        .map(|r| match r {
            動かせない理由::ここにない => "work_orders.error_disposal_not_here",
            動かせない理由::搭載 => "work_orders.error_disposal_mounted",
            動かせない理由::載せている => "work_orders.error_disposal_hosting",
            動かせない理由::アドレス => "work_orders.error_disposal_has_ip",
            動かせない理由::ケーブル => "work_orders.error_disposal_has_cable",
        }))
}

/// 部品がいまあるプロジェクト（#219）。置き場所からたどる（[`crate::part_location`]）。
async fn 部品の現在のプロジェクト<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
) -> AppResult<Option<i32>> {
    crate::part_location::部品の現在のプロジェクト(db, part_instance_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// 機器がいま所属しているプロジェクト。プロジェクトになければ `None`。
async fn 機器の現在のプロジェクト<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Option<i32>> {
    Ok(device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|a| a.location_type == PROJECT)
        .and_then(|a| a.location_id))
}

/// 移譲の実行で、機器の所属を移譲先へ移す（11.5、#215）。
async fn 移譲する(tx: &AuditedTx, w: &work_order::Model, at: DateTimeUtc) -> AppResult<()> {
    let (Some(device_id), Some(target)) = (w.device_id, w.target_project_id) else {
        return Ok(());
    };
    所属を移す(tx, device_id, PROJECT, Some(target), w.id, at).await
}

/// 倉庫からの払い出しで、機器を予約中（`planned`）にする（設計書6.3）。
///
/// 倉庫にある間の `status` には意味が無く、`running` のままかもしれない。
/// **受け取る側の `status` を必ず設定する。**払い出した機器は予約中になり、
/// 受け取った側が編集か増設のチケットで先へ進める。`health` は動かさない。
/// 倉庫以外への移譲（倉庫への入庫を含む）では `status` を動かさない。
async fn 払い出す(tx: &AuditedTx, w: &work_order::Model, at: DateTimeUtc) -> AppResult<()> {
    let Some(device_id) = w.device_id else {
        return Ok(());
    };
    let Some(d) = device::Entity::find_by_id(device_id)
        .one(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok(());
    };
    if d.status == DEVICE_PLANNED {
        return Ok(());
    }
    tx.update(
        &d,
        device::ActiveModel {
            id: Set(d.id),
            status: Set(DEVICE_PLANNED.to_owned()),
            updated_at: Set(at),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(())
}

/// 廃棄の完了で、機器または部品の所在を `Disposed` にする（11.4、#216）。
///
/// **廃棄は `status` ではなく所在で持つ**（旧B-1）。`Disposed` は参照先を持たない。
async fn 廃棄する(tx: &AuditedTx, w: &work_order::Model, at: DateTimeUtc) -> AppResult<()> {
    if let Some(part_instance_id) = w.part_instance_id {
        return 部品の所在を移す(tx, part_instance_id, DISPOSED, None, w.id, at).await;
    }
    if let Some(device_id) = w.device_id {
        return 所属を移す(tx, device_id, DISPOSED, None, w.id, at).await;
    }
    Ok(())
}

/// 機器の所属を閉じて開く（不変条件1）。
///
/// `work_order_id` は開く行にだけ入れる——閉じる行のそれは、その行を作った
/// チケットの記録である。
async fn 所属を移す(
    tx: &AuditedTx,
    device_id: i32,
    location_type: &str,
    location_id: Option<i32>,
    work_order_id: i32,
    at: DateTimeUtc,
) -> AppResult<()> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));

    let 現在 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(tx.reader())
        .await
        .map_err(内部)?;
    for row in 現在 {
        tx.update(
            &row,
            device_assignment::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(at)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }

    tx.insert(device_assignment::ActiveModel {
        device_id: Set(device_id),
        location_type: Set(location_type.to_owned()),
        location_id: Set(location_id),
        work_order_id: Set(Some(work_order_id)),
        from_date: Set(at),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(内部)?;
    Ok(())
}

/// 部品の所在を閉じて開く（不変条件1）。`work_order_id` の扱いは [`所属を移す`] と同じ。
async fn 部品の所在を移す(
    tx: &AuditedTx,
    part_instance_id: i32,
    location_type: &str,
    location_id: Option<i32>,
    work_order_id: i32,
    at: DateTimeUtc,
) -> AppResult<()> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));

    let 現在 = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(part_instance_id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(tx.reader())
        .await
        .map_err(内部)?;
    for row in 現在 {
        tx.update(
            &row,
            part_instance_location::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(at)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }

    tx.insert(part_instance_location::ActiveModel {
        part_instance_id: Set(part_instance_id),
        location_type: Set(location_type.to_owned()),
        location_id: Set(location_id),
        // どのスロットにも挿さっていない
        chassis_slot_id: Set(None),
        work_order_id: Set(Some(work_order_id)),
        from_date: Set(at),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(内部)?;
    Ok(())
}

/// このプロジェクトから見えるチケットを取得する。
///
/// **起票元と移譲先の両方を許す**（11.5）。移譲先のメンバーが承認できなければ
/// フローが成立しない。
async fn 取得(
    state: &AppState,
    project_id: i32,
    work_order_id: i32,
) -> AppResult<work_order::Model> {
    work_order::Entity::find_by_id(work_order_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|w| w.project_id == project_id || w.target_project_id == Some(project_id))
        .ok_or(AppError::NotFound)
}

async fn 承認をまとめて引く<C: ConnectionTrait>(
    db: &C,
    tickets: &[work_order::Model],
) -> AppResult<Vec<work_order_approval::Model>> {
    if tickets.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i32> = tickets.iter().map(|w| w.id).collect();
    work_order_approval::Entity::find()
        .filter(work_order_approval::Column::WorkOrderId.is_in(ids))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// 一覧で使う氏名。**行ごとに引くとN+1になる**ためまとめて解決する。
async fn 利用者名<C: ConnectionTrait>(
    db: &C,
    tickets: &[work_order::Model],
) -> AppResult<Vec<(i32, String)>> {
    let mut ids: Vec<i32> = tickets
        .iter()
        .flat_map(|w| [w.primary_assignee_id, w.secondary_assignee_id])
        .flatten()
        .collect();
    ids.sort_unstable();
    ids.dedup();

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    Ok(app_user::Entity::find()
        .filter(app_user::Column::Id.is_in(ids))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|u| (u.id, u.name))
        .collect())
}

async fn 氏名を引く<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<String> {
    Ok(app_user::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|u| u.name)
        .unwrap_or_default())
}

async fn メンバー<C: ConnectionTrait>(db: &C, project_id: i32) -> AppResult<Vec<Labeled>> {
    let members = project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut ids: Vec<i32> = members.iter().map(|m| m.user_id).collect();
    ids.sort_unstable();
    ids.dedup();

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    Ok(app_user::Entity::find()
        .filter(app_user::Column::Id.is_in(ids))
        // 無効化された利用者は担当に選べない（20.11）
        .filter(app_user::Column::DisabledAt.is_null())
        .order_by_asc(app_user::Column::Name)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|u| Labeled {
            label: u.name,
            value: u.id.to_string(),
        })
        .collect())
}

/// 移譲先の候補。**閲覧者がメンバーであるプロジェクトに限る。**
///
/// 全プロジェクトを出すと、3章で分けたプロジェクトの存在そのものが漏れる。
async fn 他のプロジェクト(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    let members = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(current.user.id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut ids: Vec<i32> = members
        .iter()
        .map(|m| m.project_id)
        .filter(|id| *id != project_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    Ok(project::Entity::find()
        .filter(project::Column::Id.is_in(ids))
        .filter(project::Column::ArchivedAt.is_null())
        .order_by_asc(project::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|p| Labeled {
            label: p.name,
            value: p.id.to_string(),
        })
        .collect())
}

/// 対象機器の候補。件数が多いため上限を置く。
///
/// **`device_id` は任意**（11.4-2）。機器未確定のまま計画だけ先行させる例外に
/// 備えて nullable にしてある。
async fn このプロジェクトの機器(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    use entity::device_assignment;
    use sea_orm::sea_query::{Expr, Query as SeaQuery};

    let 現在ここにいる = SeaQuery::select()
        .column(device_assignment::Column::DeviceId)
        .from(device_assignment::Entity)
        .and_where(Expr::col(device_assignment::Column::LocationType).eq("Project"))
        .and_where(Expr::col(device_assignment::Column::LocationId).eq(project_id))
        .and_where(Expr::col(device_assignment::Column::ToDate).is_null())
        .to_owned();

    Ok(device::Entity::find()
        .filter(device::Column::Id.in_subquery(現在ここにいる))
        .filter(device::Column::MergedIntoDeviceId.is_null())
        .order_by_asc(device::Column::Hostname)
        .limit(500)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|d| Labeled {
            label: d.hostname,
            value: d.id.to_string(),
        })
        .collect())
}

/// 対象にできる部品の候補（設計書6.3、11.4）。**今このプロジェクトにある部品。**
///
/// このプロジェクトの機器に載っている、設備・什器に置かれている、プロジェクトに
/// 置かれている部品。判定は `part_location` と同じ（機器・設備の所在を辿る）。
async fn このプロジェクトの部品(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    let 機器: Vec<i32> = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|a| a.device_id)
        .collect();
    let 設備: Vec<i32> = mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|c| c.id)
        .collect();

    let mut どこ = Condition::any().add(
        Condition::all()
            .add(part_instance_location::Column::LocationType.eq(PROJECT))
            .add(part_instance_location::Column::LocationId.eq(project_id)),
    );
    if !機器.is_empty() {
        どこ = どこ.add(
            Condition::all()
                .add(part_instance_location::Column::LocationType.eq("Device"))
                .add(part_instance_location::Column::LocationId.is_in(機器)),
        );
    }
    if !設備.is_empty() {
        どこ = どこ.add(
            Condition::all()
                .add(part_instance_location::Column::LocationType.eq("MountContainer"))
                .add(part_instance_location::Column::LocationId.is_in(設備)),
        );
    }
    let ids: Vec<i32> = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::ToDate.is_null())
        .filter(どこ)
        .limit(500)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|l| l.part_instance_id)
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for p in part_instance::Entity::find()
        .filter(part_instance::Column::Id.is_in(ids))
        .order_by_asc(part_instance::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        out.push(Labeled {
            label: 部品の表示名(&state.db, &p).await?,
            value: p.id.to_string(),
        });
    }
    Ok(out)
}

/// 部品の表示名。「分類 型番（シリアル番号）」。
async fn 部品の表示名<C: ConnectionTrait>(
    db: &C,
    p: &part_instance::Model,
) -> AppResult<String> {
    let 型 = part_catalog::Entity::find_by_id(p.part_catalog_id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|c| format!("{} {}", c.category, c.part_number))
        .unwrap_or_default();
    Ok(match &p.serial_number {
        Some(s) => format!("{型}（{s}）"),
        None => 型,
    })
}

fn 数値(value: &str) -> Option<i32> {
    value.trim().parse().ok()
}

/// 空なら未設定、読めなければ誤り。**黙って未設定に落とさない**（Q-21）。
///
/// `2026-09-23` も `2026/09/23` も読む（[`crate::date::読む`]）。
fn 日付(value: &str) -> Result<Option<NaiveDate>, ()> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    crate::date::読む(value).map(Some).ok_or(())
}

async fn 入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<(project::Model, bool)> {
    let project = project::Entity::find_by_id(project_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    authorization::require_project_member(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let can_edit = authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .is_ok();

    Ok((project, can_edit))
}
