//! マイルストーン（設計書16.1のB領域、10.4）。
//!
//! # 予定と実績を分けて持つ（10.4）
//!
//! **`planned_date` を上書きしない。**片方に上書きすると「当初いつの予定だった
//! か」が失われ、QCDの「D」を定量的に見られなくなる。**計画と実績のズレ
//! （納期遵守率）が可視化できる**ことが、この表を置いた理由である。
//!
//! # サービス開始日・終了日は `PROJECT` の列にしない（10.4）
//!
//! `ServiceStart` / `ServiceEnd` のマイルストーンとして表す。列にすると
//! 「当初はこの日を予定していたが実際はこの日になった」という変遷を追えない。
//!
//! # 機器の増設・移設・撤去を直結する（10.4）
//!
//! `MILESTONE_DEVICE` で対象機器と結ぶ。**この計画が承認・実行されるまでは、
//! 対象機器のステータスは `status="plan"` のままである**（11.6）。

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::{NaiveDate, Utc};
use entity::{device, device_assignment, milestone, milestone_device, project};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{正規化, Labeled};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 閉じた語彙（`vocabularies.md`、設計書10.4）。
const TYPES: &[&str] = &[
    "ServiceStart",
    "ServiceUpdate",
    "ServiceMaintenance",
    "ServiceEnd",
];
const CHANGE_TYPES: &[&str] = &["addition", "relocation", "removal"];

const PLANNED: &str = "planned";
const COMPLETED: &str = "completed";
const CANCELLED: &str = "cancelled";

const PROJECT: &str = "Project";

struct MilestoneRow {
    id: i32,
    milestone_type: String,
    planned_date: String,
    actual_date: String,
    status: String,
    description: String,
    /// 対象機器（10.4）。
    devices: String,
    /// 予定と実績のズレ（日）。**遅れているときだけ出す。**
    delay: String,
    open: bool,
}

