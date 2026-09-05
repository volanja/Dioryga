//! プロジェクト領域の機器一覧・詳細（設計書16.1のB領域、6章、23章）。
//!
//! # クロスプロジェクト可視性（A-6、設計書3章）
//!
//! **「現在このプロジェクトにある機器」だけを出すのでは足りない。**プロジェクトPの
//! メンバーは「現在Pに属する、**または過去にPに属したことがある**」機器の全履歴を
//! 閲覧できる。現在の割当だけで絞ると、移設された機器の履歴が誰からも見えなくなる。
//!
//! 判定は `DEVICE_ASSIGNMENT` に `(Project, P)` の行が**一度でも**現れるか
//! （`to_date` を見ない）で行う。
//!
//! # 現在の状態は計算で求める
//!
//! 所在・搭載位置・搭載部品・ファームウェアは、いずれも履歴テーブルの
//! `to_date IS NULL` から引く（不変条件2）。`DEVICE` 側に持たない。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    chassis_model, configuration, device, device_assignment, device_mount, device_stack,
    firmware_version, part_catalog, part_instance, part_instance_location, project, vendor,
};
use sea_orm::sea_query::{Expr, Func, LikeExpr, Query as SeaQuery};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// `DEVICE_ASSIGNMENT.location_type`。
const PROJECT: &str = "Project";

/// 予約中の機器（設計書11.6）。ラック図でも区別表示する。
const PLAN: &str = "plan";

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct DeviceRow {
    id: i32,
    hostname: String,
    device_type: String,
    model: String,
    status: String,
    /// 予約中は区別して表示する（設計書11.6）
    planned: bool,
    location: String,
    /// このプロジェクトを離れた機器。A-6により履歴は見える
    departed: bool,
}

#[derive(askama::Template)]
#[template(path = "devices.html")]
struct DevicesPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_new: String,
    t_import: String,
    t_work_orders: String,
    t_members: String,
    t_keyword: String,
    t_search: String,
    t_scope_current: String,
    t_scope_all: String,
    t_hostname: String,
    t_device_type: String,
    t_model: String,
    t_status: String,
    t_location: String,
    t_actions: String,
    t_detail: String,
    t_empty: String,
    t_departed: String,
    t_planned: String,
    q: String,
    scope: String,
    rows: Vec<DeviceRow>,
    can_edit: bool,
}

struct Labeled {
    label: String,
    value: String,
}

#[derive(askama::Template)]
#[template(path = "device_detail.html")]
struct DeviceDetailPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    device_id: i32,
    t_back: String,
    t_edit: String,
    t_basic: String,
    t_location_history: String,
    t_mount: String,
    t_parts: String,
    t_firmware: String,
    t_stack: String,
    t_none: String,
    t_planned: String,
    t_not_implemented: String,
    hostname: String,
    planned: bool,
    /// 統合先。**統合された機器は削除されず、ここへ誘導する**（設計書23.9）
    merged_into: Option<i32>,
    t_merged: String,
    basic: Vec<Labeled>,
    locations: Vec<Labeled>,
    mount: Vec<Labeled>,
    parts: Vec<Labeled>,
    firmware: Vec<Labeled>,
    stack: Vec<Labeled>,
    can_edit: bool,
}

#[derive(askama::Template)]
#[template(path = "device_form.html")]
struct DeviceFormPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_hostname: String,
    t_device_type: String,
    t_configuration: String,
    t_configuration_hint: String,
    t_device_category: String,
    t_device_category_hint: String,
    t_serial_number: String,
    t_serial_hint: String,
    t_asset_number: String,
    t_asset_hint: String,
    t_power_watt: String,
    t_status: String,
    t_submit: String,
    action: String,
    hostname: String,
    device_type: String,
    device_types: Vec<&'static str>,
    configuration_id: String,
    configurations: Vec<Labeled>,
    device_category: String,
    categories: Vec<&'static str>,
    serial_number: String,
    asset_number: String,
    power_watt: String,
    status: String,
    statuses: Vec<&'static str>,
    error: Option<String>,
}

