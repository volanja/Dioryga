//! System Admin領域のプロジェクト管理（設計書16.1のA領域、5章）。
//!
//! # 中身は見えない
//!
//! System Adminが扱えるのは**プロジェクトの器とAdministratorの割り当てまで**で、
//! 中のデータには触れられない（3章）。この画面に機器数やコストを出さないのは
//! そのため。
//!
//! # 日付と進行状態を持たない
//!
//! サービスの開始・終了は `MILESTONE` が持ち、進行中／完了はそこから導出する
//! （5.1）。ここで扱う `archived_at` は**一覧から外すという運用判断**であって
//! サービス上の終了ではない（5.2）。両者を混ぜないこと。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{app_user, project, project_member};
use sea_orm::sea_query::{Expr, Func, LikeExpr};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::authorization::ADMINISTRATOR;
use crate::auth::middleware::CurrentUser;
use crate::currency;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// `PROJECT_MEMBER.admin_rank`（設計書5章）。
const PRIMARY: &str = "Primary";
const SECONDARY: &str = "Secondary";

/// `PROJECT.closure_reason`（設計書5.2）。
const COMPLETED: &str = "Completed";
const CANCELLED: &str = "Cancelled";

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct ProjectRow {
    id: i32,
    name: String,
    code: String,
    currency: String,
    primary_admin: String,
    secondary_admin: String,
    status: String,
    archived: bool,
}

#[derive(askama::Template)]
#[template(path = "admin_projects.html")]
struct ProjectsPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_new: String,
    t_keyword: String,
    t_search: String,
    t_name: String,
    t_code: String,
    t_currency: String,
    t_primary_admin: String,
    t_secondary_admin: String,
    t_status: String,
    t_status_all: String,
    t_status_active: String,
    t_status_archived: String,
    t_actions: String,
    t_edit: String,
    t_members: String,
    t_archive: String,
    t_unarchive: String,
    t_empty: String,
    q: String,
    status: String,
    rows: Vec<ProjectRow>,
    error: Option<String>,
}

#[derive(askama::Template)]
#[template(path = "admin_project_form.html")]
struct ProjectFormPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_name: String,
    t_code: String,
    t_code_hint: String,
    t_description: String,
    t_currency: String,
    t_currency_hint: String,
    t_submit: String,
    action: String,
    name: String,
    code: String,
    description: String,
    currency: String,
    currencies: Vec<&'static str>,
    error: Option<String>,
}

#[derive(askama::Template)]
#[template(path = "admin_project_archive.html")]
struct ArchivePage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_reason: String,
    t_reason_hint: String,
    t_completed: String,
    t_cancelled: String,
    t_submit: String,
    project_id: i32,
    project_name: String,
    error: Option<String>,
}

struct Candidate {
    id: i32,
    label: String,
}

#[derive(askama::Template)]
#[template(path = "admin_project_members.html")]
struct MembersPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_primary: String,
    t_secondary: String,
    t_secondary_hint: String,
    t_unassigned: String,
    t_submit: String,
    /// 候補が0件のときの説明（#90）。**空なのが実態か設定漏れかを判別できるように。**
    t_no_candidate: String,
    t_go_users: String,
    project_id: i32,
    project_name: String,
    candidates: Vec<Candidate>,
    primary_id: Option<i32>,
    secondary_id: Option<i32>,
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
}

/// 絞り込みの状態。既定は「進行中」。
///
/// 16.1の通り、**アーカイブ済みを隠せるフィルタは必須**とする。5.2でアーカイブの
/// 概念を置いたのは、まさに行うべき作業に集中するためである。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    Active,
    Archived,
    All,
}

impl StatusFilter {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("archived") => Self::Archived,
            Some("all") => Self::All,
            _ => Self::Active,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
            Self::All => "all",
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let page = build_list(&state, &current, &query, None).await?;
    render(&page)
}

