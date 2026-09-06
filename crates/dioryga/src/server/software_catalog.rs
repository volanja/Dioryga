//! ソフトウェアカタログ（設計書16.1のD領域、9.4）。
//!
//! # 一覧が中心になる（16.1）
//!
//! **SBOM取込はこのテーブルを自動生成しない**（9.2）。SBOMが運ぶのは
//! 「その機器に何が入っていたか」という観測であって、**組織として管理対象に
//! 選んだソフトウェア**とは別物である。9.8のコンポーネント検索が前者を扱い、
//! ここは後者を扱う。
//!
//! したがって登録は人が行う。**それでも一覧が中心**なのは、バージョンごとに
//! 別レコードになるため行数が増えやすいからである（9.4）。
//!
//! # `purl` を持たない行は何件でも共存できる（9.4）
//!
//! `purl` はUNIQUEだが nullable であり、**両DBともNULL同士は重複と見なさない。**
//! 社内ツールのように `purl` を持たないものがあるため、**空文字ではなくNULLで
//! 保存する**——空文字で保存すると2件目から登録できなくなる。

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{software_catalog, software_instance, vendor};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{入場, 正規化, 現役のベンダー, 編集権, Labeled};
use crate::server::view::{render, Chrome};
use crate::server::AppState;

/// 閉じた語彙（`vocabularies.md`、設計書9.4）。
const CATEGORIES: &[&str] = &["OS", "Application", "Library", "Framework"];

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub retired: Option<String>,
    /// 名前での絞り込み。バージョンごとに行が増えるため（9.4）。
    #[serde(default)]
    pub q: Option<String>,
}

struct SoftwareRow {
    name: String,
    version: String,
    category: String,
    vendor: String,
    purl: String,
    license: String,
    retired: bool,
    /// `SOFTWARE_INSTANCE` から参照されている（18.2）。
    referenced: bool,
    id: i32,
}

#[derive(askama::Template)]
#[template(path = "catalog_software.html")]
struct SoftwarePage {
    chrome: Chrome,
    t_apply: String,
    t_title: String,
    t_lead: String,
    t_sbom_hint: String,
    t_keyword: String,
    t_name: String,
    t_version: String,
    t_category: String,
    t_vendor: String,
    t_purl: String,
    t_purl_hint: String,
    t_license: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_referenced: String,
    t_show_retired: String,
    t_no_vendor: String,
    rows: Vec<SoftwareRow>,
    vendors: Vec<Labeled>,
    categories: Vec<&'static str>,
    q: String,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    一覧を描く(&state, &current, &query, None).await
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    query: &ListQuery,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = 編集権(state, current).await.is_ok();

    let show_retired = query.retired.as_deref() == Some("1");
    let keyword = query.q.clone().unwrap_or_default();
    let 絞る = keyword.trim().to_lowercase();

    let list = software_catalog::Entity::find()
        .order_by_asc(software_catalog::Column::Name)
        .order_by_asc(software_catalog::Column::Version)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for s in list {
        if !show_retired && s.retired_at.is_some() {
            continue;
        }
        if !絞る.is_empty() && !s.name.to_lowercase().contains(&絞る) {
            continue;
        }
        rows.push(SoftwareRow {
            vendor: match s.vendor_id {
                Some(id) => ベンダー名(&state.db, id).await?,
                None => String::new(),
            },
            referenced: 参照されている(&state.db, s.id).await?,
            retired: s.retired_at.is_some(),
            purl: s.purl.unwrap_or_default(),
            id: s.id,
            name: s.name,
            version: s.version,
            category: s.category,
            license: s.license_expression,
        });
    }

    render(&SoftwarePage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "catalog"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_title: rust_i18n::t!("software.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("software.lead", locale = l).to_string(),
        t_sbom_hint: rust_i18n::t!("software.sbom_hint", locale = l).to_string(),
        t_keyword: rust_i18n::t!("components.keyword", locale = l).to_string(),
        t_name: rust_i18n::t!("sbom.name", locale = l).to_string(),
        t_version: rust_i18n::t!("components.version", locale = l).to_string(),
        t_category: rust_i18n::t!("parts.category", locale = l).to_string(),
        t_vendor: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
        t_purl: rust_i18n::t!("components.purl", locale = l).to_string(),
        t_purl_hint: rust_i18n::t!("software.purl_hint", locale = l).to_string(),
        t_license: rust_i18n::t!("software.license", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("software.new", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_referenced: rust_i18n::t!("catalog.referenced", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        t_no_vendor: rust_i18n::t!("software.no_vendor", locale = l).to_string(),
        rows,
        vendors: 現役のベンダー(&state.db).await?,
        categories: CATEGORIES.to_vec(),
        q: keyword,
        show_retired,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct SoftwareForm {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub vendor_id: String,
    #[serde(default)]
    pub purl: String,
    #[serde(default)]
    pub license_expression: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<SoftwareForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let query = ListQuery::default();
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    let name = 正規化(&form.name);
    let version = 正規化(&form.version);
    if name.is_empty() || version.is_empty() {
        return 一覧を描く(&state, &current, &query, 誤り("software.error_required")).await;
    }
    // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
    if !CATEGORIES.contains(&form.category.as_str()) {
        return 一覧を描く(&state, &current, &query, 誤り("software.error_category")).await;
    }

    // **空文字ではなくNULLで持つ**（9.4）。空文字だと2件目から登録できない
    let purl = match form.purl.trim() {
        "" => None,
        v => Some(v.to_owned()),
    };
    if let Some(p) = &purl {
        let 重複 = software_catalog::Entity::find()
            .filter(software_catalog::Column::Purl.eq(p))
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 重複.is_some() {
            return 一覧を描く(
                &state,
                &current,
                &query,
                誤り("software.error_purl_duplicate"),
            )
            .await;
        }
    }

    let vendor_id = match form.vendor_id.trim() {
        "" => None,
        v => match v.parse::<i32>() {
            Ok(id) => Some(id),
            Err(_) => {
                return 一覧を描く(&state, &current, &query, 誤り("cables.error_vendor")).await
            }
        },
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(software_catalog::ActiveModel {
        name: Set(name),
        version: Set(version),
        category: Set(form.category.clone()),
        vendor_id: Set(vendor_id),
        purl: Set(purl),
        license_expression: Set(正規化(&form.license_expression)),
        spec_json: Set("{}".to_owned()),
        retired_at: Set(None),
        created_by: Set(current.user.id),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to("/catalog/software").into_response())
}

#[derive(Debug, Deserialize)]
pub struct RetireForm {
    pub id: i32,
    #[serde(default)]
    pub undo: String,
}

pub async fn retire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<RetireForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

    let before = software_catalog::Entity::find_by_id(form.id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        software_catalog::ActiveModel {
            id: Set(form.id),
            retired_at: Set((form.undo != "1").then(Utc::now)),
            updated_at: Set(Utc::now()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to("/catalog/software").into_response())
}

/// 18.2の判定。
async fn 参照されている<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<bool> {
    Ok(software_instance::Entity::find()
        .filter(software_instance::Column::SoftwareCatalogId.eq(id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some())
}

async fn ベンダー名<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<String> {
    Ok(vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|v| v.name)
        .unwrap_or_default())
}
