//! プロジェクトダッシュボード（設計書16.1のB領域、16.3）。
//!
//! # プロジェクトの入口
//!
//! `/projects/{id}` は16.3で定義されていたが、ルートが無く、一覧からは機器一覧へ
//! 直接飛んでいた。**プロジェクトを開いたときに最初に見るのは、個々の機器では
//! なく全体の状況である。**
//!
//! # 出すのは4つ
//!
//! 16.1の通り、機器数・期限間近の契約・期限間近のマイルストーン・未完了の
//! 変更管理チケット。**いずれも値を保存せず、都度計算する**（本設計全体の
//! 一貫方針、10.3）。
//!
//! # 「期限間近」は90日
//!
//! 10.2が保守契約の満了通知を「3ヶ月前」としているのに合わせる。
//! **マイルストーンにも同じ窓を使う。**画面の中で窓の広さが項目ごとに違うと、
//! 並んだ数字を比べられない。
//!
//! **期限を過ぎたものも同じ枠に入れる。**切れた契約・遅れたマイルストーンこそ
//! 最初に気付くべきもので、窓から外すと**過ぎた瞬間に画面から消える。**
//!
//! # 集計を1画面に並べることの注意
//!
//! 22章（R-4）が「ダッシュボードは重いクエリを1画面に複数並べる構造になる」と
//! 指摘している。**機器を1件ずつ引かない。**IDをまとめてから `is_in` で1回引く。

use axum::extract::{Path, State};
use axum::response::Response;
use axum::Extension;
use chrono::{Duration, NaiveDate, Utc};
use entity::{device, device_assignment, milestone, project, work_order};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 「期限間近」とみなす日数（10.2の「満了3ヶ月前」）。
const 期限間近: i64 = 90;

const PROJECT: &str = "Project";
const PLAN: &str = "plan";
const PLANNED: &str = "planned";
const COMPLETED: &str = "completed";
const ABORTED: &str = "aborted";

/// 期限を持つ行の共通の見せ方。
struct DueRow {
    label: String,
    sub: String,
    due: String,
    /// **期限を過ぎている。**画面側で強調する。
    overdue: bool,
    href: String,
}

#[derive(askama::Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    chrome: Chrome,
    project_name: String,
    project_code: String,
    archived: bool,
    t_lead: String,
    t_window: String,
    t_devices_running: String,
    t_devices_planned: String,
    t_contracts: String,
    t_milestones: String,
    t_work_orders: String,
    t_due: String,
    t_overdue: String,
    t_none: String,
    t_open: String,
    t_archived: String,
    /// 現在このプロジェクトにある機器の台数。
    running: usize,
    /// うち `status=plan`（予約中）。
    planned: usize,
    contracts: Vec<DueRow>,
    milestones: Vec<DueRow>,
    work_orders: Vec<DueRow>,
}

pub async fn show(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    let project = project::Entity::find_by_id(project_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    authorization::require_project_member(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;
    let l = Locale::parse(&current.user.locale).as_str();

    let today = Utc::now().date_naive();
    let 期限 = today + Duration::days(期限間近);

    let (running, planned) = 機器の台数(&state, project_id).await?;
    let contracts = 期限間近の契約(&state, project_id, 期限, today).await?;
    let milestones = 期限間近のマイルストーン(&state, project_id, 期限, today).await?;
    let work_orders = 未完了のチケット(&state, project_id, today, l).await?;

    render(&DashboardPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "dashboard",
        )
        .await,
        project_name: project.name,
        project_code: project.code.unwrap_or_default(),
        archived: project.archived_at.is_some(),
        t_lead: rust_i18n::t!("dashboard.lead", locale = l).to_string(),
        t_window: rust_i18n::t!("dashboard.window", locale = l, days = 期限間近).to_string(),
        t_devices_running: rust_i18n::t!("dashboard.devices_running", locale = l).to_string(),
        t_devices_planned: rust_i18n::t!("dashboard.devices_planned", locale = l).to_string(),
        t_contracts: rust_i18n::t!("dashboard.contracts", locale = l).to_string(),
        t_milestones: rust_i18n::t!("dashboard.milestones", locale = l).to_string(),
        t_work_orders: rust_i18n::t!("dashboard.work_orders", locale = l).to_string(),
        t_due: rust_i18n::t!("dashboard.due", locale = l).to_string(),
        t_overdue: rust_i18n::t!("dashboard.overdue", locale = l).to_string(),
        t_none: rust_i18n::t!("dashboard.none", locale = l).to_string(),
        t_open: rust_i18n::t!("workspace.open", locale = l).to_string(),
        t_archived: rust_i18n::t!("projects.status_archived", locale = l).to_string(),
        running,
        planned,
        contracts,
        milestones,
        work_orders,
    })
}