/// 語彙（`vocabularies.md`）。DB制約にはせず、画面はリストから選ばせる。
const DEVICE_TYPES: &[&str] = &["Physical", "Virtual", "Container", "Logical"];
const STATUSES: &[&str] = &["running", "broken", "repair", "plan", "building"];
const CATEGORIES: &[&str] = &[
    "Server",
    "Switch",
    "Router",
    "Firewall",
    "LoadBalancer",
    "Vpn",
    "MediaConverter",
    "Storage",
    "Pdu",
    "Ups",
    "Kvm",
    "ConsoleServer",
    "Other",
];

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// 表示範囲。既定は「現在このプロジェクトにあるもの」。
///
/// **A-6により、過去に所属した機器も閲覧できる。**ただし既定で混ぜると、
/// 移管済みの機器が現役と並んでしまう。16.1の「完了済みを隠せるフィルタは必須」
/// と同じ考え方で、既定を現在に絞り、切り替えで過去を出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Current,
    All,
}

impl Scope {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("all") => Self::All,
            _ => Self::Current,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::All => "all",
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let scope = Scope::parse(query.scope.as_deref());
    let keyword = query.q.clone().unwrap_or_default();

    let devices = 検索(&state.db, project_id, &keyword)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // 表示に要る情報をまとめて引く。行ごとに問い合わせるとN+1になる
    let 所在 = 現在の所在(&state.db, &devices).await?;
    let 型名 = 構成名(&state.db, &devices).await?;

    let mut rows = Vec::new();
    for d in devices {
        let 現在地 = 所在.iter().find(|(id, _)| *id == d.id).map(|(_, a)| a);
        let このプロジェクトにいる =
            現在地.is_some_and(|a| a.location_type == PROJECT && a.location_id == Some(project_id));

        if scope == Scope::Current && !このプロジェクトにいる {
            continue;
        }

        rows.push(DeviceRow {
            model: 型名
                .iter()
                .find(|(id, _)| *id == d.id)
                .map(|(_, n)| n.clone())
                .or_else(|| d.device_category.clone())
                .unwrap_or_default(),
            location: match 現在地 {
                Some(a) => 所在の表示(&state, a, l).await?,
                None => rust_i18n::t!("devices.location_unknown", locale = l).to_string(),
            },
            departed: !このプロジェクトにいる,
            planned: d.status == PLAN,
            status: d.status,
            id: d.id,
            hostname: d.hostname,
            device_type: d.device_type,
        });
    }

    render(&DevicesPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "projects"),
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("devices.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("devices.lead", locale = l).to_string(),
        t_new: rust_i18n::t!("devices.new", locale = l).to_string(),
        t_import: rust_i18n::t!("import.title", locale = l).to_string(),
        t_work_orders: rust_i18n::t!("work_orders.title", locale = l).to_string(),
        t_members: rust_i18n::t!("members.title", locale = l).to_string(),
        t_keyword: rust_i18n::t!("devices.keyword", locale = l).to_string(),
        t_search: rust_i18n::t!("common.search", locale = l).to_string(),
        t_scope_current: rust_i18n::t!("devices.scope_current", locale = l).to_string(),
        t_scope_all: rust_i18n::t!("devices.scope_all", locale = l).to_string(),
        t_hostname: rust_i18n::t!("devices.hostname", locale = l).to_string(),
        t_device_type: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_model: rust_i18n::t!("devices.model", locale = l).to_string(),
        t_status: rust_i18n::t!("devices.status", locale = l).to_string(),
        t_location: rust_i18n::t!("devices.location", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_empty: rust_i18n::t!("devices.empty", locale = l).to_string(),
        t_departed: rust_i18n::t!("devices.departed", locale = l).to_string(),
        t_planned: rust_i18n::t!("devices.planned", locale = l).to_string(),
        q: keyword,
        scope: scope.as_str().to_owned(),
        rows,
        can_edit,
    })
}