async fn build_list(
    state: &AppState,
    current: &CurrentUser,
    query: &ListQuery,
    error: Option<String>,
) -> AppResult<ProjectsPage> {
    let l = Locale::parse(&current.user.locale).as_str();
    let status = StatusFilter::parse(query.status.as_deref());
    let keyword = query.q.clone().unwrap_or_default();

    let projects = 検索(&state.db, &keyword, status)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // Administratorをまとめて1回で引く。行ごとに問い合わせると、
    // プロジェクトが増えたときに一覧を開くだけでN+1になる。
    let ids: Vec<i32> = projects.iter().map(|p| p.id).collect();
    let admins = 管理者一覧(&state.db, &ids)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // 未割当は列の値として各行に入れる（見出しではないためテンプレートに渡さない）
    let 未割当 = rust_i18n::t!("projects.unassigned", locale = l).to_string();

    let rows = projects
        .into_iter()
        .map(|p| {
            let entry = admins.get(&p.id);
            ProjectRow {
                code: p.code.clone().unwrap_or_default(),
                primary_admin: entry
                    .and_then(|a| a.0.clone())
                    .unwrap_or_else(|| 未割当.clone()),
                secondary_admin: entry
                    .and_then(|a| a.1.clone())
                    .unwrap_or_else(|| 未割当.clone()),
                status: match (&p.archived_at, p.closure_reason.as_deref()) {
                    (None, _) => rust_i18n::t!("projects.status_active", locale = l).to_string(),
                    (Some(_), Some(CANCELLED)) => {
                        rust_i18n::t!("projects.reason_cancelled", locale = l).to_string()
                    }
                    (Some(_), _) => {
                        rust_i18n::t!("projects.reason_completed", locale = l).to_string()
                    }
                },
                archived: p.archived_at.is_some(),
                id: p.id,
                name: p.name,
                currency: p.currency,
            }
        })
        .collect();

    Ok(ProjectsPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "admin_projects"),
        t_title: rust_i18n::t!("projects.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("projects.lead", locale = l).to_string(),
        t_new: rust_i18n::t!("projects.new", locale = l).to_string(),
        t_keyword: rust_i18n::t!("projects.keyword", locale = l).to_string(),
        t_search: rust_i18n::t!("common.search", locale = l).to_string(),
        t_name: rust_i18n::t!("projects.name", locale = l).to_string(),
        t_code: rust_i18n::t!("projects.code", locale = l).to_string(),
        t_currency: rust_i18n::t!("projects.currency", locale = l).to_string(),
        t_primary_admin: rust_i18n::t!("projects.primary_admin", locale = l).to_string(),
        t_secondary_admin: rust_i18n::t!("projects.secondary_admin", locale = l).to_string(),
        t_status: rust_i18n::t!("projects.status", locale = l).to_string(),
        t_status_all: rust_i18n::t!("projects.status_all", locale = l).to_string(),
        t_status_active: rust_i18n::t!("projects.status_active", locale = l).to_string(),
        t_status_archived: rust_i18n::t!("projects.status_archived", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_edit: rust_i18n::t!("projects.edit", locale = l).to_string(),
        t_members: rust_i18n::t!("projects.members", locale = l).to_string(),
        t_archive: rust_i18n::t!("projects.archive", locale = l).to_string(),
        t_unarchive: rust_i18n::t!("projects.unarchive", locale = l).to_string(),
        t_empty: rust_i18n::t!("projects.empty", locale = l).to_string(),
        q: keyword,
        status: status.as_str().to_owned(),
        rows,
        error,
    })
}