#[derive(askama::Template)]
#[template(path = "milestones.html")]
struct MilestonesPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_type: String,
    t_planned_date: String,
    t_actual_date: String,
    t_actual_hint: String,
    t_status: String,
    t_description: String,
    t_devices: String,
    t_device_hint: String,
    t_change_type: String,
    t_delay: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_complete: String,
    t_cancel: String,
    t_add_device: String,
    t_target: String,
    rows: Vec<MilestoneRow>,
    types: Vec<&'static str>,
    change_types: Vec<&'static str>,
    devices: Vec<Labeled>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    一覧を描く(&state, &current, project_id, None).await
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit, l) = 入場(state, current, project_id).await?;

    let list = milestone::Entity::find()
        .filter(milestone::Column::ProjectId.eq(project_id))
        .order_by_asc(milestone::Column::PlannedDate)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for m in list {
        // **予定と実績のズレ**（10.4）。納期遵守率を見るための核
        let delay = match m.actual_date {
            Some(actual) => {
                let 日数 = (actual - m.planned_date).num_days();
                if 日数 > 0 {
                    rust_i18n::t!("milestones.delayed", locale = l, days = 日数).to_string()
                } else {
                    String::new()
                }
            }
            None => String::new(),
        };

        rows.push(MilestoneRow {
            devices: 対象機器(state, m.id).await?,
            actual_date: m.actual_date.map(|d| d.to_string()).unwrap_or_default(),
            planned_date: m.planned_date.to_string(),
            open: m.status == PLANNED,
            delay,
            id: m.id,
            milestone_type: m.milestone_type,
            status: m.status,
            description: m.description,
        });
    }

    render(&MilestonesPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "projects"),
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("milestones.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("milestones.lead", locale = l).to_string(),
        t_type: rust_i18n::t!("milestones.milestone_type", locale = l).to_string(),
        t_planned_date: rust_i18n::t!("milestones.planned_date", locale = l).to_string(),
        t_actual_date: rust_i18n::t!("milestones.actual_date", locale = l).to_string(),
        t_actual_hint: rust_i18n::t!("milestones.actual_hint", locale = l).to_string(),
        t_status: rust_i18n::t!("work_orders.status", locale = l).to_string(),
        t_description: rust_i18n::t!("vlans.description", locale = l).to_string(),
        t_devices: rust_i18n::t!("milestones.devices", locale = l).to_string(),
        t_device_hint: rust_i18n::t!("milestones.device_hint", locale = l).to_string(),
        t_change_type: rust_i18n::t!("milestones.change_type", locale = l).to_string(),
        t_delay: rust_i18n::t!("milestones.delay", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("milestones.new", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_complete: rust_i18n::t!("milestones.complete", locale = l).to_string(),
        t_cancel: rust_i18n::t!("milestones.cancel", locale = l).to_string(),
        t_add_device: rust_i18n::t!("milestones.add_device", locale = l).to_string(),
        t_target: rust_i18n::t!("network.target", locale = l).to_string(),
        rows,
        types: TYPES.to_vec(),
        change_types: CHANGE_TYPES.to_vec(),
        devices: このプロジェクトの機器(state, project_id).await?,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct MilestoneForm {
    #[serde(default)]
    pub milestone_type: String,
    #[serde(default)]
    pub planned_date: String,
    #[serde(default)]
    pub description: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<MilestoneForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
    if !TYPES.contains(&form.milestone_type.as_str()) {
        return 一覧を描く(&state, &current, project_id, 誤り("milestones.error_type")).await;
    }
    let Some(planned) = 日付(&form.planned_date) else {
        return 一覧を描く(&state, &current, project_id, 誤り("costs.error_date")).await;
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(milestone::ActiveModel {
        project_id: Set(project_id),
        milestone_type: Set(form.milestone_type.clone()),
        planned_date: Set(planned),
        // **実績は後から入れる。**登録時点では未達である
        actual_date: Set(None),
        status: Set(PLANNED.to_owned()),
        description: Set(正規化(&form.description)),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id))
}

#[derive(Debug, Deserialize)]
pub struct CompleteForm {
    pub id: i32,
    /// 空なら今日。
    #[serde(default)]
    pub actual_date: String,
    /// `1` なら中止にする。
    #[serde(default)]
    pub cancel: String,
}

/// 実績を入れて完了にする、または中止にする（10.4）。
///
/// **`planned_date` は触らない。**当初の予定を残すことが、計画と実績のズレを
/// 見るための前提である。
pub async fn complete(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<CompleteForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    let before = このプロジェクトのマイルストーン(&state, project_id, form.id).await?;

    let 中止 = form.cancel == "1";
    let actual = if 中止 {
        // **中止に実績日は付けない。**起きなかったことに日付を残さない
        None
    } else {
        match form.actual_date.trim() {
            "" => Some(Utc::now().date_naive()),
            v => match 日付(v) {
                Some(d) => Some(d),
                None => {
                    return 一覧を描く(&state, &current, project_id, 誤り("costs.error_date")).await
                }
            },
        }
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        milestone::ActiveModel {
            id: Set(form.id),
            // **planned_date は書き換えない**（10.4）
            actual_date: Set(actual),
            status: Set(if 中止 { CANCELLED } else { COMPLETED }.to_owned()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id))
}

#[derive(Debug, Deserialize)]
pub struct DeviceForm {
    pub milestone_id: i32,
    pub device_id: i32,
    #[serde(default)]
    pub change_type: String,
}

/// 対象機器を結ぶ（10.4）。
pub async fn add_device(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<DeviceForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    このプロジェクトのマイルストーン(&state, project_id, form.milestone_id).await?;
    if !このプロジェクトの機器id(&state, project_id)
        .await?
        .contains(&form.device_id)
    {
        return Err(AppError::NotFound);
    }
    if !CHANGE_TYPES.contains(&form.change_type.as_str()) {
        return 一覧を描く(
            &state,
            &current,
            project_id,
            誤り("milestones.error_change_type"),
        )
        .await;
    }

    let 重複 = milestone_device::Entity::find()
        .filter(milestone_device::Column::MilestoneId.eq(form.milestone_id))
        .filter(milestone_device::Column::DeviceId.eq(form.device_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複.is_some() {
        return 一覧を描く(
            &state,
            &current,
            project_id,
            誤り("milestones.error_duplicate"),
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(milestone_device::ActiveModel {
        milestone_id: Set(form.milestone_id),
        device_id: Set(form.device_id),
        change_type: Set(form.change_type.clone()),
        // **WORK_ORDERへの外部キーは張らない**（24.3）
        work_order_id: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id))
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 戻り先(project_id: i32) -> Response {
    Redirect::to(&format!("/projects/{project_id}/milestones")).into_response()
}

fn 日付(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
}

async fn 入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<(project::Model, bool, &'static str)> {
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

    Ok((
        project,
        can_edit,
        Locale::parse(&current.user.locale).as_str(),
    ))
}

async fn 編集入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<&'static str> {
    let (_, can_edit, l) = 入場(state, current, project_id).await?;
    if !can_edit {
        return Err(AppError::Forbidden);
    }
    Ok(l)
}

/// **このプロジェクトのマイルストーンに限る。**IDだけを信じると、他の
/// プロジェクトの計画を書き換えられる。
async fn このプロジェクトのマイルストーン(
    state: &AppState,
    project_id: i32,
    id: i32,
) -> AppResult<milestone::Model> {
    milestone::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|m| m.project_id == project_id)
        .ok_or(AppError::NotFound)
}

async fn このプロジェクトの機器id(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<i32>> {
    Ok(device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|a| a.device_id)
        .collect())
}

async fn このプロジェクトの機器(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    let ids = このプロジェクトの機器id(state, project_id).await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(device::Entity::find()
        .filter(device::Column::Id.is_in(ids))
        .order_by_asc(device::Column::Hostname)
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

async fn 対象機器(state: &AppState, milestone_id: i32) -> AppResult<String> {
    let links = milestone_device::Entity::find()
        .filter(milestone_device::Column::MilestoneId.eq(milestone_id))
        .order_by_asc(milestone_device::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for link in links {
        let name = device::Entity::find_by_id(link.device_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|d| d.hostname)
            .unwrap_or_default();
        out.push(format!("{name} ({})", link.change_type));
    }
    Ok(out.join(", "))
}