/// **A-6の判定を含む検索。**
///
/// `DEVICE_ASSIGNMENT` に `(Project, project_id)` の行が一度でも現れる機器を返す。
/// **`to_date` で絞らない**のが要点で、絞ると移設済みの機器の履歴が見えなくなる。
///
/// 統合で吸収された機器（`merged_into_device_id` あり）は除く。重複として
/// 畳まれた側であり、一覧に出すと同じ機器が2行に見える（設計書23.9）。
async fn 検索<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    keyword: &str,
) -> Result<Vec<device::Model>, sea_orm::DbErr> {
    let 所属したことがある = SeaQuery::select()
        .column(device_assignment::Column::DeviceId)
        .from(device_assignment::Entity)
        .and_where(Expr::col(device_assignment::Column::LocationType).eq(PROJECT))
        .and_where(Expr::col(device_assignment::Column::LocationId).eq(project_id))
        .to_owned();

    let mut query = device::Entity::find()
        .filter(device::Column::Id.in_subquery(所属したことがある))
        .filter(device::Column::MergedIntoDeviceId.is_null());

    let keyword = keyword.trim();
    if !keyword.is_empty() {
        let pattern = format!("%{}%", escape_like(&keyword.to_lowercase()));
        let like = |column| {
            Expr::expr(Func::lower(Expr::col(column))).like(LikeExpr::new(&pattern).escape('\\'))
        };
        query = query.filter(
            like(device::Column::Hostname)
                .or(like(device::Column::SerialNumber))
                .or(like(device::Column::AssetNumber)),
        );
    }

    query
        .order_by_asc(device::Column::Hostname)
        .order_by_asc(device::Column::Id)
        .all(db)
        .await
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 各機器の現在の所在（`to_date IS NULL` の1行）。
async fn 現在の所在(
    db: &sea_orm::DatabaseConnection,
    devices: &[device::Model],
) -> AppResult<Vec<(i32, device_assignment::Model)>> {
    if devices.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i32> = devices.iter().map(|d| d.id).collect();

    let rows = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.is_in(ids))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(rows.into_iter().map(|a| (a.device_id, a)).collect())
}

/// 所在の表示名。プロジェクト名・倉庫名まで解決する。
async fn 所在の表示(
    state: &AppState,
    assignment: &device_assignment::Model,
    locale: &str,
) -> AppResult<String> {
    let 名前 = match (assignment.location_type.as_str(), assignment.location_id) {
        (PROJECT, Some(id)) => project::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|p| p.name),
        ("Warehouse", Some(id)) => entity::warehouse::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|w| w.name),
        _ => None,
    };

    if let Some(name) = 名前 {
        return Ok(format!("{}：{name}", assignment.location_type));
    }

    // **キーを実行時に組み立てない。**組み立てると、語彙が増えたときに
    // 翻訳の欠落がコンパイルでも起動時でも表面化しない
    Ok(match assignment.location_type.as_str() {
        // Disposed は location_id を持たない（設計書6.2）
        "Disposed" => rust_i18n::t!("devices.location_disposed", locale = locale).to_string(),
        other => other.to_owned(),
    })
}