/// 表示名またはコードの部分一致で絞り込む。
///
/// `LOWER()` を両側に掛けるのは利用者一覧と同じ理由（PostgreSQLの `LIKE` は
/// 大文字小文字を区別し、SQLiteのそれはASCIIに限り区別しないため）。
async fn 検索<C: ConnectionTrait>(
    db: &C,
    keyword: &str,
    status: StatusFilter,
) -> Result<Vec<project::Model>, sea_orm::DbErr> {
    let mut query = project::Entity::find();

    match status {
        StatusFilter::Active => query = query.filter(project::Column::ArchivedAt.is_null()),
        StatusFilter::Archived => query = query.filter(project::Column::ArchivedAt.is_not_null()),
        StatusFilter::All => {}
    }

    let keyword = keyword.trim();
    if !keyword.is_empty() {
        let pattern = format!("%{}%", escape_like(&keyword.to_lowercase()));
        query = query.filter(
            Expr::expr(Func::lower(Expr::col(project::Column::Name)))
                .like(LikeExpr::new(&pattern).escape('\\'))
                .or(Expr::expr(Func::lower(Expr::col(project::Column::Code)))
                    .like(LikeExpr::new(&pattern).escape('\\'))),
        );
    }

    query
        .order_by_asc(project::Column::Name)
        .order_by_asc(project::Column::Id)
        .all(db)
        .await
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// プロジェクトIDごとの (正管理者名, 副管理者名)。
async fn 管理者一覧<C: ConnectionTrait>(
    db: &C,
    project_ids: &[i32],
) -> Result<HashMap<i32, (Option<String>, Option<String>)>, sea_orm::DbErr> {
    let mut map: HashMap<i32, (Option<String>, Option<String>)> = HashMap::new();
    if project_ids.is_empty() {
        return Ok(map);
    }

    let rows = project_member::Entity::find()
        .filter(project_member::Column::ProjectId.is_in(project_ids.to_vec()))
        .filter(project_member::Column::Role.eq(ADMINISTRATOR))
        .find_also_related(app_user::Entity)
        .all(db)
        .await?;

    for (member, user) in rows {
        let Some(user) = user else { continue };
        let entry = map.entry(member.project_id).or_default();
        match member.admin_rank.as_deref() {
            Some(PRIMARY) => entry.0 = Some(user.name),
            Some(SECONDARY) => entry.1 = Some(user.name),
            _ => {}
        }
    }

    Ok(map)
}

// ---------------------------------------------------------------------------
// 登録・編集
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ProjectForm {
    pub name: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub currency: String,
}

pub async fn new_form(Extension(current): Extension<CurrentUser>) -> AppResult<Response> {
    render(&登録画面(
        &current,
        &ProjectForm {
            name: String::new(),
            code: String::new(),
            description: String::new(),
            currency: currency::DEFAULT.to_owned(),
        },
        None,
    ))
}

fn 画面(
    current: &CurrentUser,
    form: &ProjectForm,
    action: String,
    title_key: &str,
    lead_key: &str,
    submit_key: &str,
    error: Option<String>,
) -> ProjectFormPage {
    let l = Locale::parse(&current.user.locale).as_str();
    ProjectFormPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "admin_projects"),
        t_title: rust_i18n::t!(title_key, locale = l).to_string(),
        t_lead: rust_i18n::t!(lead_key, locale = l).to_string(),
        t_back: rust_i18n::t!("common.back", locale = l).to_string(),
        t_name: rust_i18n::t!("projects.name", locale = l).to_string(),
        t_code: rust_i18n::t!("projects.code", locale = l).to_string(),
        t_code_hint: rust_i18n::t!("projects.code_hint", locale = l).to_string(),
        t_description: rust_i18n::t!("projects.description", locale = l).to_string(),
        t_currency: rust_i18n::t!("projects.currency", locale = l).to_string(),
        t_currency_hint: rust_i18n::t!("projects.currency_hint", locale = l).to_string(),
        t_submit: rust_i18n::t!(submit_key, locale = l).to_string(),
        action,
        name: form.name.clone(),
        code: form.code.clone(),
        description: form.description.clone(),
        currency: currency::normalize(&form.currency),
        currencies: currency::SUPPORTED.iter().map(|c| c.code).collect(),
        error,
    }
}

fn 登録画面(
    current: &CurrentUser,
    form: &ProjectForm,
    error: Option<String>,
) -> ProjectFormPage {
    画面(
        current,
        form,
        "/admin/projects".to_owned(),
        "projects.new_title",
        "projects.new_lead",
        "common.create",
        error,
    )
}

fn 編集画面(
    current: &CurrentUser,
    id: i32,
    form: &ProjectForm,
    error: Option<String>,
) -> ProjectFormPage {
    画面(
        current,
        form,
        format!("/admin/projects/{id}"),
        "projects.edit_title",
        "projects.edit_lead",
        "common.save",
        error,
    )
}

