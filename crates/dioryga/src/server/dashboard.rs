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
use entity::{
    device, device_assignment, device_mount, milestone, mount_container, project, work_order,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::server::view::{render, 状態の表示, Chrome, Locale};
use crate::server::AppState;

/// 「期限間近」とみなす日数（10.2の「満了3ヶ月前」）。
const 期限間近: i64 = 90;

const PROJECT: &str = "Project";
const PLANNED: &str = "planned";
const COMPLETED: &str = "completed";
const CANCELLED: &str = "cancelled";

/// 指標に出す状態と、その下に添える注記のキー（#170、設計書16.4）。
///
/// **0件の状態も枠を出す。**欠けると、位置で読んでいる目が滑る。
const 指標: &[(&str, &str)] = &[
    ("running", "dashboard.note_running"),
    ("planned", "dashboard.note_planned"),
    ("provisioning", "dashboard.note_provisioning"),
    ("repairing", "dashboard.note_repairing"),
    ("failed", "dashboard.note_failed"),
];

/// **対応が必要**とみなす状態。故障と修理中（11章の起票につなげる）。
const 要対応: &[&str] = &["failed", "repairing"];

/// 指標の1枚（#170）。ランプ・項目名・台数・注記。
struct MetricCard {
    /// ランプのクラスに使う語彙の値。
    status: &'static str,
    label: String,
    count: usize,
    note: String,
}

/// 対応が必要な機器の1行（#170）。
struct AttentionRow {
    status: &'static str,
    status_label: String,
    hostname: String,
    device_type: String,
    /// 搭載先（ラック名とU番号）。載っていなければ空。
    mounted: String,
    /// 未完了のチケットの題名。無ければ空。
    work_order: String,
    href: String,
    /// チケットが無いときの導線（起票）。
    action_href: String,
    action_label: String,
}

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
    t_attention: String,
    t_hostname: String,
    t_device_type: String,
    t_mounted: String,
    t_status: String,
    t_contracts: String,
    t_milestones: String,
    t_work_orders: String,
    t_due: String,
    t_overdue: String,
    t_none: String,
    t_open: String,
    t_archived: String,
    /// 状態ごとの台数（#170）。
    metrics: Vec<MetricCard>,
    /// 故障・修理中の機器（#170）。0件なら空
    attention: Vec<AttentionRow>,
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

    let 現在の機器 = このプロジェクトの機器(&state, project_id).await?;
    let metrics = 状態ごとの台数(&現在の機器, l);
    let attention = 対応が必要な機器(&state, project_id, &現在の機器, l).await?;
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
        t_attention: rust_i18n::t!("dashboard.attention", locale = l).to_string(),
        t_hostname: rust_i18n::t!("devices.hostname", locale = l).to_string(),
        t_device_type: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_mounted: rust_i18n::t!("dashboard.mounted", locale = l).to_string(),
        t_status: rust_i18n::t!("devices.status", locale = l).to_string(),
        t_contracts: rust_i18n::t!("dashboard.contracts", locale = l).to_string(),
        t_milestones: rust_i18n::t!("dashboard.milestones", locale = l).to_string(),
        t_work_orders: rust_i18n::t!("dashboard.work_orders", locale = l).to_string(),
        t_due: rust_i18n::t!("dashboard.due", locale = l).to_string(),
        t_overdue: rust_i18n::t!("dashboard.overdue", locale = l).to_string(),
        t_none: rust_i18n::t!("dashboard.none", locale = l).to_string(),
        t_open: rust_i18n::t!("workspace.open", locale = l).to_string(),
        t_archived: rust_i18n::t!("projects.status_archived", locale = l).to_string(),
        metrics,
        attention,
        contracts,
        milestones,
        work_orders,
    })
}

