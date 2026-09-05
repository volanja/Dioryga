//! プロジェクトのメンバー管理（設計書16.1のB領域、5章）。
//!
//! # 扱うのは Operator / Approver / Viewer だけ
//!
//! **`Administrator` はここで扱わない**（5章）。3章が「プロジェクトへの
//! Project Administratorの割り当て」をSystem Adminの職務としており、ここでも
//! 変えられるようにすると、**正・副の一意性を守る責任者が2箇所に分かれる。**
//! 「副だけを割り当てる」「同一人物を正副に置く」を拒否する判定が2箇所にあると、
//! 片方を通してもう片方の前提が崩れる経路ができる。
//!
//! 現在の管理者は読み取り専用で見せ、変更はSystem Adminへ依頼する。
//!
//! # 兼務を許す
//!
//! `PROJECT_MEMBER` の粒度は `(user_id, project_id, role)` であり、**同一利用者が
//! 同一プロジェクトで複数ロールを持てる**（5章、旧C-8）。OperatorとApproverの
//! 兼務は普通にある。したがって画面もラジオボタンではなくチェックボックスで、
//! 「持っているロールの集合」を編集する形にする。
//!
//! # 行を消すが、履歴は失われない
//!
//! ロールを外すと `PROJECT_MEMBER` の行は消える。誰がいつ外したかは
//! `AUDIT_LOG` が持つ（5.1で `created_by` を持たないとしたのと同じ理由）。

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{app_user, project, project_member};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization::{self, ADMINISTRATOR, APPROVER, OPERATOR, VIEWER};
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// この画面で編集できるロール。**`Administrator` を含めない**（5章）。
const 編集できるロール: &[&str] = &[OPERATOR, APPROVER, VIEWER];

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct MemberRow {
    user_id: i32,
    name: String,
    email: String,
    operator: bool,
    approver: bool,
    viewer: bool,
    /// この画面では変えられない（5章）。読み取り専用で見せる。
    admin_rank: String,
    /// 無効化された利用者。**一覧には残す。**消すと外せなくなる（20.11）。
    disabled: bool,
}

struct Candidate {
    id: i32,
    label: String,
}

#[derive(askama::Template)]
#[template(path = "members.html")]
struct MembersPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_admin_note: String,
    t_name: String,
    t_email: String,
    t_admin: String,
    t_operator: String,
    t_approver: String,
    t_viewer: String,
    t_actions: String,
    t_save: String,
    t_empty: String,
    t_disabled: String,
    t_add: String,
    t_add_lead: String,
    t_user: String,
    t_no_candidate: String,
    rows: Vec<MemberRow>,
    candidates: Vec<Candidate>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    描く(&state, &current, project_id, None).await
}

async fn 描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let project = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    // **メンバーのロール変更ができるのはAdministratorだけ**（3章の権限マトリクス）
    let can_edit =
        authorization::require_project_role(&state.db, &current.user, project_id, &[ADMINISTRATOR])
            .await
            .is_ok();

    let rows = 一覧(&state.db, project_id).await?;
    let candidates = if can_edit {
        追加候補(&state.db, &rows).await?
    } else {
        Vec::new()
    };

    render(&MembersPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "projects"),
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("members.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("members.lead", locale = l).to_string(),
        t_admin_note: rust_i18n::t!("members.admin_note", locale = l).to_string(),
        t_name: rust_i18n::t!("members.name", locale = l).to_string(),
        t_email: rust_i18n::t!("members.email", locale = l).to_string(),
        t_admin: rust_i18n::t!("members.administrator", locale = l).to_string(),
        t_operator: rust_i18n::t!("members.operator", locale = l).to_string(),
        t_approver: rust_i18n::t!("members.approver", locale = l).to_string(),
        t_viewer: rust_i18n::t!("members.viewer", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_save: rust_i18n::t!("members.save", locale = l).to_string(),
        t_empty: rust_i18n::t!("members.empty", locale = l).to_string(),
        t_disabled: rust_i18n::t!("members.disabled", locale = l).to_string(),
        t_add: rust_i18n::t!("members.add", locale = l).to_string(),
        t_add_lead: rust_i18n::t!("members.add_lead", locale = l).to_string(),
        t_user: rust_i18n::t!("members.user", locale = l).to_string(),
        t_no_candidate: rust_i18n::t!("members.no_candidate", locale = l).to_string(),
        rows,
        candidates,
        can_edit,
        error,
    })
}

// ---------------------------------------------------------------------------
// 更新
// ---------------------------------------------------------------------------