/// 入力を検証し、正規化した (name, code) を返す。
async fn 検証(
    state: &AppState,
    form: &ProjectForm,
    locale: &str,
    自分自身: Option<i32>,
) -> AppResult<Result<(String, Option<String>), String>> {
    let name = form.name.trim().to_owned();
    if name.is_empty() {
        return Ok(Err(rust_i18n::t!(
            "projects.name_required",
            locale = locale
        )
        .to_string()));
    }

    // コードは任意。空文字ではなくNULLで持つ——UNIQUE制約があるため、
    // 空文字にすると「コード未設定のプロジェクト」が2件目から作れなくなる。
    let code = match form.code.trim() {
        "" => None,
        value => Some(value.to_owned()),
    };

    if let Some(code) = &code {
        let mut query = project::Entity::find().filter(project::Column::Code.eq(code.as_str()));
        if let Some(id) = 自分自身 {
            query = query.filter(project::Column::Id.ne(id));
        }
        let 衝突 = query
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 衝突.is_some() {
            return Ok(Err(rust_i18n::t!(
                "projects.duplicate_code",
                locale = locale
            )
            .to_string()));
        }
    }

    Ok(Ok((name, code)))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<ProjectForm>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();

    let (name, code) = match 検証(&state, &form, l, None).await? {
        Ok(値) => 値,
        Err(message) => return render(&登録画面(&current, &form, Some(message))),
    };

    let now = Utc::now();
    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(project::ActiveModel {
        // 名前を変えても参照が切れないよう、Dioryga側で採番する（設計書5.3）
        uid: Set(uuid::Uuid::new_v4().to_string()),
        code: Set(code),
        name: Set(name),
        description: Set(form.description.trim().to_owned()),
        currency: Set(currency::normalize(&form.currency)),
        // 日付と進行状態は持たない（5.1）。アーカイブは運用判断（5.2）
        archived_at: Set(None),
        closure_reason: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to("/admin/projects").into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let target = 対象(&state, id).await?;
    render(&編集画面(&current, id, &既存の値(&target), None))
}

fn 既存の値(target: &project::Model) -> ProjectForm {
    ProjectForm {
        name: target.name.clone(),
        code: target.code.clone().unwrap_or_default(),
        description: target.description.clone(),
        currency: target.currency.clone(),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<ProjectForm>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();
    let target = 対象(&state, id).await?;

    let (name, code) = match 検証(&state, &form, l, Some(id)).await? {
        Ok(値) => 値,
        Err(message) => return render(&編集画面(&current, id, &form, Some(message))),
    };

    変更(&state, &current, &target, |active| {
        active.name = Set(name);
        active.code = Set(code);
        active.description = Set(form.description.trim().to_owned());
        active.currency = Set(currency::normalize(&form.currency));
    })
    .await?;

    Ok(Redirect::to("/admin/projects").into_response())
}

// ---------------------------------------------------------------------------
// アーカイブ（設計書5.2）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ArchiveForm {
    #[serde(default)]
    pub closure_reason: String,
}

/// アーカイブの確認画面。
///
/// **一覧に確認なしのボタンを置かない。**`closure_reason` を必ず選ばせる必要が
/// あるため、独立した画面にしている。理由を取り違えると10.4の納期遵守率が
/// 汚れる（中止した案件が完了として数えられる）。
pub async fn archive_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let target = 対象(&state, id).await?;
    render(&アーカイブ画面(&current, &target, None))
}

fn アーカイブ画面(
    current: &CurrentUser,
    target: &project::Model,
    error: Option<String>,
) -> ArchivePage {
    let l = Locale::parse(&current.user.locale).as_str();
    ArchivePage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "admin_projects"),
        t_title: rust_i18n::t!("projects.archive_title", locale = l).to_string(),
        t_lead: rust_i18n::t!("projects.archive_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("common.back", locale = l).to_string(),
        t_reason: rust_i18n::t!("projects.closure_reason", locale = l).to_string(),
        t_reason_hint: rust_i18n::t!("projects.closure_reason_hint", locale = l).to_string(),
        t_completed: rust_i18n::t!("projects.reason_completed", locale = l).to_string(),
        t_cancelled: rust_i18n::t!("projects.reason_cancelled", locale = l).to_string(),
        t_submit: rust_i18n::t!("projects.archive", locale = l).to_string(),
        project_id: target.id,
        project_name: target.name.clone(),
        error,
    }
}

