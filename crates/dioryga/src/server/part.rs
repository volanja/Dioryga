//! 部品カタログ（設計書16.1のD領域、6.4、8.3）。
//!
//! # 集計に使う値だけをカラムにする（6.4のハイブリッド）
//!
//! `core_count` / `capacity_gb` は**横断集計の対象**なので実カラム、周波数・
//! ECC有無・RPM等は `spec_json` に置く。完全JSONにしなかったのは、
//! PostgreSQL（`jsonb ->>`）とSQLite（`json_extract`）で構文が異なり、
//! **集計クエリがバックエンドごとに分岐する**ためである（6.4）。
//!
//! 「プロジェクト全体のCPUコア数合計」のような典型的な集計が、実カラムへの
//! 通常の `SUM()` として両DB共通のSQLで書ける。
//!
//! # `spec_json` は妥当なJSONであることだけ確かめる
//!
//! **中身の構造は検証しない。**6.4が「キー名の一貫性をDBが保証できない」ことを
//! JSON方式の難点として挙げているが、それは*集計に使う値*についての話である。
//! 集計に使わないと決めた値にスキーマを課すと、ハイブリッドにした意味が薄れる。
//!
//! ただし**壊れたJSONは受け付けない。**読めない文字列を溜めると、後から
//! 使おうとした時点で全件が疑わしくなる。
//!
//! # ポートは部品が持つ（8.3）
//!
//! `PART_PORT_SLOT` は `port_kind`（Network / Power / Stack）で一般化されて
//! いる。`port_speed` は `Network` のとき、`voltage_min`/`voltage_max` は
//! `Power` のときだけ意味を持つ（12.7）。**他の `port_kind` に値が来たら
//! 黙って捨てず拒否する**（Q-21）。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{configuration_part, part_catalog, part_instance, part_port_slot, vendor};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{入場, 正規化, 現役のベンダー, 編集権, Labeled};
use crate::server::view::{render, Chrome};
use crate::server::AppState;

/// 語彙（`vocabularies.md`、設計書6.4）。
const CATEGORIES: &[&str] = &["CPU", "Memory", "NIC", "Storage", "PSU", "PDU"];
const PORT_KINDS: &[&str] = &["Network", "Power", "Stack"];

const NETWORK: &str = "Network";
const POWER: &str = "Power";

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub retired: Option<String>,
}

struct PartRow {
    id: i32,
    category: String,
    vendor: String,
    part_number: String,
    /// 該当しない部品では空。**集計に使うので実カラム**（6.4）。
    core_count: String,
    capacity_gb: String,
    ports: usize,
    retired: bool,
    /// 18.2により、スペックを編集できない。
    referenced: bool,
}

#[derive(askama::Template)]
#[template(path = "catalog_parts.html")]
struct PartsPage {
    chrome: Chrome,
    t_apply: String,
    t_title: String,
    t_lead: String,
    t_hybrid_hint: String,
    t_category: String,
    t_vendor: String,
    t_part_number: String,
    t_core_count: String,
    t_capacity_gb: String,
    t_spec_json: String,
    t_spec_hint: String,
    t_ports: String,
    t_actions: String,
    t_detail: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_referenced: String,
    t_show_retired: String,
    t_no_vendor: String,
    t_spec_column_hint: String,
    rows: Vec<PartRow>,
    vendors: Vec<Labeled>,
    categories: Vec<&'static str>,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
}

struct PortRow {
    id: i32,
    port_kind: String,
    port_label: String,
    connector_type: String,
    /// `port_kind=Network` のときだけ埋まる。
    port_speed: String,
    /// `port_kind=Power` のときだけ埋まる（12.7）。
    voltage: String,
}

#[derive(askama::Template)]
#[template(path = "catalog_part_detail.html")]
struct PartDetailPage {
    chrome: Chrome,
    part_id: i32,
    t_back: String,
    t_basic: String,
    t_ports: String,
    t_ports_hint: String,
    t_port_kind: String,
    t_port_label: String,
    t_connector_type: String,
    t_connector_hint: String,
    t_port_speed: String,
    t_port_speed_hint: String,
    t_voltage: String,
    t_voltage_hint: String,
    t_voltage_min: String,
    t_voltage_max: String,
    t_add_port: String,
    t_remove: String,
    t_empty: String,
    t_actions: String,
    title: String,
    basic: Vec<Labeled>,
    ports: Vec<PortRow>,
    port_kinds: Vec<&'static str>,
    can_edit: bool,
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// 一覧・登録
// ---------------------------------------------------------------------------

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    一覧を描く(
        &state,
        &current,
        query.retired.as_deref() == Some("1"),
        None,
    )
    .await
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    show_retired: bool,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = 編集権(state, current).await.is_ok();