/// チェックボックスは**チェックされたときだけ送られてくる。**
/// したがって `Option` で受け、無ければ外すと解釈する。
#[derive(Debug, Deserialize)]
pub struct UpdateForm {
    pub user_id: i32,
    #[serde(default)]
    pub operator: Option<String>,
    #[serde(default)]
    pub approver: Option<String>,
    #[serde(default)]
    pub viewer: Option<String>,
}

pub async fn update(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<UpdateForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_role(&state.db, &current.user, project_id, &[ADMINISTRATOR])
        .await
        .map_err(|_| AppError::Forbidden)?;

    let l = Locale::parse(&current.user.locale).as_str();

    let target = app_user::Entity::find_by_id(form.user_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    // **System Adminは割り当てない**（5章）。ロールを持っても3章により
    // プロジェクトデータへアクセスできず、割り当てると画面と挙動が食い違う
    if target.is_system_admin {
        let e = rust_i18n::t!("members.error_system_admin", locale = l).to_string();
        return 描く(&state, &current, project_id, Some(e)).await;
    }

    let 希望: Vec<&str> = [
        (OPERATOR, form.operator.is_some()),
        (APPROVER, form.approver.is_some()),
        (VIEWER, form.viewer.is_some()),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(role, _)| role)
    .collect();

    // 既に無効化された利用者を**新規に**追加はしない。既存メンバーの整理は通す
    let 現在 = 保有ロール(&state.db, project_id, target.id).await?;
    let 新規追加 = 現在.is_empty() && !希望.is_empty();
    if 新規追加 && target.disabled_at.is_some() {
        let e = rust_i18n::t!("members.error_disabled", locale = l).to_string();
        return 描く(&state, &current, project_id, Some(e)).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **触るのは編集できる3ロールの行だけ。**Administratorの行には手を出さない
    let 既存 = project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .filter(project_member::Column::UserId.eq(target.id))
        .filter(project_member::Column::Role.is_in(編集できるロール.to_vec()))
        .all(tx.reader())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    for row in 既存.iter().filter(|r| !希望.contains(&r.role.as_str())) {
        tx.delete(row.clone())
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    let now = Utc::now();
    for role in 希望.iter().filter(|r| !既存.iter().any(|e| e.role == **r)) {
        tx.insert(project_member::ActiveModel {
            user_id: Set(target.id),
            project_id: Set(project_id),
            role: Set((*role).to_owned()),
            // **この画面では Administrator を扱わないため常に null**（5章）
            admin_rank: Set(None),
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

    Ok(Redirect::to(&format!("/projects/{project_id}/members")).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 一覧<C: ConnectionTrait>(db: &C, project_id: i32) -> AppResult<Vec<MemberRow>> {
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

    let users = app_user::Entity::find()
        .filter(app_user::Column::Id.is_in(ids))
        .order_by_asc(app_user::Column::Name)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(users
        .into_iter()
        .map(|u| {
            let 保有 = |role: &str| members.iter().any(|m| m.user_id == u.id && m.role == role);
            MemberRow {
                operator: 保有(OPERATOR),
                approver: 保有(APPROVER),
                viewer: 保有(VIEWER),
                admin_rank: members
                    .iter()
                    .find(|m| m.user_id == u.id && m.role == ADMINISTRATOR)
                    .and_then(|m| m.admin_rank.clone())
                    .unwrap_or_default(),
                disabled: u.disabled_at.is_some(),
                user_id: u.id,
                name: u.name,
                email: u.email,
            }
        })
        .collect())
}

/// 追加できる利用者。
///
/// **System Adminと無効化された利用者は出さない**（5章、20.11）。既にこの
/// プロジェクトに何らかのロールを持つ人も、行が既に一覧にあるため出さない。
async fn 追加候補<C: ConnectionTrait>(
    db: &C, 既存: &[MemberRow]
) -> AppResult<Vec<Candidate>> {
    let users = app_user::Entity::find()
        .filter(app_user::Column::IsSystemAdmin.eq(false))
        .filter(app_user::Column::DisabledAt.is_null())
        .order_by_asc(app_user::Column::Name)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(users
        .into_iter()
        .filter(|u| !既存.iter().any(|r| r.user_id == u.id))
        .map(|u| Candidate {
            id: u.id,
            label: format!("{}（{}）", u.name, u.email),
        })
        .collect())
}

async fn 保有ロール<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    user_id: i32,
) -> AppResult<Vec<String>> {
    Ok(project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .filter(project_member::Column::UserId.eq(user_id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|m| m.role)
        .collect())
}

async fn 入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<project::Model> {
    let project = project::Entity::find_by_id(project_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    authorization::require_project_member(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    Ok(project)
}