pub async fn archive(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<ArchiveForm>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();
    let target = 対象(&state, id).await?;

    let reason = match form.closure_reason.as_str() {
        COMPLETED => COMPLETED,
        CANCELLED => CANCELLED,
        _ => {
            return render(&アーカイブ画面(
                &current,
                &target,
                Some(rust_i18n::t!("projects.reason_required", locale = l).to_string()),
            ))
        }
    };

    if target.archived_at.is_none() {
        let now = Utc::now();
        変更(&state, &current, &target, |active| {
            active.archived_at = Set(Some(now));
            active.closure_reason = Set(Some(reason.to_owned()));
        })
        .await?;
    }

    Ok(Redirect::to("/admin/projects").into_response())
}

/// アーカイブを取り消す。
///
/// **`closure_reason` も一緒に消す。**残すと「進行中なのに完了理由がある」
/// という読み取れない状態になる。
pub async fn unarchive(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let target = 対象(&state, id).await?;

    if target.archived_at.is_some() {
        変更(&state, &current, &target, |active| {
            active.archived_at = Set(None);
            active.closure_reason = Set(None);
        })
        .await?;
    }

    Ok(Redirect::to("/admin/projects").into_response())
}

// ---------------------------------------------------------------------------
// Administratorの割り当て（設計書5章、A-5）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MembersForm {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub secondary: String,
}

impl MembersForm {
    /// 空文字は「割り当てなし」。
    fn parse(value: &str) -> Option<i32> {
        value.trim().parse().ok()
    }
}

pub async fn members_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let target = 対象(&state, id).await?;
    let (primary, secondary) = 現在の管理者(&state, id).await?;
    render(&割り当て画面(&state, &current, &target, primary, secondary, None).await?)
}

async fn 割り当て画面(
    state: &AppState,
    current: &CurrentUser,
    target: &project::Model,
    primary_id: Option<i32>,
    secondary_id: Option<i32>,
    error: Option<String>,
) -> AppResult<MembersPage> {
    let l = Locale::parse(&current.user.locale).as_str();

    Ok(MembersPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "admin_projects"),
        t_title: rust_i18n::t!("projects.members_title", locale = l).to_string(),
        t_lead: rust_i18n::t!("projects.members_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("common.back", locale = l).to_string(),
        t_primary: rust_i18n::t!("projects.primary_admin", locale = l).to_string(),
        t_secondary: rust_i18n::t!("projects.secondary_admin", locale = l).to_string(),
        t_secondary_hint: rust_i18n::t!("projects.secondary_hint", locale = l).to_string(),
        t_unassigned: rust_i18n::t!("projects.unassigned", locale = l).to_string(),
        t_submit: rust_i18n::t!("common.save", locale = l).to_string(),
        t_no_candidate: rust_i18n::t!("projects.no_candidate", locale = l).to_string(),
        t_go_users: rust_i18n::t!("projects.go_users", locale = l).to_string(),
        project_id: target.id,
        project_name: target.name.clone(),
        candidates: 候補(state).await?,
        primary_id,
        secondary_id,
        error,
    })
}