/// 各機器の型名（`CONFIGURATION` → `CHASSIS_MODEL` → `VENDOR`）。
async fn 構成名(
    db: &sea_orm::DatabaseConnection,
    devices: &[device::Model],
) -> AppResult<Vec<(i32, String)>> {
    let config_ids: Vec<i32> = devices.iter().filter_map(|d| d.configuration_id).collect();
    if config_ids.is_empty() {
        return Ok(Vec::new());
    }

    let configs = configuration::Entity::find()
        .filter(configuration::Column::Id.is_in(config_ids))
        .find_also_related(chassis_model::Entity)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let vendor_ids: Vec<i32> = configs
        .iter()
        .filter_map(|(_, m)| m.as_ref().map(|m| m.vendor_id))
        .collect();
    let vendors = if vendor_ids.is_empty() {
        Vec::new()
    } else {
        vendor::Entity::find()
            .filter(vendor::Column::Id.is_in(vendor_ids))
            .all(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    Ok(devices
        .iter()
        .filter_map(|d| {
            let cid = d.configuration_id?;
            let (config, model) = configs.iter().find(|(c, _)| c.id == cid)?;
            let model = model.as_ref()?;
            let vendor = vendors.iter().find(|v| v.id == model.vendor_id);
            Some((
                d.id,
                match vendor {
                    Some(v) => format!("{} {} / {}", v.name, model.model_name, config.name),
                    None => format!("{} / {}", model.model_name, config.name),
                },
            ))
        })
        .collect())
}

// ---------------------------------------------------------------------------
// 詳細
// ---------------------------------------------------------------------------

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let d = 対象(&state, project_id, device_id).await?;

    let 未設定 = rust_i18n::t!("devices.none", locale = l).to_string();
    let 空欄 = |value: Option<String>| value.unwrap_or_else(|| 未設定.clone());

    let basic = vec![
        Labeled {
            label: rust_i18n::t!("devices.device_type", locale = l).to_string(),
            value: d.device_type.clone(),
        },
        Labeled {
            label: rust_i18n::t!("devices.model", locale = l).to_string(),
            value: 構成名(&state.db, std::slice::from_ref(&d))
                .await?
                .first()
                .map(|(_, n)| n.clone())
                .or_else(|| d.device_category.clone())
                .unwrap_or_else(|| 未設定.clone()),
        },
        Labeled {
            label: rust_i18n::t!("devices.serial_number", locale = l).to_string(),
            value: 空欄(d.serial_number.clone()),
        },
        Labeled {
            label: rust_i18n::t!("devices.asset_number", locale = l).to_string(),
            value: 空欄(d.asset_number.clone()),
        },
        Labeled {
            label: rust_i18n::t!("devices.power_watt", locale = l).to_string(),
            value: format!("{} W", d.power_watt),
        },
        Labeled {
            label: rust_i18n::t!("devices.status", locale = l).to_string(),
            value: d.status.clone(),
        },
        Labeled {
            label: "UID".to_owned(),
            value: d.uid.clone(),
        },
    ];

    // 所在は履歴をすべて出す。**A-6が保証するのはこの履歴の閲覧**であり、
    // 現在地だけを見せるのでは移設の経緯が追えない
    let mut locations = Vec::new();
    for a in device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(d.id))
        .order_by_desc(device_assignment::Column::FromDate)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        let 期間 = match a.to_date {
            Some(to) => format!(
                "{} 〜 {}",
                a.from_date.format("%Y-%m-%d"),
                to.format("%Y-%m-%d")
            ),
            None => format!("{} 〜", a.from_date.format("%Y-%m-%d")),
        };
        locations.push(Labeled {
            label: 期間,
            value: 所在の表示(&state, &a, l).await?,
        });
    }

    render(&DeviceDetailPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "projects"),
        project_id,
        project_name: project.name,
        device_id: d.id,
        t_back: rust_i18n::t!("devices.back", locale = l).to_string(),
        t_edit: rust_i18n::t!("projects.edit", locale = l).to_string(),
        t_basic: rust_i18n::t!("devices.basic", locale = l).to_string(),
        t_location_history: rust_i18n::t!("devices.location_history", locale = l).to_string(),
        t_mount: rust_i18n::t!("devices.mount", locale = l).to_string(),
        t_parts: rust_i18n::t!("devices.parts", locale = l).to_string(),
        t_firmware: rust_i18n::t!("devices.firmware", locale = l).to_string(),
        t_stack: rust_i18n::t!("devices.stack", locale = l).to_string(),
        t_none: 未設定,
        t_planned: rust_i18n::t!("devices.planned", locale = l).to_string(),
        t_not_implemented: rust_i18n::t!("devices.section_pending", locale = l).to_string(),
        t_merged: rust_i18n::t!("devices.merged", locale = l).to_string(),
        hostname: d.hostname.clone(),
        planned: d.status == PLAN,
        merged_into: d.merged_into_device_id,
        basic,
        locations,
        mount: 搭載位置(&state, &d, l).await?,
        parts: 搭載部品(&state, &d).await?,
        firmware: ファームウェア(&state, &d).await?,
        stack: スタック構成(&state, &d).await?,
        can_edit,
    })
}

