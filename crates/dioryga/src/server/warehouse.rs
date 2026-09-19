//! 倉庫領域（設計書16.1のC領域、12章、12.4）。
//!
//! プロジェクトを横断する。`DEVICE_ASSIGNMENT.location_type="Warehouse"` と
//! `PART_INSTANCE_LOCATION.location_type="Warehouse"` の受け皿にあたる。
//!
//! # 権限はカタログ領域に揃える（16.1）
//!
//! | 操作 | 権限 |
//! |---|---|
//! | 倉庫・在庫の閲覧 | いずれか1つ以上のプロジェクトのメンバー |
//! | 倉庫の登録・編集 | いずれか1つ以上のプロジェクトで `Operator` 以上 |
//!
//! **System Adminは入れない。**倉庫にある機器は**過去にプロジェクトへ属していた
//! 履歴を持ち続ける**（A-6）ため、倉庫を経由すればプロジェクト内データが見える。
//! 「倉庫はプロジェクト外だからSystem Adminの管轄」という整理は、この一点で
//! 成立しない（3章）。`system_admin_guard` とハンドラの双方で拒否する。
//!
//! # 倉庫の中は全件・全履歴を見せる（16.1）
//!
//! **A-6の但し書き（無関係な他プロジェクトのデータは見えない）に対する、意図した
//! 例外である。**倉庫を経由すると、自分が関わったことのない機器の過去の所属先が
//! 見える。
//!
//! そう決めたのは、**倉庫一覧が答えるべき問いが「予備はあるか」だから**である。
//! A-6をそのまま適用すると自分のプロジェクトに属したことがある機器しか在庫に
//! 出ず、**新品の予備は誰のプロジェクトにも属したことがないため、最も見たいものが
//! 最も見えない。**
//!
//! **A-6の判定をこの画面に持ち込まないこと。**払い出された後の機器には通常どおり
//! A-6が効く。例外は倉庫にある機器に閉じている。
//!
//! # 機器・パーツの一覧は読み取り専用（16.1）
//!
//! **プロジェクトと倉庫の間で機器を動かす導線はここに置かない。**11章の変更管理
//! チケット（Transfer/Relocation）の担当である。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    device, device_assignment, part_catalog, part_instance, part_instance_location, project,
    warehouse,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::正規化;
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

const WAREHOUSE: &str = "Warehouse";
const PROJECT: &str = "Project";

// ---------------------------------------------------------------------------
// 倉庫一覧（定型CRUD）
// ---------------------------------------------------------------------------

struct WarehouseRow {
    id: i32,
    name: String,
    address: String,
    /// 廃止済み（#133）。**既定の一覧からは隠す**（カタログの廃番と同じ、18.5）。
    retired: bool,
    /// 保管中の機器の台数。**空の倉庫と、まだ数えていない倉庫を区別する。**
    devices: usize,
    /// 保管中のパーツの点数。
    parts: usize,
}

#[derive(askama::Template)]
#[template(path = "warehouses.html")]
struct WarehousesPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_name: String,
    t_address: String,
    t_devices: String,
    t_parts: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_edit: String,
    t_delete: String,
    t_retired: String,
    t_unretire: String,
    t_show_retired: String,
    t_apply: String,
    rows: Vec<WarehouseRow>,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    一覧を描く(&state, &current, 廃止を含む(&query), None).await
}

fn 廃止を含む(query: &ListQuery) -> bool {
    query.retired.as_deref() == Some("1")
}