/// 割り当ての候補となる利用者。
///
/// **System Adminと無効化された利用者を除く。**System Adminはロールを持っても
/// プロジェクトデータへアクセスできず（3章）、選べてしまうと画面と実際の挙動が
/// 食い違う。無効化された利用者を新規の担当者にできないのは20.11の通り。
///
/// **0件になりうる。**初回セットアップ直後はSystem Adminしか居ないため、
/// 候補が1人も出ない。画面はそのとき理由と次の一手を出す（#90）——
/// 空なのが実態なのか設定漏れなのかを、利用者が判別できる必要がある。
async fn 候補(state: &AppState) -> AppResult<Vec<Candidate>> {
    let users = app_user::Entity::find()
        .filter(app_user::Column::IsSystemAdmin.eq(false))
        .filter(app_user::Column::DisabledAt.is_null())
        .order_by_asc(app_user::Column::Name)
        .order_by_asc(app_user::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(users
        .into_iter()
        .map(|u| Candidate {
            label: format!("{}（{}）", u.name, u.username),
            id: u.id,
        })
        .collect())
}

async fn 現在の管理者(
    state: &AppState,
    project_id: i32,
) -> AppResult<(Option<i32>, Option<i32>)> {
    let rows = 管理者の行(&state.db, project_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let 探す = |rank: &str| {
        rows.iter()
            .find(|m| m.admin_rank.as_deref() == Some(rank))
            .map(|m| m.user_id)
    };

    Ok((探す(PRIMARY), 探す(SECONDARY)))
}

async fn 管理者の行<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> Result<Vec<project_member::Model>, sea_orm::DbErr> {
    project_member::Entity::find()
        .filter(project_member::Column::ProjectId.eq(project_id))
        .filter(project_member::Column::Role.eq(ADMINISTRATOR))
        .all(db)
        .await
}

/// 正・副のAdministratorを設定し直す。
///
/// **既存のAdministrator行を消してから入れ直す。**A-5（正副それぞれ最大1名）を
/// 満たすうえで最も単純であり、`admin_rank` を付け替えるより取りこぼしがない。
/// 消すのは `role="Administrator"` の行だけで、**同じ利用者が兼務している
/// Operator等の行には触れない**（5章：複数ロールの兼務を許す）。
pub async fn update_members(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<MembersForm>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();
    let target = 対象(&state, id).await?;

    let primary = MembersForm::parse(&form.primary);
    let secondary = MembersForm::parse(&form.secondary);

    let 再表示 = |message: &str| {
        割り当て画面(
            &state,
            &current,
            &target,
            primary,
            secondary,
            Some(rust_i18n::t!(message, locale = l).to_string()),
        )
    };

    // 同一人物を正副に置いても冗長化にならない。5章が正副を分けた狙い
    // （不在時・引き継ぎ）が成立しなくなるため拒否する。
    if primary.is_some() && primary == secondary {
        return render(&再表示("projects.same_admin").await?);
    }

    // 副だけを置くと、A-5の「正は最大1名」は満たすが正が不在になる。
    // 5章は「Secondaryは任意（Primaryのみでも登録可）」であり、逆は想定していない。
    if primary.is_none() && secondary.is_some() {
        return render(&再表示("projects.secondary_without_primary").await?);
    }

    for candidate in [primary, secondary].into_iter().flatten() {
        if !割り当てられる(&state, candidate).await? {
            return render(&再表示("projects.invalid_candidate").await?);
        }
    }

    let now = Utc::now();
    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    for existing in 管理者の行(tx.reader(), id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        tx.delete(existing)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    for (user_id, rank) in [(primary, PRIMARY), (secondary, SECONDARY)] {
        let Some(user_id) = user_id else { continue };
        tx.insert(project_member::ActiveModel {
            user_id: Set(user_id),
            project_id: Set(id),
            role: Set(ADMINISTRATOR.to_owned()),
            admin_rank: Set(Some(rank.to_owned())),
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

    Ok(Redirect::to("/admin/projects").into_response())
}

/// 割り当ててよい利用者か。画面の候補と同じ条件をここでも確かめる。
///
/// **選択肢を絞るだけでは足りない。**送信内容は書き換えられる。
async fn 割り当てられる(state: &AppState, user_id: i32) -> AppResult<bool> {
    let user = app_user::Entity::find_by_id(user_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(user.is_some_and(|u| !u.is_system_admin && u.disabled_at.is_none()))
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 対象(state: &AppState, id: i32) -> AppResult<project::Model> {
    project::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)
}

async fn 変更<F>(
    state: &AppState,
    current: &CurrentUser,
    target: &project::Model,
    適用: F,
) -> AppResult<()>
where
    F: FnOnce(&mut project::ActiveModel),
{
    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut active: project::ActiveModel = target.clone().into();
    適用(&mut active);
    active.updated_at = Set(Utc::now());

    tx.update(target, active)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 状態フィルタの既定は進行中のみ() {
        assert_eq!(StatusFilter::parse(None), StatusFilter::Active);
        assert_eq!(
            StatusFilter::parse(Some("archived")),
            StatusFilter::Archived
        );
        assert_eq!(StatusFilter::parse(Some("all")), StatusFilter::All);
    }

    #[test]
    fn 未選択は割り当てなしとして読む() {
        assert_eq!(MembersForm::parse(""), None);
        assert_eq!(MembersForm::parse("  "), None);
        assert_eq!(MembersForm::parse("7"), Some(7));
    }

    #[test]
    fn likeのワイルドカードが打ち消される() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
    }
}
