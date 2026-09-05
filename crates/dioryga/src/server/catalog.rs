//! 共有カタログ領域（設計書16.1のD領域、18章）。
//!
//! Vendor / ChassisModel / Configuration を扱う。部品・ケーブル・ソフトウェア・
//! VLANは別途（#64）。
//!
//! # 誰が編集できるか（18.1）
//!
//! カタログは特定のプロジェクトに属さないため、通常のプロジェクトロールだけでは
//! 権限を決められない。**いずれか1つ以上のプロジェクトでOperator以上**であれば
//! 編集できる。**System Adminは触れない**——3章の「プロジェクトデータに一切
//! アクセスできない」を維持するため。既存の [`authorization::require_catalog_editor`]
//! がこれを判定する。
//!
//! # 参照されたカタログはスペックを編集できない（18.2）
//!
//! 「構成がいつの間にか変わっていることを防ぐ」ため。DB制約では表現できないので、
//! **更新前にEXISTS判定を行う。**
//!
//! | カタログ | 参照の有無をチェックする先 |
//! |---|---|
//! | `CHASSIS_MODEL` | `CONFIGURATION`、`CHASSIS_SLOT` |
//! | `CONFIGURATION` | `DEVICE` |
//!
//! **`VENDOR` は対象外**（18.3）。ベンダー名の表記修正は構成そのものを変えない。
//!
//! # 廃番は参照済みでも設定できる（18.5）
//!
//! 18.2が禁じているのは*スペックを定義するフィールド*の編集であり、**選択可否は
//! スペックではない。**既存の参照は壊さず、一覧の既定で隠すだけである。
//!
//! # 名前の付け方（18.4）
//!
//! `model_name` はベンダーが公表する製品名を正とし、同一製品名の中でスロット
//! 構成が異なる派生が実在する場合に限り、**ベンダーの型名をASCII空白1つで連結**
//! する。括弧や区切り記号は使わない——18.3でVENDORを作って防いだ表記ゆれを
//! `model_name` で再生産しないため。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{chassis_model, chassis_slot, configuration, device, vendor};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 語彙（`vocabularies.md`、設計書6.2、12.3）。
const MOUNT_FORMS: &[&str] = &["RackU", "RackSide", "Surface"];
const RACK_WIDTHS: &[&str] = &["Full", "Half"];
const DEVICE_CATEGORIES: &[&str] = &[
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
// 画面
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// 既定は現役のみ。廃番を含めるときだけ `1`（設計書18.5）。
    #[serde(default)]
    pub retired: Option<String>,
}

struct VendorRow {
    id: i32,
    name: String,
    retired: bool,
}

#[derive(askama::Template)]
#[template(path = "catalog_vendors.html")]
struct VendorsPage {
    chrome: Chrome,
    t_apply: String,
    t_title: String,
    t_lead: String,
    t_name: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_save: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_show_retired: String,
    t_edit_hint: String,
    rows: Vec<VendorRow>,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
}

struct ChassisModelRow {
    id: i32,
    vendor: String,
    model_name: String,
    device_category: String,
    height_u: i32,
    mount_form: String,
    rack_width: String,
    retired: bool,
    /// 18.2により、スペックを編集できない。
    referenced: bool,
}

#[derive(askama::Template)]
#[template(path = "catalog_chassis_models.html")]
struct ChassisModelsPage {
    chrome: Chrome,
    t_apply: String,
    t_title: String,
    t_lead: String,
    t_naming_hint: String,
    t_vendor: String,
    t_model_name: String,
    t_device_category: String,
    t_height_u: String,
    t_height_hint: String,
    t_mount_form: String,
    t_mount_form_hint: String,
    t_rack_width: String,
    t_rack_width_hint: String,
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
    rows: Vec<ChassisModelRow>,
    vendors: Vec<Labeled>,
    device_categories: Vec<&'static str>,
    mount_forms: Vec<&'static str>,
    rack_widths: Vec<&'static str>,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
}

struct ConfigurationRow {
    id: i32,
    name: String,
    chassis_model: String,
    retired: bool,
    referenced: bool,
}