/// 一覧の絞り込み。**既定では廃止を隠す**（18.5と同じ）。
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub retired: Option<String>,
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    show_retired: bool,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current).await?;
    let can_edit = 編集権(state, current).await.is_ok();

    let list = warehouse::Entity::find()
        .order_by_asc(warehouse::Column::Name)
        .order_by_asc(warehouse::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **在庫を1倉庫ずつ引かない。**現在有効な行をまとめて取り、倉庫ごとに数える
    let 機器 = 倉庫にある機器の割当(state).await?;
    let パーツ = 倉庫にあるパーツの所在(state).await?;

    let rows = list
        .into_iter()
        .filter(|w| show_retired || w.retired_at.is_none())
        .map(|w| WarehouseRow {
            retired: w.retired_at.is_some(),
            devices: 機器.iter().filter(|a| a.location_id == Some(w.id)).count(),
            parts: パーツ
                .iter()
                .filter(|p| p.location_id == Some(w.id))
                .count(),
            id: w.id,
            name: w.name,
            address: w.address,
        })
        .collect();

    render(&WarehousesPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "warehouses"),
        t_title: rust_i18n::t!("warehouses.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("warehouses.lead", locale = l).to_string(),
        t_name: rust_i18n::t!("warehouses.name", locale = l).to_string(),
        t_address: rust_i18n::t!("warehouses.address", locale = l).to_string(),
        t_devices: rust_i18n::t!("warehouses.devices", locale = l).to_string(),
        t_parts: rust_i18n::t!("warehouses.parts", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("warehouses.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("warehouses.new", locale = l).to_string(),
        t_edit: rust_i18n::t!("warehouses.edit", locale = l).to_string(),
        t_delete: rust_i18n::t!("warehouses.delete", locale = l).to_string(),
        t_retired: rust_i18n::t!("warehouses.retired", locale = l).to_string(),
        t_unretire: rust_i18n::t!("warehouses.unretire", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("warehouses.show_retired", locale = l).to_string(),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        rows,
        show_retired,
        can_edit,
        error,
    })
}

/// 登録・編集の画面（#133）。共有カタログ（#124）と同じく一覧から分ける。
#[derive(askama::Template)]
#[template(path = "warehouse_form.html")]
struct WarehouseFormPage {
    chrome: Chrome,
    t_title: String,
    t_back: String,
    t_name: String,
    t_address: String,
    t_submit: String,
    /// 送り先。登録は `/warehouses`、編集は `/warehouses/{id}`。
    action: String,
    v_name: String,
    v_address: String,
    error: Option<String>,
}

/// 削除の確認画面（#133）。**何が起きるかを示してから実行する。**
#[derive(askama::Template)]
#[template(path = "warehouse_delete.html")]
struct WarehouseDeletePage {
    chrome: Chrome,
    t_title: String,
    t_back: String,
    t_lead: String,
    t_submit: String,
    t_cancel: String,
    warehouse_id: i32,
    name: String,
    /// 実行できるか。中にものが残っていれば押せない。
    can_delete: bool,
}

/// 「削除」を押したときに何が起きるか（#133）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum 削除の結末 {
    /// 現在、機器・部品がある。**できない。**
    使用中,
    /// 空で、所在の履歴にも一度も現れない。**行ごと消せる。**
    物理削除,
    /// 空だが過去に使った。**廃止にする**（履歴が指し続けるため消せない。24.5）。
    廃止,
}

/// 倉庫の状態を調べる（#133）。
///
/// **見るのは所在の2表だけ。**`location_type="Warehouse"` の行が、いま開いて
/// いるか（`to_date IS NULL`）、過去に閉じられているかで結末が決まる。
async fn 削除の判定(state: &AppState, id: i32) -> AppResult<削除の結末> {
    let 機器 = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(WAREHOUSE))
        .filter(device_assignment::Column::LocationId.eq(id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let 部品 = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(WAREHOUSE))
        .filter(part_instance_location::Column::LocationId.eq(id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    if 機器.iter().any(|a| a.to_date.is_none()) || 部品.iter().any(|p| p.to_date.is_none()) {
        return Ok(削除の結末::使用中);
    }
    if 機器.is_empty() && 部品.is_empty() {
        return Ok(削除の結末::物理削除);
    }
    Ok(削除の結末::廃止)
}

async fn 入力画面を描く(
    state: &AppState,
    current: &CurrentUser,
    id: Option<i32>,
    form: &WarehouseForm,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current).await?;
    編集権(state, current).await?;

    render(&WarehouseFormPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "warehouses"),
        t_title: match id {
            Some(_) => rust_i18n::t!("warehouses.edit_title", locale = l).to_string(),
            None => rust_i18n::t!("warehouses.new", locale = l).to_string(),
        },
        t_back: rust_i18n::t!("warehouses.back", locale = l).to_string(),
        t_name: rust_i18n::t!("warehouses.name", locale = l).to_string(),
        t_address: rust_i18n::t!("warehouses.address", locale = l).to_string(),
        t_submit: rust_i18n::t!("common.save", locale = l).to_string(),
        action: match id {
            Some(id) => format!("/warehouses/{id}"),
            None => "/warehouses".to_owned(),
        },
        v_name: form.name.clone(),
        v_address: form.address.clone(),
        error,
    })
}

