//! プロジェクト領域の入口（設計書16.1のB領域、3章）。
//!
//! ここから先は**ロールでアクセス制御される**。System Adminはそもそも入れない
//! （`system_admin_guard` が `/projects/**` を拒否する）。
//!
//! # 見えるのは所属するプロジェクトだけ
//!
//! 一覧に出すのは `PROJECT_MEMBER` を持つプロジェクトに限る。System Admin領域の
//! プロジェクト一覧（全件）とは別物である。

use axum::extract::State;
use axum::response::Response;
use axum::Extension;
use entity::{project, project_member};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

struct ProjectRow {
    id: i32,
    name: String,
    code: String,
    roles: String,
    archived: bool,
}

#[derive(askama::Template)]
#[template(path = "projects.html")]
struct ProjectsPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_name: String,
    t_code: String,
    t_roles: String,
    t_empty: String,
    t_archived: String,
    rows: Vec<ProjectRow>,
}

/// 自分が所属するプロジェクトの一覧。
pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();

    // 所属しているプロジェクトのIDを引く。**複数ロールを兼務しうる**ため
    // 重複が出る。distinct で潰す（設計書5章）
    let ids: Vec<i32> = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(current.user.id))
        .select_only()
        .column(project_member::Column::ProjectId)
        .distinct()
        .into_tuple()
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let projects = if ids.is_empty() {
        Vec::new()
    } else {
        project::Entity::find()
            .filter(project::Column::Id.is_in(ids))
            .order_by_asc(project::Column::Name)
            .order_by_asc(project::Column::Id)
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    // ロールは1プロジェクトにつき複数ありうるので、まとめて引いて連結する
    let memberships = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(current.user.id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let rows = projects
        .into_iter()
        .map(|p| {
            let mut roles: Vec<String> = memberships
                .iter()
                .filter(|m| m.project_id == p.id)
                .map(|m| match m.admin_rank.as_deref() {
                    Some(rank) => format!("{}（{}）", m.role, rank),
                    None => m.role.clone(),
                })
                .collect();
            roles.sort();

            ProjectRow {
                code: p.code.unwrap_or_default(),
                roles: roles.join(" / "),
                archived: p.archived_at.is_some(),
                id: p.id,
                name: p.name,
            }
        })
        .collect();

    render(&ProjectsPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "projects"),
        t_title: rust_i18n::t!("nav.projects", locale = l).to_string(),
        t_lead: rust_i18n::t!("workspace.lead", locale = l).to_string(),
        t_name: rust_i18n::t!("projects.name", locale = l).to_string(),
        t_code: rust_i18n::t!("projects.code", locale = l).to_string(),
        t_roles: rust_i18n::t!("workspace.roles", locale = l).to_string(),
        t_empty: rust_i18n::t!("workspace.empty", locale = l).to_string(),
        t_archived: rust_i18n::t!("projects.status_archived", locale = l).to_string(),
        rows,
    })
}