/// 現在このプロジェクトにある機器を数える。
///
/// **`to_date IS NULL` で絞る。**コスト画面が使っている「過去に所属したものも
/// 含む」判定（A-6）はここでは誤り——**移管した機器を今の台数に数えてしまう。**
///
/// 予約中（`status=plan`）は分けて返す。12.5の電力集計で予約中を分けたのと
/// 同じ考え方で、現在の姿と増設後の見通しの両方を出せるようにする（11.6）。
async fn 機器の台数(state: &AppState, project_id: i32) -> AppResult<(usize, usize)> {
    let ids: Vec<i32> = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|a| a.device_id)
        .collect();

    if ids.is_empty() {
        return Ok((0, 0));
    }

    // **1件ずつ引かない**（22章 R-4）。IDをまとめてから1回で引く
    let devices = device::Entity::find()
        .filter(device::Column::Id.is_in(ids))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let planned = devices.iter().filter(|d| d.status == PLAN).count();
    Ok((devices.len() - planned, planned))
}

/// 満了が近い保守契約（10.2）。
///
/// **契約はプロジェクトを直接持たない。**`MAINTENANCE_CONTRACT_ITEM` から機器を
/// 経由して辿る（10.2）。ここは `crate::server::cost` の判定をそのまま使う——
/// **コスト画面に出る契約と、ここに出る契約が食い違ってはならない。**
async fn 期限間近の契約(
    state: &AppState,
    project_id: i32,
    期限: NaiveDate,
    today: NaiveDate,
) -> AppResult<Vec<DueRow>> {
    let mut rows: Vec<DueRow> =
        crate::server::cost::このプロジェクトの契約(state, project_id)
            .await?
            .into_iter()
            .filter(|c| c.end_date <= 期限)
            .map(|c| DueRow {
                label: c.contract_number,
                sub: String::new(),
                due: c.end_date.to_string(),
                overdue: c.end_date < today,
                href: format!("/projects/{project_id}/costs/maintenance-contracts"),
            })
            .collect();
    // **満了の早い順。**`ContractNumber` 順で来るため、ここで並べ直す
    rows.sort_by(|a, b| a.due.cmp(&b.due));
    Ok(rows)
}

/// 予定日が近い、未完了のマイルストーン（10.4）。
///
/// **`status=planned` のものだけを見る。**完了・中止したものに期限は無い。
/// **予定日で判定する**——実績日は完了して初めて入るため、未完了の行では常に空。
async fn 期限間近のマイルストーン(
    state: &AppState,
    project_id: i32,
    期限: NaiveDate,
    today: NaiveDate,
) -> AppResult<Vec<DueRow>> {
    let list = milestone::Entity::find()
        .filter(milestone::Column::ProjectId.eq(project_id))
        .filter(milestone::Column::Status.eq(PLANNED))
        .filter(milestone::Column::PlannedDate.lte(期限))
        .order_by_asc(milestone::Column::PlannedDate)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(list
        .into_iter()
        .map(|m| DueRow {
            label: m.milestone_type,
            sub: m.description,
            due: m.planned_date.to_string(),
            overdue: m.planned_date < today,
            href: format!("/projects/{project_id}/milestones"),
        })
        .collect())
}

/// 未完了の変更管理チケット（11章）。
///
/// **期限で絞らない。**16.1が求めているのは「未完了のサマリ」であり、
/// **期限が未設定のチケットが最も見落とされやすい。**窓を掛けると消える。
///
/// `due_date` を持つものを先に、期限の早い順に並べる。
async fn 未完了のチケット(
    state: &AppState,
    project_id: i32,
    today: NaiveDate,
    l: &'static str,
) -> AppResult<Vec<DueRow>> {
    let list = work_order::Entity::find()
        .filter(work_order::Column::ProjectId.eq(project_id))
        .filter(work_order::Column::Status.is_not_in([COMPLETED, ABORTED]))
        .order_by_asc(work_order::Column::DueDate)
        .order_by_asc(work_order::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let 期限なし = rust_i18n::t!("dashboard.no_due", locale = l).to_string();
    Ok(list
        .into_iter()
        .map(|w| DueRow {
            due: match w.due_date {
                Some(d) => d.to_string(),
                None => 期限なし.clone(),
            },
            overdue: w.due_date.is_some_and(|d| d < today),
            href: format!("/projects/{project_id}/work-orders/{}", w.id),
            label: w.title,
            sub: format!("{} / {}", w.work_type, w.status),
        })
        .collect())
}