pub async fn new_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> AppResult<Response> {
    入力画面を描く(&state, &current, None, &WarehouseForm::default(), None).await
}

pub async fn edit_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let w = 倉庫(&state, id).await?;
    let form = WarehouseForm {
        name: w.name,
        address: w.address,
    };
    入力画面を描く(&state, &current, Some(id), &form, None).await
}

/// 削除の確認画面（#133）。
pub async fn delete_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let l = 入場(&state, &current).await?;
    編集権(&state, &current).await?;
    let w = 倉庫(&state, id).await?;
    let 結末 = 削除の判定(&state, id).await?;

    render(&WarehouseDeletePage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "warehouses"),
        t_title: rust_i18n::t!("warehouses.delete_title", locale = l).to_string(),
        t_back: rust_i18n::t!("warehouses.back", locale = l).to_string(),
        t_lead: match 結末 {
            削除の結末::使用中 => {
                rust_i18n::t!("warehouses.delete_in_use", locale = l).to_string()
            }
            削除の結末::物理削除 => {
                rust_i18n::t!("warehouses.delete_hard", locale = l).to_string()
            }
            削除の結末::廃止 => {
                rust_i18n::t!("warehouses.delete_retire", locale = l).to_string()
            }
        },
        t_submit: rust_i18n::t!("warehouses.delete", locale = l).to_string(),
        t_cancel: rust_i18n::t!("warehouses.back", locale = l).to_string(),
        warehouse_id: id,
        name: w.name,
        can_delete: 結末 != 削除の結末::使用中,
    })
}