#[derive(askama::Template)]
#[template(path = "catalog_configurations.html")]
struct ConfigurationsPage {
    chrome: Chrome,
    t_apply: String,
    t_title: String,
    t_lead: String,
    t_name: String,
    t_chassis_model: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_referenced: String,
    t_show_retired: String,
    t_no_model: String,
    rows: Vec<ConfigurationRow>,
    models: Vec<Labeled>,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
}

struct Labeled {
    label: String,
    value: String,
}

// ---------------------------------------------------------------------------
// Vendor（設計書18.3）
// ---------------------------------------------------------------------------

pub async fn vendors(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    ベンダーを描く(&state, &current, 廃番を含む(&query), None).await
}

async fn ベンダーを描く(
    state: &AppState,
    current: &CurrentUser,
    show_retired: bool,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = authorization::require_catalog_editor(&state.db, &current.user)
        .await
        .is_ok();

    let rows = vendor::Entity::find()
        .order_by_asc(vendor::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        // **既定では廃番を隠す**（18.5）。詳細は切り替えで見える
        .filter(|v| show_retired || v.retired_at.is_none())
        .map(|v| VendorRow {
            id: v.id,
            name: v.name,
            retired: v.retired_at.is_some(),
        })
        .collect();

    render(&VendorsPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "catalog"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_title: rust_i18n::t!("catalog.vendors", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.vendors_lead", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("catalog.new_vendor", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_save: rust_i18n::t!("members.save", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        t_edit_hint: rust_i18n::t!("catalog.vendor_edit_hint", locale = l).to_string(),
        rows,
        show_retired,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct VendorForm {
    /// 空なら新規登録、値があれば改名。
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

pub async fn save_vendor(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<VendorForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;

    // **ベンダー名も正規化する**（18.4と同じ規則）。`ＨＰＥ` と `HPE` が別行に
    // なると、表記ゆれを防ぐために置いたマスタが表記ゆれの発生源になる
    let name = 正規化(&form.name);
    let name = name.as_str();
    if name.is_empty() {
        let e = rust_i18n::t!("catalog.error_name", locale = l).to_string();
        return ベンダーを描く(&state, &current, false, Some(e)).await;
    }

    // **`name` はUNIQUE**（18.3）。表記ゆれを防ぐために置いたマスタで同名を
    // 2件持てると、存在意義がなくなる
    let 既存 = vendor::Entity::find()
        .filter(vendor::Column::Name.eq(name))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let id = form.id.trim().parse::<i32>().ok();
    if 既存.as_ref().is_some_and(|v| Some(v.id) != id) {
        let e = rust_i18n::t!("catalog.error_vendor_duplicate", locale = l).to_string();
        return ベンダーを描く(&state, &current, false, Some(e)).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    match id {
        // **VENDORは18.2の対象外**（18.3）。参照済みでも改名できる——誤字訂正は
        // 構成そのものを変えるわけではない
        Some(id) => {
            let before = vendor::Entity::find_by_id(id)
                .one(tx.reader())
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .ok_or(AppError::NotFound)?;
            tx.update(
                &before,
                vendor::ActiveModel {
                    id: Set(id),
                    name: Set(name.to_owned()),
                    updated_at: Set(now),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
        None => {
            tx.insert(vendor::ActiveModel {
                name: Set(name.to_owned()),
                retired_at: Set(None),
                created_by: Set(current.user.id),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(Redirect::to("/catalog/vendors").into_response())
}

// ---------------------------------------------------------------------------
// ChassisModel（設計書6.2、18.4）
// ---------------------------------------------------------------------------

pub async fn chassis_models(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    筐体型を描く(&state, &current, 廃番を含む(&query), None).await
}

async fn 筐体型を描く(
    state: &AppState,
    current: &CurrentUser,
    show_retired: bool,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = authorization::require_catalog_editor(&state.db, &current.user)
        .await
        .is_ok();

    let models = chassis_model::Entity::find()
        .order_by_asc(chassis_model::Column::ModelName)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for m in models {
        if !show_retired && m.retired_at.is_some() {
            continue;
        }
        rows.push(ChassisModelRow {
            vendor: ベンダー名(&state.db, m.vendor_id).await?,
            referenced: 筐体型が参照されている(&state.db, m.id).await?,
            retired: m.retired_at.is_some(),
            rack_width: m.rack_width.clone().unwrap_or_default(),
            id: m.id,
            model_name: m.model_name,
            device_category: m.device_category,
            height_u: m.height_u,
            mount_form: m.mount_form,
        });
    }

    render(&ChassisModelsPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "catalog"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_title: rust_i18n::t!("catalog.chassis_models", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.chassis_models_lead", locale = l).to_string(),
        t_naming_hint: rust_i18n::t!("catalog.naming_hint", locale = l).to_string(),
        t_vendor: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
        t_model_name: rust_i18n::t!("catalog.model_name", locale = l).to_string(),
        t_device_category: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_height_u: rust_i18n::t!("catalog.height_u", locale = l).to_string(),
        t_height_hint: rust_i18n::t!("catalog.height_hint", locale = l).to_string(),
        t_mount_form: rust_i18n::t!("catalog.mount_form", locale = l).to_string(),
        t_mount_form_hint: rust_i18n::t!("catalog.mount_form_hint", locale = l).to_string(),
        t_rack_width: rust_i18n::t!("catalog.rack_width", locale = l).to_string(),
        t_rack_width_hint: rust_i18n::t!("catalog.rack_width_hint", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("catalog.new_chassis_model", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_referenced: rust_i18n::t!("catalog.referenced", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        t_no_vendor: rust_i18n::t!("catalog.no_vendor", locale = l).to_string(),
        rows,
        vendors: 現役のベンダー(&state.db).await?,
        device_categories: DEVICE_CATEGORIES.to_vec(),
        mount_forms: MOUNT_FORMS.to_vec(),
        rack_widths: RACK_WIDTHS.to_vec(),
        show_retired,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct ChassisModelForm {
    pub vendor_id: i32,
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub device_category: String,
    #[serde(default)]
    pub height_u: String,
    #[serde(default)]
    pub mount_form: String,
    #[serde(default)]
    pub rack_width: String,
}

pub async fn create_chassis_model(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<ChassisModelForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **18.4の正規化。**全角英数を半角に、連続する空白を1つに（25.2の段階3）
    let model_name = 正規化(&form.model_name);
    if model_name.is_empty() {
        return 筐体型を描く(&state, &current, false, 誤り("catalog.error_model_name")).await;
    }

    // **語彙外は既定へ寄せず拒否する**（Q-21）
    if !DEVICE_CATEGORIES.contains(&form.device_category.as_str()) {
        return 筐体型を描く(&state, &current, false, 誤り("catalog.error_category")).await;
    }
    if !MOUNT_FORMS.contains(&form.mount_form.as_str()) {
        return 筐体型を描く(&state, &current, false, 誤り("catalog.error_mount_form")).await;
    }

    let height_u = match form.height_u.trim().parse::<i32>() {
        Ok(n) if n >= 0 => n,
        _ => return 筐体型を描く(&state, &current, false, 誤り("catalog.error_height")).await,
    };

    // **`rack_width` は `mount_form=RackU` のときのみ意味を持つ**（6.2）。
    // それ以外に値が来たら黙って捨てず拒否する（Q-21）
    let rack_width = match (form.mount_form.as_str(), form.rack_width.trim()) {
        ("RackU", "") => Some("Full".to_owned()),
        ("RackU", v) if RACK_WIDTHS.contains(&v) => Some(v.to_owned()),
        ("RackU", _) => {
            return 筐体型を描く(&state, &current, false, 誤り("catalog.error_rack_width")).await;
        }
        (_, "") => None,
        (_, _) => {
            return 筐体型を描く(
                &state,
                &current,
                false,
                誤り("catalog.error_rack_width_unused"),
            )
            .await;
        }
    };

    // 自然キーは `(vendor_id, model_name)`（6.2）。**別ベンダーなら同じ型番を持てる**
    let 重複 = chassis_model::Entity::find()
        .filter(chassis_model::Column::VendorId.eq(form.vendor_id))
        .filter(chassis_model::Column::ModelName.eq(&model_name))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複.is_some() {
        return 筐体型を描く(
            &state,
            &current,
            false,
            誤り("catalog.error_model_duplicate"),
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(chassis_model::ActiveModel {
        vendor_id: Set(form.vendor_id),
        model_name: Set(model_name),
        device_category: Set(form.device_category.clone()),
        height_u: Set(height_u),
        mount_form: Set(form.mount_form.clone()),
        rack_width: Set(rack_width),
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

    Ok(Redirect::to("/catalog/chassis-models").into_response())
}

// ---------------------------------------------------------------------------
// Configuration（設計書6.2）
// ---------------------------------------------------------------------------

pub async fn configurations(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    構成を描く(&state, &current, 廃番を含む(&query), None).await
}

async fn 構成を描く(
    state: &AppState,
    current: &CurrentUser,
    show_retired: bool,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = authorization::require_catalog_editor(&state.db, &current.user)
        .await
        .is_ok();

    let list = configuration::Entity::find()
        .order_by_asc(configuration::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for c in list {
        if !show_retired && c.retired_at.is_some() {
            continue;
        }
        rows.push(ConfigurationRow {
            chassis_model: 筐体型名(&state.db, c.chassis_model_id).await?,
            // **`DEVICE` から参照されていたらスペックを編集できない**（18.2）
            referenced: device::Entity::find()
                .filter(device::Column::ConfigurationId.eq(c.id))
                .one(&state.db)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .is_some(),
            retired: c.retired_at.is_some(),
            id: c.id,
            name: c.name,
        });
    }

    render(&ConfigurationsPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "catalog"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_title: rust_i18n::t!("catalog.configurations", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.configurations_lead", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_chassis_model: rust_i18n::t!("catalog.chassis_model", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("catalog.new_configuration", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_referenced: rust_i18n::t!("catalog.referenced", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        t_no_model: rust_i18n::t!("catalog.no_model", locale = l).to_string(),
        rows,
        models: 現役の筐体型(&state.db).await?,
        show_retired,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct ConfigurationForm {
    pub chassis_model_id: i32,
    #[serde(default)]
    pub name: String,
}

pub async fn create_configuration(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<ConfigurationForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;

    let name = 正規化(&form.name);
    if name.is_empty() {
        let e = rust_i18n::t!("catalog.error_name", locale = l).to_string();
        return 構成を描く(&state, &current, false, Some(e)).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(configuration::ActiveModel {
        chassis_model_id: Set(form.chassis_model_id),
        name: Set(name),
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

    Ok(Redirect::to("/catalog/configurations").into_response())
}

// ---------------------------------------------------------------------------
// 廃番（設計書18.5）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RetireForm {
    pub id: i32,
    /// `1` なら廃番を取り消す。
    #[serde(default)]
    pub undo: String,
}

/// どのカタログを廃番にするか。URLで分ける。
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Vendor,
    ChassisModel,
    Configuration,
}

impl Kind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "vendors" => Some(Self::Vendor),
            "chassis-models" => Some(Self::ChassisModel),
            "configurations" => Some(Self::Configuration),
            _ => None,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Vendor => "/catalog/vendors",
            Self::ChassisModel => "/catalog/chassis-models",
            Self::Configuration => "/catalog/configurations",
        }
    }
}

/// 廃番にする／取り消す。
///
/// **参照済みでも設定できる**（18.5）。18.2が禁じているのはスペックを定義する
/// フィールドの編集であり、選択可否はスペックではない。
pub async fn retire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(kind): Path<String>,
    Form(form): Form<RetireForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

    let kind = Kind::parse(&kind).ok_or(AppError::NotFound)?;
    let at = (form.undo != "1").then(Utc::now);

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    match kind {
        Kind::Vendor => {
            let before = vendor::Entity::find_by_id(form.id)
                .one(tx.reader())
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .ok_or(AppError::NotFound)?;
            tx.update(
                &before,
                vendor::ActiveModel {
                    id: Set(form.id),
                    retired_at: Set(at),
                    updated_at: Set(now),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
        }
        Kind::ChassisModel => {
            let before = chassis_model::Entity::find_by_id(form.id)
                .one(tx.reader())
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .ok_or(AppError::NotFound)?;
            tx.update(
                &before,
                chassis_model::ActiveModel {
                    id: Set(form.id),
                    retired_at: Set(at),
                    updated_at: Set(now),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
        }
        Kind::Configuration => {
            let before = configuration::Entity::find_by_id(form.id)
                .one(tx.reader())
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                .ok_or(AppError::NotFound)?;
            tx.update(
                &before,
                configuration::ActiveModel {
                    id: Set(form.id),
                    retired_at: Set(at),
                    updated_at: Set(now),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
        }
    }
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(Redirect::to(kind.path()).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 廃番を含む(query: &ListQuery) -> bool {
    query.retired.as_deref() == Some("1")
}

/// 18.4の正規化（25.2の段階3）。
///
/// **全角英数を半角に、連続する空白を1つにする。**18.3でVENDORを作って防いだ
/// 表記ゆれを `model_name` で再生産しないため。
///
/// **ベンダー名にも同じ規則を当てる。**18.4は `model_name` について書かれて
/// いるが、`ＨＰＥ` と `HPE` が別行になると、表記ゆれを防ぐために置いたマスタが
/// 表記ゆれの発生源になる。18.3の目的からして同じ扱いが要る。
///
/// 変換するのは全角ASCII（`U+FF01`〜`U+FF5E`）と全角空白だけで、**仮名・漢字は
/// 触らない。**「富士通」はそのまま残る。
fn 正規化(value: &str) -> String {
    let 半角: String = value
        .chars()
        .map(|c| match c {
            // 全角英数・記号は半角へ。全角空白も通常の空白へ
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect();

    半角.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 18.2の判定。**`CONFIGURATION` と `CHASSIS_SLOT` の両方を見る。**
async fn 筐体型が参照されている<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<bool> {
    let 構成あり = configuration::Entity::find()
        .filter(configuration::Column::ChassisModelId.eq(id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some();
    if 構成あり {
        return Ok(true);
    }

    Ok(chassis_slot::Entity::find()
        .filter(chassis_slot::Column::ChassisModelId.eq(id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some())
}

/// 登録時の候補。**廃番は出さない**（18.5）——押せてしまう選択肢を出さない。
async fn 現役のベンダー<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    Ok(vendor::Entity::find()
        .filter(vendor::Column::RetiredAt.is_null())
        .order_by_asc(vendor::Column::Name)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|v| Labeled {
            label: v.name,
            value: v.id.to_string(),
        })
        .collect())
}

async fn 現役の筐体型<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    let models = chassis_model::Entity::find()
        .filter(chassis_model::Column::RetiredAt.is_null())
        .order_by_asc(chassis_model::Column::ModelName)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for m in models {
        out.push(Labeled {
            label: format!("{} {}", ベンダー名(db, m.vendor_id).await?, m.model_name),
            value: m.id.to_string(),
        });
    }
    Ok(out)
}

async fn ベンダー名<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<String> {
    Ok(vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|v| v.name)
        .unwrap_or_default())
}

async fn 筐体型名<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<String> {
    let Some(m) = chassis_model::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok(String::new());
    };
    Ok(format!(
        "{} {}",
        ベンダー名(db, m.vendor_id).await?,
        m.model_name
    ))
}

/// カタログは全メンバーが閲覧できる。**System Adminだけが入れない**（3章）。
fn 入場(_state: &AppState, current: &CurrentUser) -> AppResult<&'static str> {
    authorization::deny_system_admin(&current.user).map_err(|_| AppError::Forbidden)?;
    Ok(Locale::parse(&current.user.locale).as_str())
}

async fn 編集権(state: &AppState, current: &CurrentUser) -> AppResult<()> {
    authorization::require_catalog_editor(&state.db, &current.user)
        .await
        .map_err(|_| AppError::Forbidden)
}