/// 現在このプロジェクトにある機器を引く。
///
/// **`to_date IS NULL` で絞る。**コスト画面が使っている「過去に所属したものも
/// 含む」判定（A-6）はここでは誤り——**移管した機器を今の台数に数えてしまう。**
async fn このプロジェクトの機器(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<device::Model>> {
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
        return Ok(Vec::new());
    }

    // **1件ずつ引かない**（22章 R-4）。IDをまとめてから1回で引く
    device::Entity::find()
        .filter(device::Column::Id.is_in(ids))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// 状態ごとの台数（#170）。
///
/// **引いた機器を数えるだけで、状態ごとに問い合わせない**（22章 R-4）。
fn 状態ごとの台数(devices: &[device::Model], l: &'static str) -> Vec<MetricCard> {
    指標
        .iter()
        .map(|(status, note)| MetricCard {
            status,
            label: 状態の表示(status, l),
            count: devices.iter().filter(|d| d.status == *status).count(),
            note: rust_i18n::t!(*note, locale = l).to_string(),
        })
        .collect()
}

/// 対応が必要な機器（#170）。故障・修理中のものを、搭載先と未完了のチケットとともに出す。
///
/// **起票への導線をここに置く。**故障に気付いても、一覧を開き直して機器を
/// 探すところから始めるのでは、気付きが作業につながらない。
async fn 対応が必要な機器(
    state: &AppState,
    project_id: i32,
    devices: &[device::Model],
    l: &'static str,
) -> AppResult<Vec<AttentionRow>> {
    let 対象: Vec<&device::Model> = devices
        .iter()
        .filter(|d| 要対応.contains(&d.status.as_str()))
        .collect();
    if 対象.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<i32> = 対象.iter().map(|d| d.id).collect();

    // 搭載先。**載っていない機器もある**（仮想機器、入庫直後）
    let mounts = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.is_in(ids.clone()))
        .filter(device_mount::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let container_ids: Vec<i32> = mounts.iter().filter_map(|m| m.container_id).collect();
    let containers = if container_ids.is_empty() {
        Vec::new()
    } else {
        mount_container::Entity::find()
            .filter(mount_container::Column::Id.is_in(container_ids))
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    // 未完了のチケット。**同じ機器に複数あるときは期限の早いものを出す**
    let orders = work_order::Entity::find()
        .filter(work_order::Column::ProjectId.eq(project_id))
        .filter(work_order::Column::DeviceId.is_in(ids))
        .filter(work_order::Column::Status.is_not_in([COMPLETED, CANCELLED]))
        .order_by_asc(work_order::Column::DueDate)
        .order_by_asc(work_order::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(対象
        .into_iter()
        .map(|d| {
            let mounted = mounts
                .iter()
                .find(|m| m.device_id == d.id)
                .map(|m| {
                    let 什器 = m
                        .container_id
                        .and_then(|id| containers.iter().find(|c| c.id == id))
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    match m.position {
                        Some(u) => format!("{什器} / {u}"),
                        None => 什器,
                    }
                })
                .unwrap_or_default();
            let 担当のチケット = orders.iter().find(|w| w.device_id == Some(d.id));
            AttentionRow {
                status: 要対応
                    .iter()
                    .find(|s| **s == d.status)
                    .copied()
                    .unwrap_or("failed"),
                status_label: 状態の表示(&d.status, l),
                hostname: d.hostname.clone(),
                device_type: d.device_type.clone(),
                mounted,
                work_order: 担当のチケット.map(|w| w.title.clone()).unwrap_or_default(),
                href: format!("/projects/{project_id}/devices/{}", d.id),
                action_href: match 担当のチケット {
                    Some(w) => format!("/projects/{project_id}/work-orders/{}", w.id),
                    None => format!("/projects/{project_id}/work-orders/new"),
                },
                action_label: match 担当のチケット {
                    Some(_) => rust_i18n::t!("dashboard.open_work_order", locale = l).to_string(),
                    None => rust_i18n::t!("dashboard.create_work_order", locale = l).to_string(),
                },
            }
        })
        .collect())
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
        .filter(work_order::Column::Status.is_not_in([COMPLETED, CANCELLED]))
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