    let parts = part_catalog::Entity::find()
        .order_by_asc(part_catalog::Column::PartNumber)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for p in parts {
        if !show_retired && p.retired_at.is_some() {
            continue;
        }
        rows.push(PartRow {
            vendor: ベンダー名(&state.db, p.vendor_id).await?,
            ports: ポート数(&state.db, p.id).await?,
            referenced: 参照されている(&state.db, p.id).await?,
            retired: p.retired_at.is_some(),
            core_count: p.core_count.map(|v| v.to_string()).unwrap_or_default(),
            capacity_gb: p.capacity_gb.map(|v| v.to_string()).unwrap_or_default(),
            id: p.id,
            category: p.category,
            part_number: p.part_number,
        });
    }

    render(&PartsPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "catalog"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_title: rust_i18n::t!("parts.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("parts.lead", locale = l).to_string(),
        t_hybrid_hint: rust_i18n::t!("parts.hybrid_hint", locale = l).to_string(),
        t_category: rust_i18n::t!("parts.category", locale = l).to_string(),
        t_vendor: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
        t_part_number: rust_i18n::t!("parts.part_number", locale = l).to_string(),
        t_core_count: rust_i18n::t!("parts.core_count", locale = l).to_string(),
        t_capacity_gb: rust_i18n::t!("parts.capacity_gb", locale = l).to_string(),
        t_spec_json: rust_i18n::t!("parts.spec_json", locale = l).to_string(),
        t_spec_hint: rust_i18n::t!("parts.spec_hint", locale = l).to_string(),
        t_ports: rust_i18n::t!("parts.ports", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("parts.new", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_referenced: rust_i18n::t!("catalog.referenced", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        t_no_vendor: rust_i18n::t!("catalog.no_vendor", locale = l).to_string(),
        t_spec_column_hint: rust_i18n::t!("parts.spec_column_hint", locale = l).to_string(),
        rows,
        vendors: 現役のベンダー(&state.db).await?,
        categories: CATEGORIES.to_vec(),
        show_retired,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct PartForm {
    pub vendor_id: i32,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub part_number: String,
    #[serde(default)]
    pub core_count: String,
    #[serde(default)]
    pub capacity_gb: String,
    #[serde(default)]
    pub spec_json: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<PartForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // 型番も製品名と同じ規則で正規化する（18.4、25.2の段階3）
    let part_number = 正規化(&form.part_number);
    if part_number.is_empty() {
        return 一覧を描く(&state, &current, false, 誤り("parts.error_part_number")).await;
    }
    // **語彙外は既定へ寄せず拒否する**（Q-21）
    if !CATEGORIES.contains(&form.category.as_str()) {
        return 一覧を描く(&state, &current, false, 誤り("parts.error_category")).await;
    }

    let core_count = match 任意の整数(&form.core_count) {
        Ok(v) => v,
        Err(()) => return 一覧を描く(&state, &current, false, 誤り("parts.error_number")).await,
    };
    let capacity_gb = match 任意の整数(&form.capacity_gb) {
        Ok(v) => v,
        Err(()) => return 一覧を描く(&state, &current, false, 誤り("parts.error_number")).await,
    };

    // **壊れたJSONは受け付けない。**読めない文字列を溜めると、後から使おうと
    // した時点で全件が疑わしくなる。中身の構造までは見ない（6.4）
    let spec_json = match form.spec_json.trim() {
        "" => "{}".to_owned(),
        v => match serde_json::from_str::<serde_json::Value>(v) {
            Ok(_) => v.to_owned(),
            Err(_) => {
                return 一覧を描く(&state, &current, false, 誤り("parts.error_spec_json")).await;
            }
        },
    };

    // **`UNIQUE(vendor_id, part_number)`**（18.5）。同一ベンダー内では重複を作れない
    let 重複 = part_catalog::Entity::find()
        .filter(part_catalog::Column::VendorId.eq(form.vendor_id))
        .filter(part_catalog::Column::PartNumber.eq(&part_number))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複.is_some() {
        return 一覧を描く(&state, &current, false, 誤り("parts.error_duplicate")).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(part_catalog::ActiveModel {
        category: Set(form.category.clone()),
        vendor_id: Set(form.vendor_id),
        part_number: Set(part_number),
        core_count: Set(core_count),
        capacity_gb: Set(capacity_gb),
        spec_json: Set(spec_json),
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

    Ok(Redirect::to("/catalog/parts").into_response())
}

// ---------------------------------------------------------------------------
// 詳細（ポート、設計書8.3）
// ---------------------------------------------------------------------------

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(part_id): Path<i32>,
) -> AppResult<Response> {
    詳細を描く(&state, &current, part_id, None).await
}

async fn 詳細を描く(
    state: &AppState,
    current: &CurrentUser,
    part_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = 編集権(state, current).await.is_ok();

    let p = part_catalog::Entity::find_by_id(part_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let mut basic = vec![
        Labeled {
            label: rust_i18n::t!("parts.category", locale = l).to_string(),
            value: p.category.clone(),
        },
        Labeled {
            label: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
            value: ベンダー名(&state.db, p.vendor_id).await?,
        },
    ];
    // **集計に使う値は実カラム**（6.4）。該当しない部品では出さない
    for (key, value) in [
        ("parts.core_count", p.core_count),
        ("parts.capacity_gb", p.capacity_gb),
    ] {
        if let Some(v) = value {
            basic.push(Labeled {
                label: rust_i18n::t!(key, locale = l).to_string(),
                value: v.to_string(),
            });
        }
    }
    if p.spec_json != "{}" {
        basic.push(Labeled {
            label: rust_i18n::t!("parts.spec_json", locale = l).to_string(),
            value: p.spec_json.clone(),
        });
    }

    let ports = part_port_slot::Entity::find()
        .filter(part_port_slot::Column::PartCatalogId.eq(part_id))
        .order_by_asc(part_port_slot::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|s| PortRow {
            // **意味を持つ列だけを見せる**（8.3、12.7）
            port_speed: match s.port_kind.as_str() {
                NETWORK => s.port_speed.clone().unwrap_or_default(),
                _ => String::new(),
            },
            voltage: match (s.port_kind.as_str(), s.voltage_min, s.voltage_max) {
                (POWER, Some(min), Some(max)) if min == max => format!("{min}V"),
                (POWER, Some(min), Some(max)) => format!("{min}〜{max}V"),
                _ => String::new(),
            },
            id: s.id,
            port_kind: s.port_kind,
            port_label: s.port_label,
            connector_type: s.connector_type,
        })
        .collect();

    render(&PartDetailPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "catalog"),
        part_id,
        t_back: rust_i18n::t!("parts.back", locale = l).to_string(),
        t_basic: rust_i18n::t!("devices.basic", locale = l).to_string(),
        t_ports: rust_i18n::t!("parts.ports", locale = l).to_string(),
        t_ports_hint: rust_i18n::t!("parts.ports_hint", locale = l).to_string(),
        t_port_kind: rust_i18n::t!("parts.port_kind", locale = l).to_string(),
        t_port_label: rust_i18n::t!("parts.port_label", locale = l).to_string(),
        t_connector_type: rust_i18n::t!("parts.connector_type", locale = l).to_string(),
        t_connector_hint: rust_i18n::t!("parts.connector_hint", locale = l).to_string(),
        t_port_speed: rust_i18n::t!("parts.port_speed", locale = l).to_string(),
        t_port_speed_hint: rust_i18n::t!("parts.port_speed_hint", locale = l).to_string(),
        t_voltage: rust_i18n::t!("parts.voltage", locale = l).to_string(),
        t_voltage_hint: rust_i18n::t!("parts.voltage_hint", locale = l).to_string(),
        t_voltage_min: rust_i18n::t!("parts.voltage_min", locale = l).to_string(),
        t_voltage_max: rust_i18n::t!("parts.voltage_max", locale = l).to_string(),
        t_add_port: rust_i18n::t!("parts.add_port", locale = l).to_string(),
        t_remove: rust_i18n::t!("parts.remove_port", locale = l).to_string(),
        t_empty: rust_i18n::t!("parts.no_port", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        title: format!(
            "{} {}",
            ベンダー名(&state.db, p.vendor_id).await?,
            p.part_number
        ),
        basic,
        ports,
        port_kinds: PORT_KINDS.to_vec(),
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct PortForm {
    #[serde(default)]
    pub port_kind: String,
    #[serde(default)]
    pub port_label: String,
    #[serde(default)]
    pub connector_type: String,
    #[serde(default)]
    pub port_speed: String,
    #[serde(default)]
    pub voltage_min: String,
    #[serde(default)]
    pub voltage_max: String,
}

pub async fn add_port(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(part_id): Path<i32>,
    Form(form): Form<PortForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    part_catalog::Entity::find_by_id(part_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    if !PORT_KINDS.contains(&form.port_kind.as_str()) {
        return 詳細を描く(&state, &current, part_id, 誤り("parts.error_port_kind")).await;
    }
    let label = form.port_label.trim();
    let connector = form.connector_type.trim();
    if label.is_empty() || connector.is_empty() {
        return 詳細を描く(&state, &current, part_id, 誤り("parts.error_port_required")).await;
    }

    // **`port_speed` は Network のときだけ意味を持つ**（8.3）。
    // 他の port_kind に値が来たら黙って捨てず拒否する（Q-21）
    let speed = form.port_speed.trim();
    if !speed.is_empty() && form.port_kind != NETWORK {
        return 詳細を描く(&state, &current, part_id, 誤り("parts.error_speed_unused")).await;
    }

    // **電圧は Power のときだけ**（12.7）
    let (min, max) = match (任意の整数(&form.voltage_min), 任意の整数(&form.voltage_max))
    {
        (Ok(a), Ok(b)) => (a, b),
        _ => return 詳細を描く(&state, &current, part_id, 誤り("parts.error_number")).await,
    };
    if (min.is_some() || max.is_some()) && form.port_kind != POWER {
        return 詳細を描く(
            &state,
            &current,
            part_id,
            誤り("parts.error_voltage_unused"),
        )
        .await;
    }
    // **片方だけでは範囲にならない。**「100-240V対応」と「200V専用」を区別する
    // ために範囲で持っているので、欠けていると意味を成さない（12.7）
    if min.is_some() != max.is_some() {
        return 詳細を描く(&state, &current, part_id, 誤り("parts.error_voltage_pair")).await;
    }
    if matches!((min, max), (Some(a), Some(b)) if a > b) {
        return 詳細を描く(&state, &current, part_id, 誤り("parts.error_voltage_order")).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(part_port_slot::ActiveModel {
        part_catalog_id: Set(part_id),
        port_kind: Set(form.port_kind.clone()),
        port_label: Set(label.to_owned()),
        connector_type: Set(connector.to_owned()),
        port_speed: Set((!speed.is_empty()).then(|| speed.to_owned())),
        voltage_min: Set(min),
        voltage_max: Set(max),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/catalog/parts/{part_id}")).into_response())
}

#[derive(Debug, Deserialize)]
pub struct RemovePortForm {
    pub port_id: i32,
}

/// ポートを取り消す。
///
/// **カタログの定義であって履歴ではないため、行を消す。**「いつまでこの定義
/// だったか」は事実ではなく、登録の誤りを直しているだけである。誰がいつ消したかは
/// `AUDIT_LOG` が持つ。
pub async fn remove_port(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(part_id): Path<i32>,
    Form(form): Form<RemovePortForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

    let row = part_port_slot::Entity::find_by_id(form.port_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|r| r.part_catalog_id == part_id)
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.delete(row)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/catalog/parts/{part_id}")).into_response())
}

// ---------------------------------------------------------------------------
// 廃番（設計書18.5）
// ---------------------------------------------------------------------------

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

    let before = part_catalog::Entity::find_by_id(form.id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        part_catalog::ActiveModel {
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

    Ok(Redirect::to("/catalog/parts").into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// 18.2の判定。**`PART_INSTANCE` と `CONFIGURATION_PART` の両方を見る。**
async fn 参照されている<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<bool> {
    let 実物あり = part_instance::Entity::find()
        .filter(part_instance::Column::PartCatalogId.eq(id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some();
    if 実物あり {
        return Ok(true);
    }

    Ok(configuration_part::Entity::find()
        .filter(configuration_part::Column::PartCatalogId.eq(id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some())
}

async fn ポート数<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<usize> {
    Ok(part_port_slot::Entity::find()
        .filter(part_port_slot::Column::PartCatalogId.eq(id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .len())
}

async fn ベンダー名<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<String> {
    Ok(vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|v| v.name)
        .unwrap_or_default())
}

/// 空なら未設定、数値でなければ誤り。**黙って未設定に落とさない**（Q-21）。
fn 任意の整数(value: &str) -> Result<Option<i32>, ()> {
    match value.trim() {
        "" => Ok(None),
        v => v.parse::<i32>().map(Some).map_err(|_| ()),
    }
}