/// 現在の搭載位置（`DEVICE_MOUNT` の `to_date IS NULL`）。
async fn 搭載位置(
    state: &AppState,
    d: &device::Model,
    locale: &str,
) -> AppResult<Vec<Labeled>> {
    let Some(m) = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(d.id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok(Vec::new());
    };

    let mut rows = Vec::new();

    // container_id と host_device_id は排他（設計書12.2）
    if let Some(container_id) = m.container_id {
        let name = entity::mount_container::Entity::find_by_id(container_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|c| c.name)
            .unwrap_or_default();
        rows.push(Labeled {
            label: rust_i18n::t!("devices.container", locale = locale).to_string(),
            value: name,
        });
    }
    if let Some(host_id) = m.host_device_id {
        let name = device::Entity::find_by_id(host_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|h| h.hostname)
            .unwrap_or_default();
        rows.push(Labeled {
            label: rust_i18n::t!("devices.host_device", locale = locale).to_string(),
            value: name,
        });
    }
    if let Some(position) = m.position {
        rows.push(Labeled {
            label: rust_i18n::t!("devices.position", locale = locale).to_string(),
            value: position.to_string(),
        });
    }

    Ok(rows)
}

/// 現在搭載されている部品。
async fn 搭載部品(state: &AppState, d: &device::Model) -> AppResult<Vec<Labeled>> {
    let locations = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq("Device"))
        .filter(part_instance_location::Column::LocationId.eq(d.id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for l in locations {
        let Some(instance) = part_instance::Entity::find_by_id(l.part_instance_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            continue;
        };
        let catalog = part_catalog::Entity::find_by_id(instance.part_catalog_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

        rows.push(Labeled {
            label: catalog
                .as_ref()
                .map(|c| format!("{} {}", c.category, c.part_number))
                .unwrap_or_default(),
            value: instance.serial_number.unwrap_or_default(),
        });
    }
    Ok(rows)
}

/// 現在有効なファームウェアの版。
async fn ファームウェア(state: &AppState, d: &device::Model) -> AppResult<Vec<Labeled>> {
    Ok(firmware_version::Entity::find()
        .filter(firmware_version::Column::ItemType.eq("Device"))
        .filter(firmware_version::Column::ItemId.eq(d.id))
        .filter(firmware_version::Column::ToDate.is_null())
        .order_by_asc(firmware_version::Column::Component)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|f| Labeled {
            label: f.component,
            value: f.version,
        })
        .collect())
}

/// スタックの構成筐体（`device_type="Logical"` の場合）。
async fn スタック構成(state: &AppState, d: &device::Model) -> AppResult<Vec<Labeled>> {
    let members = device_stack::Entity::find()
        .filter(device_stack::Column::LogicalDeviceId.eq(d.id))
        .filter(device_stack::Column::ToDate.is_null())
        .order_by_asc(device_stack::Column::MemberNumber)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for m in members {
        let name = device::Entity::find_by_id(m.member_device_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|x| x.hostname)
            .unwrap_or_default();
        rows.push(Labeled {
            label: format!("#{}", m.member_number),
            value: name,
        });
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// 登録・編集
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeviceForm {
    pub hostname: String,
    #[serde(default)]
    pub device_type: String,
    #[serde(default)]
    pub configuration_id: String,
    #[serde(default)]
    pub device_category: String,
    #[serde(default)]
    pub serial_number: String,
    #[serde(default)]
    pub asset_number: String,
    #[serde(default)]
    pub power_watt: String,
    #[serde(default)]
    pub status: String,
}

/// 検証を通った入力。**保存してよいのはここに入った値だけ。**
struct 検証済み {
    hostname: String,
    device_type: String,
    configuration_id: Option<i32>,
    device_category: Option<String>,
    serial_number: Option<String>,
    asset_number: Option<String>,
    power_watt: i32,
    status: String,
}

impl DeviceForm {
    fn 空ならnone(value: &str) -> Option<String> {
        match value.trim() {
            "" => None,
            v => Some(v.to_owned()),
        }
    }

    /// 語彙に含まれていれば返す。**含まれていなければ `None`。**
    ///
    /// **既定値へ倒さない。**選択肢はサーバが描画しているため、語彙外の値が
    /// 届くのは改竄かクライアントの不具合しかありえない。黙って別の値を保存
    /// すると、どちらの場合も気付けない（設計書8.6、Q-21）。
    fn 語彙(value: &str, allowed: &[&'static str]) -> Option<String> {
        allowed
            .iter()
            .find(|v| **v == value.trim())
            .map(|v| (*v).to_owned())
    }
}

/// 入力を検証する。誤りがあれば表示用のi18nキーを返す。
///
/// **型・語彙の整合性は緩めない。**不変条件6の「検証は原則ハードな禁止ではなく
/// 警告」は「実機が仕様の想定外でありうる」ことへの配慮（スロット本数の超過等）
/// であって、送られてきた値が解釈できない場合の話ではない。
async fn 検証(
    state: &AppState,
    form: &DeviceForm,
) -> AppResult<Result<検証済み, &'static str>> {
    let hostname = form.hostname.trim().to_owned();
    if hostname.is_empty() {
        return Ok(Err("devices.hostname_required"));
    }

    let Some(device_type) = DeviceForm::語彙(&form.device_type, DEVICE_TYPES) else {
        return Ok(Err("devices.device_type_invalid"));
    };
    let Some(status) = DeviceForm::語彙(&form.status, STATUSES) else {
        return Ok(Err("devices.status_invalid"));
    };

    // 構成を持たない機器（仮想アプライアンス等）があるため空欄は許す
    let device_category = match DeviceForm::空ならnone(&form.device_category) {
        None => None,
        Some(value) => match DeviceForm::語彙(&value, CATEGORIES) {
            Some(v) => Some(v),
            None => return Ok(Err("devices.device_category_invalid")),
        },
    };

    // Virtual / Container / Logical は構成を持たない（設計書6.2）
    let configuration_id = match DeviceForm::空ならnone(&form.configuration_id) {
        None => None,
        Some(value) => {
            let Ok(id) = value.parse::<i32>() else {
                return Ok(Err("devices.configuration_invalid"));
            };
            // **存在も確かめる。**確かめないと外部キー違反で500になり、
            // 利用者には何が悪いのか分からない
            let 実在 = configuration::Entity::find_by_id(id)
                .one(&state.db)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .is_some();
            if !実在 {
                return Ok(Err("devices.configuration_invalid"));
            }
            Some(id)
        }
    };

    let power_watt = match form.power_watt.trim() {
        "" => 0,
        value => match value.parse::<i32>() {
            Ok(v) if v >= 0 => v,
            _ => return Ok(Err("devices.power_watt_invalid")),
        },
    };

    Ok(Ok(検証済み {
        hostname,
        device_type,
        configuration_id,
        device_category,
        serial_number: DeviceForm::空ならnone(&form.serial_number),
        // 採番待ちでも登録できる（設計書23.2）
        asset_number: DeviceForm::空ならnone(&form.asset_number),
        power_watt,
        status,
    }))
}

pub async fn new_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let form = DeviceForm {
        hostname: String::new(),
        device_type: "Physical".to_owned(),
        configuration_id: String::new(),
        device_category: String::new(),
        serial_number: String::new(),
        asset_number: String::new(),
        power_watt: "0".to_owned(),
        status: "building".to_owned(),
    };
    render(&フォーム(&state, &current, &project, None, &form, None).await?)
}

async fn フォーム(
    state: &AppState,
    current: &CurrentUser,
    project: &project::Model,
    device_id: Option<i32>,
    form: &DeviceForm,
    error: Option<String>,
) -> AppResult<DeviceFormPage> {
    let l = Locale::parse(&current.user.locale).as_str();
    let 新規 = device_id.is_none();

    Ok(DeviceFormPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "projects"),
        project_id: project.id,
        project_name: project.name.clone(),
        t_title: if 新規 {
            rust_i18n::t!("devices.new_title", locale = l).to_string()
        } else {
            rust_i18n::t!("devices.edit_title", locale = l).to_string()
        },
        t_lead: if 新規 {
            rust_i18n::t!("devices.new_lead", locale = l).to_string()
        } else {
            rust_i18n::t!("devices.edit_lead", locale = l).to_string()
        },
        t_back: rust_i18n::t!("devices.back", locale = l).to_string(),
        t_hostname: rust_i18n::t!("devices.hostname", locale = l).to_string(),
        t_device_type: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_configuration: rust_i18n::t!("devices.configuration", locale = l).to_string(),
        t_configuration_hint: rust_i18n::t!("devices.configuration_hint", locale = l).to_string(),
        t_device_category: rust_i18n::t!("devices.device_category", locale = l).to_string(),
        t_device_category_hint: rust_i18n::t!("devices.device_category_hint", locale = l)
            .to_string(),
        t_serial_number: rust_i18n::t!("devices.serial_number", locale = l).to_string(),
        t_serial_hint: rust_i18n::t!("devices.serial_hint", locale = l).to_string(),
        t_asset_number: rust_i18n::t!("devices.asset_number", locale = l).to_string(),
        t_asset_hint: rust_i18n::t!("devices.asset_hint", locale = l).to_string(),
        t_power_watt: rust_i18n::t!("devices.power_watt", locale = l).to_string(),
        t_status: rust_i18n::t!("devices.status", locale = l).to_string(),
        t_submit: if 新規 {
            rust_i18n::t!("common.create", locale = l).to_string()
        } else {
            rust_i18n::t!("common.save", locale = l).to_string()
        },
        action: match device_id {
            Some(id) => format!("/projects/{}/devices/{id}", project.id),
            None => format!("/projects/{}/devices", project.id),
        },
        hostname: form.hostname.clone(),
        device_type: form.device_type.clone(),
        device_types: DEVICE_TYPES.to_vec(),
        configuration_id: form.configuration_id.clone(),
        configurations: 構成の候補(state).await?,
        device_category: form.device_category.clone(),
        categories: CATEGORIES.to_vec(),
        serial_number: form.serial_number.clone(),
        asset_number: form.asset_number.clone(),
        power_watt: form.power_watt.clone(),
        status: form.status.clone(),
        statuses: STATUSES.to_vec(),
        error,
    })
}

