//! 機器横断のコンポーネント検索（設計書16.1のB領域、9.8）。
//!
//! # これは脆弱性対応の画面である
//!
//! 「特定のバージョンのライブラリが入っている機器を全て挙げる」に答える。
//! 16.2のフロー9がその使い方——**検索で洗い出し、対象ごとに変更管理チケット
//! （Repair）を起票し、更新後にSBOMを再取込して差分で解消を確認する。**
//!
//! # 索引は `content_hash` 単位（9.8）
//!
//! Deviceごとに張ると3,000万行になるが、スナップショットを共有している以上
//! その必要がない。**結合は `SBOM_IMPORT` を経由する**ため、同じイメージの
//! 機器が何台あっても索引の行数は変わらない。
//!
//! # 過去時点も同じ索引で引ける（9.8）
//!
//! `superseded_at` の条件を変えるだけ。「あの日この機器に何が入っていたか」に
//! 答えられる。**索引を捨てて作り直しても失われるものが無い**という性質は、
//! この検索が派生物の上に成り立っていることの裏返しである。
//!
//! # クロスプロジェクト可視性（3章、9.8）
//!
//! スナップショットは複数プロジェクトの機器間で共有されうるが、**閲覧できるのは
//! 自分が権限を持つDeviceの取込行を通じてのみ**である。`device_assignment` で
//! 絞ることでこれを担保する。A-6により**過去に所属した機器も対象に含める**
//! ——移設された機器に脆弱性が残っていても見えなくなっては困る。

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Extension;
use entity::project;
use sea_orm::{ConnectionTrait, EntityTrait, FromQueryResult, Statement};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 返す上限。**超えたら画面で伝える**（23.5の「分母を表示する」と同じ考え方で、
/// 出ていない行があることを黙らせない）。
const 上限: usize = 500;

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    /// `all` なら過去の観測も含める（設計書9.8）。
    #[serde(default)]
    pub scope: Option<String>,
}

struct Hit {
    device_id: i32,
    hostname: String,
    name: String,
    version: String,
    purl: String,
    observed_at: String,
    /// 過去の観測（`superseded_at` あり）。
    past: bool,
}

#[derive(askama::Template)]
#[template(path = "components.html")]
struct ComponentsPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_keyword: String,
    t_keyword_hint: String,
    t_search: String,
    t_scope_current: String,
    t_scope_all: String,
    t_device: String,
    t_name: String,
    t_version: String,
    t_purl: String,
    t_observed_at: String,
    t_past: String,
    t_detail: String,
    t_actions: String,
    t_empty: String,
    t_prompt: String,
    t_truncated: String,
    q: String,
    scope: String,
    hits: Vec<Hit>,
    /// 検索していない状態。**0件と区別する。**
    searched: bool,
    truncated: bool,
}

pub async fn search(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Response> {
    let project = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let keyword = query.q.clone().unwrap_or_default();
    let 過去も = query.scope.as_deref() == Some("all");
    let searched = !keyword.trim().is_empty();

    let mut hits = if searched {
        引く(&state.db, project_id, keyword.trim(), 過去も).await?
    } else {
        Vec::new()
    };

    let truncated = hits.len() > 上限;
    hits.truncate(上限);

    render(&ComponentsPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "projects"),
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("components.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("components.lead", locale = l).to_string(),
        t_keyword: rust_i18n::t!("components.keyword", locale = l).to_string(),
        t_keyword_hint: rust_i18n::t!("components.keyword_hint", locale = l).to_string(),
        t_search: rust_i18n::t!("common.search", locale = l).to_string(),
        t_scope_current: rust_i18n::t!("components.scope_current", locale = l).to_string(),
        t_scope_all: rust_i18n::t!("components.scope_all", locale = l).to_string(),
        t_device: rust_i18n::t!("components.device", locale = l).to_string(),
        t_name: rust_i18n::t!("sbom.name", locale = l).to_string(),
        t_version: rust_i18n::t!("components.version", locale = l).to_string(),
        t_purl: rust_i18n::t!("components.purl", locale = l).to_string(),
        t_observed_at: rust_i18n::t!("components.observed_at", locale = l).to_string(),
        t_past: rust_i18n::t!("components.past", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("components.empty", locale = l).to_string(),
        t_prompt: rust_i18n::t!("components.prompt", locale = l).to_string(),
        t_truncated: rust_i18n::t!("components.truncated", locale = l, limit = 上限).to_string(),
        q: keyword,
        scope: if 過去も { "all" } else { "current" }.to_owned(),
        hits,
        searched,
        truncated,
    })
}

// ---------------------------------------------------------------------------
// 検索（設計書9.8）
// ---------------------------------------------------------------------------

/// 9.8が載せている検索の形をそのまま使う。
///
/// **`purl` の前方一致と `name` の部分一致の両方で引く。**`purl` を持たない
/// コンポーネントがあるため（9.6）、`purl` だけでは取りこぼす。
#[derive(Debug, FromQueryResult)]
struct Row {
    device_id: i32,
    hostname: String,
    name: String,
    version: String,
    purl: Option<String>,
    imported_at: chrono::DateTime<chrono::Utc>,
    /// PostgreSQL / SQLite とも真偽値を整数で返すため `i32` で受ける。
    past: i32,
}

async fn 引く<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    keyword: &str,
    過去も: bool,
) -> AppResult<Vec<Hit>> {
    // **`superseded_at` の条件を変えるだけで過去時点を引ける**（9.8）
    let 期間 = if 過去も {
        ""
    } else {
        "AND si.superseded_at IS NULL"
    };

    // A-6により、**過去にこのプロジェクトへ所属した機器も対象に含める**（3章）。
    // `to_date` を見ないのが要点で、絞ると移設された機器の脆弱性が見えなくなる
    let sql = format!(
        r#"
        SELECT d.id AS device_id,
               d.hostname AS hostname,
               i.name AS name,
               i.version AS version,
               i.purl AS purl,
               si.imported_at AS imported_at,
               CASE WHEN si.superseded_at IS NULL THEN 0 ELSE 1 END AS past
        FROM sbom_import si
        JOIN sbom_component_index i ON i.content_hash = si.content_hash
        JOIN device d ON d.id = si.device_id
        WHERE d.id IN (
                SELECT da.device_id FROM device_assignment da
                WHERE da.location_type = 'Project' AND da.location_id = $1
              )
          {期間}
          AND (i.purl LIKE $2 ESCAPE '\' OR lower(i.name) LIKE $3 ESCAPE '\')
        ORDER BY d.hostname, i.name, i.version
        "#
    );

    let 語 = escape_like(keyword);
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        &sql,
        [
            project_id.into(),
            // purl は前方一致。`pkg:maven/org/log4j` で `@2.14.1` 付きに当てる
            format!("{語}%").into(),
            format!("%{}%", 語.to_lowercase()).into(),
        ],
    ))
    .all(db)
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(rows
        .into_iter()
        .map(|r| Hit {
            device_id: r.device_id,
            hostname: r.hostname,
            name: r.name,
            version: r.version,
            purl: r.purl.unwrap_or_default(),
            observed_at: r.imported_at.format("%Y-%m-%d %H:%M").to_string(),
            past: r.past != 0,
        })
        .collect())
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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