/// 削除を実行する（#133）。
///
/// **中にものが残っていればできない。**空でも、過去に使った倉庫は廃止に留める
/// ——所在の履歴が指し続けており、消すと参照先を失う（24.5）。
pub async fn delete(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let l = 入場(&state, &current).await?;
    編集権(&state, &current).await?;
    let w = 倉庫(&state, id).await?;

    let 結末 = 削除の判定(&state, id).await?;
    if 結末 == 削除の結末::使用中 {
        let 誤り = rust_i18n::t!("warehouses.error_in_use", locale = l).to_string();
        return 一覧を描く(&state, &current, false, Some(誤り)).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    match 結末 {
        削除の結末::物理削除 => tx
            .delete(w.clone())
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?,
        _ => {
            tx.update(
                &w,
                warehouse::ActiveModel {
                    id: Set(id),
                    retired_at: Set(Some(Utc::now())),
                    updated_at: Set(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to("/warehouses").into_response())
}

/// 廃止を取り消す（#133）。カタログの「廃番を取り消す」（18.5）と同じ。
pub async fn unretire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    入場(&state, &current).await?;
    編集権(&state, &current).await?;
    let w = 倉庫(&state, id).await?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &w,
        warehouse::ActiveModel {
            id: Set(id),
            retired_at: Set(None),
            updated_at: Set(Utc::now()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to("/warehouses?retired=1").into_response())
}

#[derive(Debug, Default, Deserialize)]
pub struct WarehouseForm {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<WarehouseForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current).await?;
    編集権(&state, &current).await?;

    let name = 正規化(&form.name);
    if name.is_empty() {
        let 誤り = rust_i18n::t!("catalog.error_name", locale = l).to_string();
        return 入力画面を描く(&state, &current, None, &form, Some(誤り)).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(warehouse::ActiveModel {
        name: Set(name),
        address: Set(正規化(&form.address)),
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

    Ok(Redirect::to("/warehouses").into_response())
}

pub async fn update(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<WarehouseForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current).await?;
    編集権(&state, &current).await?;

    let before = warehouse::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let name = 正規化(&form.name);
    if name.is_empty() {
        let 誤り = rust_i18n::t!("catalog.error_name", locale = l).to_string();
        return 入力画面を描く(&state, &current, Some(id), &form, Some(誤り)).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        warehouse::ActiveModel {
            id: Set(id),
            name: Set(name),
            address: Set(正規化(&form.address)),
            updated_at: Set(Utc::now()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to("/warehouses").into_response())
}

// ---------------------------------------------------------------------------
// 倉庫内機器一覧（読み取り専用）
// ---------------------------------------------------------------------------

struct StoredDeviceRow {
    id: i32,
    hostname: String,
    device_type: String,
    status: String,
    serial_number: String,
    /// この倉庫に入った日。
    since: String,
}

#[derive(askama::Template)]
#[template(path = "warehouse_devices.html")]
struct DevicesPage {
    chrome: Chrome,
    warehouse_id: i32,
    warehouse_name: String,
    t_title: String,
    t_lead: String,
    t_hostname: String,
    t_device_type: String,
    t_status: String,
    t_serial: String,
    t_since: String,
    t_actions: String,
    t_detail: String,
    t_empty: String,
    rows: Vec<StoredDeviceRow>,
}

pub async fn devices(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let l = 入場(&state, &current).await?;
    let w = 倉庫(&state, id).await?;

    // **全件出す。**A-6の判定をここに持ち込むと、誰のプロジェクトにも属した
    // ことがない新品の予備が誰にも見えなくなる（16.1）
    let 割当 = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(WAREHOUSE))
        .filter(device_assignment::Column::LocationId.eq(id))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let ids: Vec<i32> = 割当.iter().map(|a| a.device_id).collect();
    let list = if ids.is_empty() {
        Vec::new()
    } else {
        // **1台ずつ引かない**（22章 R-4）
        device::Entity::find()
            .filter(device::Column::Id.is_in(ids))
            .order_by_asc(device::Column::Hostname)
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    let rows = list
        .into_iter()
        .map(|d| StoredDeviceRow {
            since: 割当
                .iter()
                .find(|a| a.device_id == d.id)
                .map(|a| a.from_date.date_naive().to_string())
                .unwrap_or_default(),
            serial_number: d.serial_number.clone().unwrap_or_default(),
            id: d.id,
            hostname: d.hostname,
            device_type: d.device_type,
            status: d.status,
        })
        .collect();

    render(&DevicesPage {
        chrome: Chrome::warehouse(
            &current.user,
            current.csrf_token.clone(),
            w.id,
            &w.name,
            "devices",
        ),
        warehouse_id: id,
        warehouse_name: w.name,
        t_title: rust_i18n::t!("warehouses.devices", locale = l).to_string(),
        t_lead: rust_i18n::t!("warehouses.devices_lead", locale = l).to_string(),
        t_hostname: rust_i18n::t!("devices.hostname", locale = l).to_string(),
        t_device_type: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_status: rust_i18n::t!("devices.status", locale = l).to_string(),
        t_serial: rust_i18n::t!("warehouses.serial", locale = l).to_string(),
        t_since: rust_i18n::t!("warehouses.since", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_empty: rust_i18n::t!("warehouses.devices_empty", locale = l).to_string(),
        rows,
    })
}

// ---------------------------------------------------------------------------
// 倉庫内機器の詳細（所在の履歴を全期間ぶん出す）
// ---------------------------------------------------------------------------

struct HistoryRow {
    /// Warehouse / Project / Disposed。
    location_type: String,
    /// 倉庫名またはプロジェクト名。**消えた参照先は空にせず印を出す。**
    location_name: String,
    from_date: String,
    to_date: String,
    current: bool,
}

#[derive(askama::Template)]
#[template(path = "warehouse_device_detail.html")]
struct DeviceDetailPage {
    chrome: Chrome,
    warehouse_id: i32,
    warehouse_name: String,
    hostname: String,
    device_type: String,
    status: String,
    serial_number: String,
    asset_number: String,
    t_title: String,
    t_history_note: String,
    t_hostname: String,
    t_device_type: String,
    t_status: String,
    t_serial: String,
    t_asset: String,
    t_history: String,
    t_location: String,
    t_from: String,
    t_to: String,
    t_current: String,
    history: Vec<HistoryRow>,
}

pub async fn device_detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    let l = 入場(&state, &current).await?;
    let w = 倉庫(&state, id).await?;

    let d = device::Entity::find_by_id(device_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let 履歴 = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .order_by_asc(device_assignment::Column::FromDate)
        .order_by_asc(device_assignment::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **URLに機器IDを直接指定された場合に素通りさせない**（3章）。
    // 今この倉庫にある機器だけを開ける
    let ここにある = 履歴
        .iter()
        .any(|a| a.to_date.is_none() && a.location_type == WAREHOUSE && a.location_id == Some(id));
    if !ここにある {
        return Err(AppError::NotFound);
    }

    // **全期間ぶん出す。**倉庫にある間はA-6の対象外と決めた（16.1）
    let history = 履歴を組む(&state, &履歴, l).await?;

    render(&DeviceDetailPage {
        chrome: Chrome::warehouse(
            &current.user,
            current.csrf_token.clone(),
            w.id,
            &w.name,
            "devices",
        ),
        warehouse_id: id,
        warehouse_name: w.name,
        serial_number: d.serial_number.unwrap_or_default(),
        asset_number: d.asset_number.unwrap_or_default(),
        hostname: d.hostname,
        device_type: d.device_type,
        status: d.status,
        t_title: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_history_note: rust_i18n::t!("warehouses.history_note", locale = l).to_string(),
        t_hostname: rust_i18n::t!("devices.hostname", locale = l).to_string(),
        t_device_type: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_status: rust_i18n::t!("devices.status", locale = l).to_string(),
        t_serial: rust_i18n::t!("warehouses.serial", locale = l).to_string(),
        t_asset: rust_i18n::t!("warehouses.asset", locale = l).to_string(),
        t_history: rust_i18n::t!("warehouses.history", locale = l).to_string(),
        t_location: rust_i18n::t!("devices.location", locale = l).to_string(),
        t_from: rust_i18n::t!("warehouses.from", locale = l).to_string(),
        t_to: rust_i18n::t!("warehouses.to", locale = l).to_string(),
        t_current: rust_i18n::t!("warehouses.current", locale = l).to_string(),
        history,
    })
}

/// 所在の履歴に、倉庫名・プロジェクト名を付けて並べる。
///
/// **多態的参照のため外部キーが張れない**（4章）。参照先が消えている行は
/// **空欄にせず印を出す**——空欄だと「まだ入力されていない」と読める。
async fn 履歴を組む(
    state: &AppState,
    履歴: &[device_assignment::Model],
    l: &'static str,
) -> AppResult<Vec<HistoryRow>> {
    let 不明 = rust_i18n::t!("warehouses.unknown", locale = l).to_string();

    let mut 倉庫id = Vec::new();
    let mut プロジェクトid = Vec::new();
    for a in 履歴 {
        match (a.location_type.as_str(), a.location_id) {
            (WAREHOUSE, Some(id)) => 倉庫id.push(id),
            (PROJECT, Some(id)) => プロジェクトid.push(id),
            _ => {}
        }
    }

    // **参照先も1件ずつ引かない**（22章 R-4）
    let 倉庫名 = if 倉庫id.is_empty() {
        Vec::new()
    } else {
        warehouse::Entity::find()
            .filter(warehouse::Column::Id.is_in(倉庫id))
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };
    let プロジェクト名 = if プロジェクトid.is_empty() {
        Vec::new()
    } else {
        project::Entity::find()
            .filter(project::Column::Id.is_in(プロジェクトid))
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    Ok(履歴
        .iter()
        .map(|a| HistoryRow {
            location_name: match (a.location_type.as_str(), a.location_id) {
                (WAREHOUSE, Some(id)) => 倉庫名
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| 不明.clone()),
                (PROJECT, Some(id)) => プロジェクト名
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| 不明.clone()),
                // Disposed は参照先を持たない（12章）
                _ => String::new(),
            },
            location_type: a.location_type.clone(),
            from_date: a.from_date.date_naive().to_string(),
            to_date: a
                .to_date
                .map(|t| t.date_naive().to_string())
                .unwrap_or_default(),
            current: a.to_date.is_none(),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// 倉庫内パーツ在庫一覧（読み取り専用、12.4）
// ---------------------------------------------------------------------------

struct StoredPartRow {
    category: String,
    part_number: String,
    serial_number: String,
    status: String,
    since: String,
}

#[derive(askama::Template)]
#[template(path = "warehouse_parts.html")]
struct PartsPage {
    chrome: Chrome,
    warehouse_name: String,
    t_title: String,
    t_lead: String,
    t_category: String,
    t_part_number: String,
    t_serial: String,
    t_status: String,
    t_since: String,
    t_empty: String,
    rows: Vec<StoredPartRow>,
}

pub async fn parts(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let l = 入場(&state, &current).await?;
    let w = 倉庫(&state, id).await?;

    let 所在 = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(WAREHOUSE))
        .filter(part_instance_location::Column::LocationId.eq(id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let ids: Vec<i32> = 所在.iter().map(|p| p.part_instance_id).collect();
    let instances = if ids.is_empty() {
        Vec::new()
    } else {
        part_instance::Entity::find()
            .filter(part_instance::Column::Id.is_in(ids))
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    let カタログid: Vec<i32> = instances.iter().map(|i| i.part_catalog_id).collect();
    let catalog = if カタログid.is_empty() {
        Vec::new()
    } else {
        part_catalog::Entity::find()
            .filter(part_catalog::Column::Id.is_in(カタログid))
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    let mut rows: Vec<StoredPartRow> = instances
        .into_iter()
        .map(|i| {
            let c = catalog.iter().find(|c| c.id == i.part_catalog_id);
            StoredPartRow {
                category: c.map(|c| c.category.clone()).unwrap_or_default(),
                part_number: c.map(|c| c.part_number.clone()).unwrap_or_default(),
                serial_number: i.serial_number.unwrap_or_default(),
                since: 所在
                    .iter()
                    .find(|p| p.part_instance_id == i.id)
                    .map(|p| p.from_date.date_naive().to_string())
                    .unwrap_or_default(),
                status: i.status,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then(a.part_number.cmp(&b.part_number))
            .then(a.serial_number.cmp(&b.serial_number))
    });

    render(&PartsPage {
        chrome: Chrome::warehouse(
            &current.user,
            current.csrf_token.clone(),
            w.id,
            &w.name,
            "parts",
        ),
        warehouse_name: w.name,
        t_title: rust_i18n::t!("warehouses.parts", locale = l).to_string(),
        t_lead: rust_i18n::t!("warehouses.parts_lead", locale = l).to_string(),
        t_category: rust_i18n::t!("parts.category", locale = l).to_string(),
        t_part_number: rust_i18n::t!("parts.part_number", locale = l).to_string(),
        t_serial: rust_i18n::t!("warehouses.serial", locale = l).to_string(),
        t_status: rust_i18n::t!("devices.status", locale = l).to_string(),
        t_since: rust_i18n::t!("warehouses.since", locale = l).to_string(),
        t_empty: rust_i18n::t!("warehouses.parts_empty", locale = l).to_string(),
        rows,
    })
}

// ---------------------------------------------------------------------------
// 共通
// ---------------------------------------------------------------------------

/// 閲覧できるか（16.1のC領域）。
async fn 入場(state: &AppState, current: &CurrentUser) -> AppResult<&'static str> {
    authorization::require_warehouse_viewer(&state.db, &current.user)
        .await
        .map_err(|_| AppError::Forbidden)?;
    Ok(Locale::parse(&current.user.locale).as_str())
}

/// 倉庫を登録・編集できるか（16.1のC領域）。
async fn 編集権(state: &AppState, current: &CurrentUser) -> AppResult<()> {
    authorization::require_warehouse_editor(&state.db, &current.user)
        .await
        .map_err(|_| AppError::Forbidden)
}

async fn 倉庫(state: &AppState, id: i32) -> AppResult<warehouse::Model> {
    warehouse::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)
}

/// 倉庫にある機器の、現在有効な割当（12章）。
async fn 倉庫にある機器の割当(
    state: &AppState,
) -> AppResult<Vec<device_assignment::Model>> {
    device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(WAREHOUSE))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// 倉庫にあるパーツの、現在有効な所在（12.4）。
async fn 倉庫にあるパーツの所在(
    state: &AppState,
) -> AppResult<Vec<part_instance_location::Model>> {
    part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(WAREHOUSE))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}