/// 構成の選択肢。カタログはプロジェクト横断の共有マスタ（設計書18.1）。
async fn 構成の候補(state: &AppState) -> AppResult<Vec<Labeled>> {
    let configs = configuration::Entity::find()
        .find_also_related(chassis_model::Entity)
        .order_by_asc(configuration::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(configs
        .into_iter()
        .map(|(c, model)| Labeled {
            label: match model {
                Some(m) => format!("{} / {}", m.model_name, c.name),
                None => c.name.clone(),
            },
            value: c.id.to_string(),
        })
        .collect())
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<DeviceForm>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let 入力 = match 検証(&state, &form).await? {
        Ok(値) => 値,
        Err(key) => {
            let page = フォーム(
                &state,
                &current,
                &project,
                None,
                &form,
                Some(rust_i18n::t!(key, locale = l).to_string()),
            )
            .await?;
            return render(&page);
        }
    };

    let now = Utc::now();
    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let created = tx
        .insert(device::ActiveModel {
            // 名前を変えても参照が切れない不変の識別子（設計書23.2）
            uid: Set(uuid::Uuid::new_v4().to_string()),
            external_id: Set(None),
            merged_into_device_id: Set(None),
            merged_at: Set(None),
            configuration_id: Set(入力.configuration_id),
            device_type: Set(入力.device_type),
            device_category: Set(入力.device_category),
            hostname: Set(入力.hostname),
            serial_number: Set(入力.serial_number),
            asset_number: Set(入力.asset_number),
            power_watt: Set(入力.power_watt),
            status: Set(入力.status),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **所在は DEVICE から導出するため、割当の行が要る**（旧B-1）。
    // これを書かないと、登録した機器がどの一覧にも出てこない
    tx.insert(device_assignment::ActiveModel {
        device_id: Set(created.id),
        location_type: Set(PROJECT.to_owned()),
        location_id: Set(Some(project_id)),
        work_order_id: Set(None),
        from_date: Set(now),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/devices")).into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let d = 対象(&state, project_id, device_id).await?;
    let page = フォーム(&state, &current, &project, Some(d.id), &既存の値(&d), None).await?;
    render(&page)
}

fn 既存の値(d: &device::Model) -> DeviceForm {
    DeviceForm {
        hostname: d.hostname.clone(),
        device_type: d.device_type.clone(),
        configuration_id: d
            .configuration_id
            .map(|v| v.to_string())
            .unwrap_or_default(),
        device_category: d.device_category.clone().unwrap_or_default(),
        serial_number: d.serial_number.clone().unwrap_or_default(),
        asset_number: d.asset_number.clone().unwrap_or_default(),
        power_watt: d.power_watt.to_string(),
        status: d.status.clone(),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<DeviceForm>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let target = 対象(&state, project_id, device_id).await?;

    let 入力 = match 検証(&state, &form).await? {
        Ok(値) => 値,
        Err(key) => {
            let page = フォーム(
                &state,
                &current,
                &project,
                Some(target.id),
                &form,
                Some(rust_i18n::t!(key, locale = l).to_string()),
            )
            .await?;
            return render(&page);
        }
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mut active: device::ActiveModel = target.clone().into();
    active.hostname = Set(入力.hostname);
    active.device_type = Set(入力.device_type);
    active.configuration_id = Set(入力.configuration_id);
    active.device_category = Set(入力.device_category);
    active.serial_number = Set(入力.serial_number);
    active.asset_number = Set(入力.asset_number);
    active.power_watt = Set(入力.power_watt);
    active.status = Set(入力.status);
    active.updated_at = Set(Utc::now());
    tx.update(&target, active)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/devices/{device_id}")).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// 閲覧の可否を確かめ、プロジェクトと「編集できるか」を返す。
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

/// 編集の可否を確かめる。**ApproverとViewerはここで止まる。**
async fn 編集入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<project::Model> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    if !can_edit {
        return Err(AppError::Forbidden);
    }
    Ok(project)
}

/// 対象の機器。**このプロジェクトから見える機器に限る**（A-6）。
///
/// IDを直接叩かれても、所属したことのないプロジェクトからは見えない。
async fn 対象(state: &AppState, project_id: i32, device_id: i32) -> AppResult<device::Model> {
    let 見える = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .limit(1)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some();

    if !見える {
        return Err(AppError::NotFound);
    }

    device::Entity::find_by_id(device_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 表示範囲の既定は現在所属しているもの() {
        assert_eq!(Scope::parse(None), Scope::Current);
        assert_eq!(Scope::parse(Some("all")), Scope::All);
    }

    /// **語彙外は `None`。既定値へ倒さない。**倒すと改竄も不具合も気付けない。
    #[test]
    fn 語彙外の値は受け付けない() {
        assert_eq!(
            DeviceForm::語彙("Virtual", DEVICE_TYPES),
            Some("Virtual".to_owned())
        );
        assert_eq!(DeviceForm::語彙("なりすまし", DEVICE_TYPES), None);
        assert_eq!(DeviceForm::語彙("", STATUSES), None);
    }

    #[test]
    fn 空欄はnullとして扱う() {
        assert_eq!(DeviceForm::空ならnone("  "), None);
        assert_eq!(DeviceForm::空ならnone(" S1 "), Some("S1".to_owned()));
    }
}
