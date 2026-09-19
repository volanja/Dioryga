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
use entity::{
    cable_catalog, chassis_model, chassis_slot, configuration, configuration_part, device,
    part_catalog, part_port_slot, port_power_rating, software_catalog, vendor, vlan,
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde::Deserialize;

// 語彙は取込と同じ表を見る（8.6、#125、#148）
use dioryga_catalog_format::{DEVICE_CATEGORIES, MOUNT_FORMS, RACK_WIDTHS, SLOT_TYPES};

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, 種別の表示, 種別の選択肢, Choice, Chrome, Locale};
use crate::server::AppState;

/// どの部品カテゴリがどのスロットを消費するか（設計書6.1）。
///
/// **対応が無いカテゴリ（`PDU`）では警告を出さない。**機器に内蔵する部品では
/// ないため、消費するスロットが無い。
const カテゴリとスロット: &[(&str, &str)] = &[
    ("CPU", "CPU_SOCKET"),
    ("Memory", "DIMM"),
    ("Storage", "DRIVE_BAY"),
    ("NIC", "PCIE"),
    ("PSU", "PSU_BAY"),
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
    t_detail: String,
    t_title: String,
    t_lead: String,
    t_vendor: String,
    t_model_name: String,
    t_device_category: String,
    t_height_u: String,
    t_mount_form: String,
    t_rack_width: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_referenced: String,
    t_show_retired: String,
    rows: Vec<ChassisModelRow>,
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
    t_detail: String,
    t_title: String,
    t_lead: String,
    t_name: String,
    t_chassis_model: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_referenced: String,
    t_show_retired: String,
    rows: Vec<ConfigurationRow>,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
}

/// ベンダーの登録画面（#124）。
#[derive(askama::Template)]
#[template(path = "catalog_vendor_form.html")]
struct VendorFormPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_name: String,
    t_submit: String,
    v_name: String,
    error: Option<String>,
}

/// 筐体モデルの登録画面（#124）。
#[derive(askama::Template)]
#[template(path = "catalog_chassis_model_form.html")]
struct ChassisModelFormPage {
    chrome: Chrome,
    t_title: String,
    t_back: String,
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
    t_no_vendor: String,
    t_submit: String,
    vendors: Vec<Labeled>,
    device_categories: Vec<Choice>,
    mount_forms: Vec<&'static str>,
    rack_widths: Vec<&'static str>,
    // **入力した値を保つ**（#124）。誤りのたびに入れ直させない
    v_vendor_id: String,
    v_model_name: String,
    v_device_category: String,
    v_height_u: String,
    v_mount_form: String,
    v_rack_width: String,
    error: Option<String>,
}

/// 構成の登録画面（#124）。
#[derive(askama::Template)]
#[template(path = "catalog_configuration_form.html")]
struct ConfigurationFormPage {
    chrome: Chrome,
    t_title: String,
    t_back: String,
    t_name: String,
    t_chassis_model: String,
    t_no_model: String,
    t_current_type: String,
    t_assumed_voltage: String,
    t_assumed_va: String,
    t_assumed_va_hint: String,
    t_power_unset: String,
    t_submit: String,
    models: Vec<Labeled>,
    current_types: Vec<&'static str>,
    v_chassis_model_id: String,
    v_name: String,
    v_current_type: String,
    v_assumed_voltage: String,
    v_assumed_va: String,
    error: Option<String>,
}

pub(crate) struct Labeled {
    pub label: String,
    pub value: String,
}

// ---------------------------------------------------------------------------
// ダッシュボード（#118）
// ---------------------------------------------------------------------------

/// カタログ1種の登録数。
struct CountCard {
    /// テストと一覧へのリンクで使う識別子（`Chrome.sub` と同じ語）
    kind: &'static str,
    label: String,
    href: &'static str,
    /// **統合で吸収された行を除いた全件**（23.9.4）。一覧の件数と合わせる
    total: u64,
    /// 「うち廃番 N」
    t_retired: String,
}

#[derive(askama::Template)]
#[template(path = "catalog_index.html")]
struct IndexPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_unit: String,
    cards: Vec<CountCard>,
}

/// 全件と、そのうちの廃番の数。
///
/// **有効と廃番を並べて出す**（設計書16.1のD領域）。一覧は既定で廃番を隠す
/// （18.5）ため、一覧を開いても廃番の溜まり具合は分からない。
macro_rules! 件数 {
    ($db:expr, $m:ident, $基準:expr) => {{
        let 基準 = $基準;
        let 全件 = 基準.clone().count($db).await;
        let 廃番 = 基準
            .filter($m::Column::RetiredAt.is_not_null())
            .count($db)
            .await;
        match (全件, 廃番) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => return Err(AppError::Internal(anyhow::anyhow!(e))),
        }
    }};
}

pub async fn index(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    let db = &state.db;

    let vendors = 件数!(
        db,
        vendor,
        vendor::Entity::find().filter(vendor::Column::MergedIntoVendorId.is_null())
    );
    let chassis_models = 件数!(db, chassis_model, chassis_model::Entity::find());
    let parts = 件数!(
        db,
        part_catalog,
        part_catalog::Entity::find()
            .filter(part_catalog::Column::MergedIntoPartCatalogId.is_null())
    );
    let configurations = 件数!(db, configuration, configuration::Entity::find());
    let cables = 件数!(db, cable_catalog, cable_catalog::Entity::find());
    let software = 件数!(db, software_catalog, software_catalog::Entity::find());
    let vlans = 件数!(db, vlan, vlan::Entity::find());

    // 並びはメニューと同じにする。見比べたときに位置で対応が取れる
    let 並び = [
        ("vendors", "catalog.vendors", "/catalog/vendors", vendors),
        (
            "chassis_models",
            "catalog.chassis_models",
            "/catalog/chassis-models",
            chassis_models,
        ),
        ("parts", "parts.title", "/catalog/parts", parts),
        (
            "configurations",
            "catalog.configurations",
            "/catalog/configurations",
            configurations,
        ),
        ("cables", "cables.title", "/catalog/cables", cables),
        ("software", "software.title", "/catalog/software", software),
        ("vlans", "vlans.title", "/catalog/vlans", vlans),
    ];
    let cards = 並び
        .into_iter()
        .map(|(kind, key, href, (total, retired))| CountCard {
            kind,
            label: rust_i18n::t!(key, locale = l).to_string(),
            href,
            total,
            t_retired: rust_i18n::t!("catalog.retired_count", locale = l, count = retired)
                .to_string(),
        })
        .collect();

    render(&IndexPage {
        // ダッシュボードでは下位の項目に印を付けない
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), ""),
        t_title: rust_i18n::t!("catalog.nav", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.dashboard_lead", locale = l).to_string(),
        t_unit: rust_i18n::t!("catalog.count_unit", locale = l).to_string(),
        cards,
    })
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
        // **統合で吸収された行は一覧に出さない**（23.9.4）。重複として畳まれた
        // 側であり、出すと同じベンダーが2行に見える
        .filter(vendor::Column::MergedIntoVendorId.is_null())
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
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "vendors"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_title: rust_i18n::t!("catalog.vendors", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.vendors_lead", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("catalog.new_vendor", locale = l).to_string(),
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

/// 登録画面を描く（#124）。**Viewerは入れない**（18.1）。
async fn ベンダー登録を描く(
    state: &AppState,
    current: &CurrentUser,
    v_name: String,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    編集権(state, current).await?;

    render(&VendorFormPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "vendors"),
        t_title: rust_i18n::t!("catalog.new_vendor", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.vendors_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("catalog.back_vendors", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        v_name,
        error,
    })
}

pub async fn new_vendor(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> AppResult<Response> {
    ベンダー登録を描く(&state, &current, String::new(), None).await
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
    // **改名は一覧の行で、登録は登録画面で行う**（#124）。誤りは来た画面へ返す
    let 改名 = !form.id.trim().is_empty();
    let 誤りを返す = async |state: &AppState, current: &CurrentUser, e: String| {
        if 改名 {
            ベンダーを描く(state, current, false, Some(e)).await
        } else {
            ベンダー登録を描く(state, current, form.name.clone(), Some(e)).await
        }
    };
    if name.is_empty() {
        let e = rust_i18n::t!("catalog.error_name", locale = l).to_string();
        return 誤りを返す(&state, &current, e).await;
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
        return 誤りを返す(&state, &current, e).await;
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
            device_category: 種別の表示(&m.device_category, l),
            height_u: m.height_u,
            mount_form: m.mount_form,
        });
    }

    render(&ChassisModelsPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "chassis_models"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_title: rust_i18n::t!("catalog.chassis_models", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.chassis_models_lead", locale = l).to_string(),
        t_vendor: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
        t_model_name: rust_i18n::t!("catalog.model_name", locale = l).to_string(),
        t_device_category: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_height_u: rust_i18n::t!("catalog.height_u", locale = l).to_string(),
        t_mount_form: rust_i18n::t!("catalog.mount_form", locale = l).to_string(),
        t_rack_width: rust_i18n::t!("catalog.rack_width", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("catalog.new_chassis_model", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_referenced: rust_i18n::t!("catalog.referenced", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        rows,
        show_retired,
        can_edit,
        error,
    })
}

#[derive(Debug, Default, Deserialize)]
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

/// 登録画面を描く（#124）。**Viewerは入れない**（18.1）。
async fn 筐体型の登録を描く(
    state: &AppState,
    current: &CurrentUser,
    form: &ChassisModelForm,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    編集権(state, current).await?;

    render(&ChassisModelFormPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "chassis_models"),
        t_title: rust_i18n::t!("catalog.new_chassis_model", locale = l).to_string(),
        t_back: rust_i18n::t!("catalog.back_models", locale = l).to_string(),
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
        t_no_vendor: rust_i18n::t!("catalog.no_vendor", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        vendors: 現役のベンダー(&state.db).await?,
        device_categories: 種別の選択肢(l),
        mount_forms: MOUNT_FORMS.to_vec(),
        rack_widths: RACK_WIDTHS.to_vec(),
        v_vendor_id: 空でなければ(form.vendor_id),
        v_model_name: form.model_name.clone(),
        v_device_category: form.device_category.clone(),
        v_height_u: form.height_u.clone(),
        v_mount_form: form.mount_form.clone(),
        v_rack_width: form.rack_width.clone(),
        error,
    })
}

/// 未選択（0）は空文字にする。**選択肢の値と比べるため文字列で持つ。**
fn 空でなければ(id: i32) -> String {
    if id == 0 {
        String::new()
    } else {
        id.to_string()
    }
}

pub async fn new_chassis_model(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> AppResult<Response> {
    let form = ChassisModelForm {
        // 高さは1Uを既定にする。最も多い値を初期値にして入力を減らす
        height_u: "1".to_owned(),
        ..Default::default()
    };
    筐体型の登録を描く(&state, &current, &form, None).await
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
        return 筐体型の登録を描く(
            &state,
            &current,
            &form,
            誤り("catalog.error_model_name"),
        )
        .await;
    }

    // **語彙外は既定へ寄せず拒否する**（Q-21）
    if !DEVICE_CATEGORIES.contains(&form.device_category.as_str()) {
        return 筐体型の登録を描く(
            &state,
            &current,
            &form,
            誤り("catalog.error_category"),
        )
        .await;
    }
    if !MOUNT_FORMS.contains(&form.mount_form.as_str()) {
        return 筐体型の登録を描く(
            &state,
            &current,
            &form,
            誤り("catalog.error_mount_form"),
        )
        .await;
    }

    let height_u = match form.height_u.trim().parse::<i32>() {
        Ok(n) if n >= 0 => n,
        _ => {
            return 筐体型の登録を描く(
                &state,
                &current,
                &form,
                誤り("catalog.error_height"),
            )
            .await
        }
    };

    // **`rack_width` は `mount_form=RackU` のときのみ意味を持つ**（6.2）。
    // それ以外に値が来たら黙って捨てず拒否する（Q-21）
    let rack_width = match (form.mount_form.as_str(), form.rack_width.trim()) {
        ("RackU", "") => Some("Full".to_owned()),
        ("RackU", v) if RACK_WIDTHS.contains(&v) => Some(v.to_owned()),
        ("RackU", _) => {
            return 筐体型の登録を描く(
                &state,
                &current,
                &form,
                誤り("catalog.error_rack_width"),
            )
            .await;
        }
        (_, "") => None,
        (_, _) => {
            return 筐体型の登録を描く(
                &state,
                &current,
                &form,
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
        return 筐体型の登録を描く(
            &state,
            &current,
            &form,
            誤り("catalog.error_model_duplicate"),
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    let model = tx
        .insert(chassis_model::ActiveModel {
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

    // **詳細へ進む。**登録したらスロットを足す作業が続く（#124）
    Ok(Redirect::to(&format!("/catalog/chassis-models/{}", model.id)).into_response())
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
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "configurations"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_title: rust_i18n::t!("catalog.configurations", locale = l).to_string(),
        t_lead: rust_i18n::t!("catalog.configurations_lead", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_chassis_model: rust_i18n::t!("catalog.chassis_model", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("catalog.new_configuration", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_referenced: rust_i18n::t!("catalog.referenced", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        rows,
        show_retired,
        can_edit,
        error,
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct ConfigurationForm {
    pub chassis_model_id: i32,
    #[serde(default)]
    pub name: String,
    // 想定消費電力（12.8）。**`serde(flatten)` は使わない**——`Form` が使う
    // urlencodedのデシリアライザが対応しておらず、実行時に落ちる
    #[serde(default)]
    pub current_type: String,
    #[serde(default)]
    pub assumed_voltage: String,
    #[serde(default)]
    pub assumed_va: String,
}

/// 想定消費電力だけを受ける（設計書12.8）。詳細画面での編集に使う。
#[derive(Debug, Default, Deserialize)]
pub struct PowerForm {
    #[serde(default)]
    pub current_type: String,
    #[serde(default)]
    pub assumed_voltage: String,
    #[serde(default)]
    pub assumed_va: String,
}

/// 読み取った想定消費電力。
#[derive(Debug, Default, Clone, Copy)]
struct 想定電力<'a> {
    current_type: Option<&'a str>,
    voltage: Option<i32>,
    va: Option<i32>,
}

/// 入力を読む（設計書12.8）。
///
/// **3つは揃うか、揃って空か。**この3列は「何アンペア引く見込みか」と
/// 「選んだPSUの対応範囲に収まっているか」に答えるために置いたもので、
/// **欠けた組み合わせではどちらにも答えられない。**分からない値を0で埋めさせない
/// ために全体をnullableにしてあるので、「未入力」は3つとも空で表す。
///
/// **電流は受け取らない**（不変条件2）。`A = VA ÷ V` で求める。
fn 電力を読む<'a>(
    current_type: &'a str,
    assumed_voltage: &str,
    assumed_va: &str,
) -> Result<想定電力<'a>, &'static str> {
    let kind = current_type.trim();
    let voltage = assumed_voltage.trim();
    let va = assumed_va.trim();

    if kind.is_empty() && voltage.is_empty() && va.is_empty() {
        return Ok(想定電力::default());
    }
    if kind.is_empty() || voltage.is_empty() || va.is_empty() {
        return Err("catalog.error_power_partial");
    }

    // **語彙外は既定へ寄せず拒否する**（Q-21）
    if !crate::server::part::CURRENT_TYPES.contains(&kind) {
        return Err("catalog.error_current_type");
    }

    // **0Vは受け付けない。**`A = VA ÷ V` が定義できない。DCの負値は正当なので
    // 「正の数」ではなく「0でないこと」を条件にする（12.7）
    let voltage: i32 = voltage.parse().map_err(|_| "catalog.error_power_number")?;
    if voltage == 0 {
        return Err("catalog.error_voltage_zero");
    }
    let va: i32 = va.parse().map_err(|_| "catalog.error_power_number")?;
    if va <= 0 {
        return Err("catalog.error_va_positive");
    }

    Ok(想定電力 {
        current_type: Some(kind),
        voltage: Some(voltage),
        va: Some(va),
    })
}

/// 引くと見込む電流（設計書12.7・12.8）。
///
/// **保存せず、表示のたびに計算する**（不変条件2）。DCの負電圧でも電流の向きは
/// 問題にならないため、**絶対値で割る。**
fn 想定電流(voltage: i32, va: i32) -> String {
    format!("{:.1}A", va as f64 / (voltage.abs() as f64))
}

/// 登録画面を描く（#124）。**Viewerは入れない**（18.1）。
async fn 構成の登録を描く(
    state: &AppState,
    current: &CurrentUser,
    form: &ConfigurationForm,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    編集権(state, current).await?;

    render(&ConfigurationFormPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "configurations"),
        t_title: rust_i18n::t!("catalog.new_configuration", locale = l).to_string(),
        t_back: rust_i18n::t!("catalog.back_configurations", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_chassis_model: rust_i18n::t!("catalog.chassis_model", locale = l).to_string(),
        t_no_model: rust_i18n::t!("catalog.no_model", locale = l).to_string(),
        t_current_type: rust_i18n::t!("catalog.current_type", locale = l).to_string(),
        t_assumed_voltage: rust_i18n::t!("catalog.assumed_voltage", locale = l).to_string(),
        t_assumed_va: rust_i18n::t!("catalog.assumed_va", locale = l).to_string(),
        t_assumed_va_hint: rust_i18n::t!("catalog.assumed_va_hint", locale = l).to_string(),
        t_power_unset: rust_i18n::t!("catalog.power_unset", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        models: 現役の筐体型(&state.db).await?,
        current_types: crate::server::part::CURRENT_TYPES.to_vec(),
        v_chassis_model_id: 空でなければ(form.chassis_model_id),
        v_name: form.name.clone(),
        v_current_type: form.current_type.clone(),
        v_assumed_voltage: form.assumed_voltage.clone(),
        v_assumed_va: form.assumed_va.clone(),
        error,
    })
}

pub async fn new_configuration(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> AppResult<Response> {
    構成の登録を描く(&state, &current, &ConfigurationForm::default(), None).await
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
        return 構成の登録を描く(&state, &current, &form, Some(e)).await;
    }

    let 電力 = match 電力を読む(&form.current_type, &form.assumed_voltage, &form.assumed_va) {
        Ok(v) => v,
        Err(key) => {
            let e = rust_i18n::t!(key, locale = l).to_string();
            return 構成の登録を描く(&state, &current, &form, Some(e)).await;
        }
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    let configuration = tx
        .insert(configuration::ActiveModel {
            chassis_model_id: Set(form.chassis_model_id),
            name: Set(name),
            retired_at: Set(None),
            current_type: Set(電力.current_type.map(str::to_owned)),
            assumed_voltage: Set(電力.voltage),
            assumed_va: Set(電力.va),
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

    // **詳細へ進む。**登録したら部品を足す作業が続く（#124）
    Ok(Redirect::to(&format!("/catalog/configurations/{}", configuration.id)).into_response())
}

// ---------------------------------------------------------------------------
// 筐体モデルの詳細（スロット、設計書6.1）
// ---------------------------------------------------------------------------

struct SlotRow {
    id: i32,
    slot_type: String,
    slot_label: String,
}

#[derive(askama::Template)]
#[template(path = "catalog_chassis_model_detail.html")]
struct ChassisModelDetailPage {
    chrome: Chrome,
    chassis_model_id: i32,
    t_back: String,
    t_basic: String,
    t_slots: String,
    t_slots_hint: String,
    t_slot_type: String,
    t_slot_label: String,
    t_add_slot: String,
    t_remove: String,
    t_empty: String,
    t_actions: String,
    title: String,
    basic: Vec<Labeled>,
    slots: Vec<SlotRow>,
    slot_types: Vec<&'static str>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn chassis_model_detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    筐体型の詳細を描く(&state, &current, id, None).await
}

async fn 筐体型の詳細を描く(
    state: &AppState,
    current: &CurrentUser,
    id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = 編集権(state, current).await.is_ok();

    let m = chassis_model::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let mut basic = vec![
        Labeled {
            label: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
            value: ベンダー名(&state.db, m.vendor_id).await?,
        },
        Labeled {
            label: rust_i18n::t!("devices.device_type", locale = l).to_string(),
            value: 種別の表示(&m.device_category, l),
        },
        Labeled {
            label: rust_i18n::t!("catalog.height_u", locale = l).to_string(),
            value: m.height_u.to_string(),
        },
        Labeled {
            label: rust_i18n::t!("catalog.mount_form", locale = l).to_string(),
            value: m.mount_form.clone(),
        },
    ];
    if let Some(w) = &m.rack_width {
        basic.push(Labeled {
            label: rust_i18n::t!("catalog.rack_width", locale = l).to_string(),
            value: w.clone(),
        });
    }

    let slots = chassis_slot::Entity::find()
        .filter(chassis_slot::Column::ChassisModelId.eq(id))
        .order_by_asc(chassis_slot::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|s| SlotRow {
            id: s.id,
            slot_type: s.slot_type,
            slot_label: s.slot_label,
        })
        .collect();

    render(&ChassisModelDetailPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "chassis_models"),
        chassis_model_id: id,
        t_back: rust_i18n::t!("catalog.back_models", locale = l).to_string(),
        t_basic: rust_i18n::t!("devices.basic", locale = l).to_string(),
        t_slots: rust_i18n::t!("catalog.slots", locale = l).to_string(),
        t_slots_hint: rust_i18n::t!("catalog.slots_hint", locale = l).to_string(),
        t_slot_type: rust_i18n::t!("catalog.slot_type", locale = l).to_string(),
        t_slot_label: rust_i18n::t!("catalog.slot_label", locale = l).to_string(),
        t_add_slot: rust_i18n::t!("catalog.add_slot", locale = l).to_string(),
        t_remove: rust_i18n::t!("catalog.remove", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.no_slot", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        title: format!(
            "{} {}",
            ベンダー名(&state.db, m.vendor_id).await?,
            m.model_name
        ),
        basic,
        slots,
        slot_types: SLOT_TYPES.to_vec(),
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct SlotForm {
    #[serde(default)]
    pub slot_type: String,
    #[serde(default)]
    pub slot_label: String,
}

pub async fn add_slot(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<SlotForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    chassis_model::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    if !SLOT_TYPES.contains(&form.slot_type.as_str()) {
        return 筐体型の詳細を描く(&state, &current, id, 誤り("catalog.error_slot_type")).await;
    }
    let label = form.slot_label.trim();
    if label.is_empty() {
        return 筐体型の詳細を描く(&state, &current, id, 誤り("catalog.error_slot_label")).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(chassis_slot::ActiveModel {
        chassis_model_id: Set(id),
        slot_type: Set(form.slot_type.clone()),
        slot_label: Set(label.to_owned()),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/catalog/chassis-models/{id}")).into_response())
}

#[derive(Debug, Deserialize)]
pub struct RemoveChildForm {
    pub child_id: i32,
}

pub async fn remove_slot(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<RemoveChildForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

    let row = chassis_slot::Entity::find_by_id(form.child_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|r| r.chassis_model_id == id)
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

    Ok(Redirect::to(&format!("/catalog/chassis-models/{id}")).into_response())
}

// ---------------------------------------------------------------------------
// 構成の詳細（部品、設計書6.1・6.2）
// ---------------------------------------------------------------------------

struct ConfigurationPartRow {
    id: i32,
    part: String,
    category: String,
    quantity: i32,
}

#[derive(askama::Template)]
#[template(path = "catalog_configuration_detail.html")]
struct ConfigurationDetailPage {
    chrome: Chrome,
    configuration_id: i32,
    t_back: String,
    t_basic: String,
    t_parts: String,
    t_parts_hint: String,
    t_part: String,
    t_category: String,
    t_quantity: String,
    t_quantity_hint: String,
    t_add_part: String,
    t_remove: String,
    t_empty: String,
    t_actions: String,
    t_no_part: String,
    t_power: String,
    t_power_hint: String,
    t_current_type: String,
    t_assumed_voltage: String,
    t_assumed_va: String,
    t_assumed_va_hint: String,
    t_save: String,
    t_unset: String,
    title: String,
    basic: Vec<Labeled>,
    parts: Vec<ConfigurationPartRow>,
    candidates: Vec<Labeled>,
    can_edit: bool,
    error: Option<String>,
    /// スロット本数の超過。**エラーではなく警告**（6.1、不変条件6）。
    notice: Option<String>,
    /// 想定消費電力（12.8）。フォームの初期値に使う。
    current_type: String,
    assumed_voltage: String,
    assumed_va: String,
    current_types: Vec<&'static str>,
    /// `VA ÷ V` の計算結果。**保存しない**（不変条件2）。
    assumed_current: Option<String>,
    /// 選んだPSUの対応範囲から外れている、という警告（12.8、不変条件6）。
    power_warning: Option<String>,
}

pub async fn configuration_detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    構成の詳細を描く(&state, &current, id, None, None).await
}

async fn 構成の詳細を描く(
    state: &AppState,
    current: &CurrentUser,
    id: i32,
    error: Option<String>,
    notice: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = 編集権(state, current).await.is_ok();

    let c = configuration::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let rows = configuration_part::Entity::find()
        .filter(configuration_part::Column::ConfigurationId.eq(id))
        .order_by_asc(configuration_part::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut parts = Vec::new();
    for r in rows {
        let p = part_catalog::Entity::find_by_id(r.part_catalog_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        parts.push(ConfigurationPartRow {
            part: match &p {
                Some(p) => format!(
                    "{} {}",
                    ベンダー名(&state.db, p.vendor_id).await?,
                    p.part_number
                ),
                None => String::new(),
            },
            category: p.map(|p| p.category).unwrap_or_default(),
            id: r.id,
            quantity: r.quantity,
        });
    }

    render(&ConfigurationDetailPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "configurations"),
        configuration_id: id,
        t_back: rust_i18n::t!("catalog.back_configurations", locale = l).to_string(),
        t_basic: rust_i18n::t!("devices.basic", locale = l).to_string(),
        t_parts: rust_i18n::t!("catalog.parts", locale = l).to_string(),
        t_parts_hint: rust_i18n::t!("catalog.parts_hint", locale = l).to_string(),
        t_part: rust_i18n::t!("catalog.part", locale = l).to_string(),
        t_category: rust_i18n::t!("parts.category", locale = l).to_string(),
        t_quantity: rust_i18n::t!("catalog.quantity", locale = l).to_string(),
        t_quantity_hint: rust_i18n::t!("catalog.quantity_hint", locale = l).to_string(),
        t_add_part: rust_i18n::t!("catalog.add_part", locale = l).to_string(),
        t_remove: rust_i18n::t!("catalog.remove", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.no_part", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_no_part: rust_i18n::t!("catalog.no_part_catalog", locale = l).to_string(),
        t_power: rust_i18n::t!("catalog.power", locale = l).to_string(),
        t_power_hint: rust_i18n::t!("catalog.power_hint", locale = l).to_string(),
        t_current_type: rust_i18n::t!("parts.current_type", locale = l).to_string(),
        t_assumed_voltage: rust_i18n::t!("catalog.assumed_voltage", locale = l).to_string(),
        t_assumed_va: rust_i18n::t!("catalog.assumed_va", locale = l).to_string(),
        t_assumed_va_hint: rust_i18n::t!("catalog.assumed_va_hint", locale = l).to_string(),
        t_save: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_unset: rust_i18n::t!("catalog.power_unset", locale = l).to_string(),
        title: c.name.clone(),
        basic: vec![Labeled {
            label: rust_i18n::t!("catalog.chassis_model", locale = l).to_string(),
            value: 筐体型名(&state.db, c.chassis_model_id).await?,
        }],
        parts,
        candidates: 現役の部品(&state.db).await?,
        can_edit,
        error,
        notice,
        current_type: c.current_type.clone().unwrap_or_default(),
        assumed_voltage: c.assumed_voltage.map(|v| v.to_string()).unwrap_or_default(),
        assumed_va: c.assumed_va.map(|v| v.to_string()).unwrap_or_default(),
        current_types: crate::server::part::CURRENT_TYPES.to_vec(),
        // **計算して見せるだけで保存しない**（不変条件2）。利用者が知りたいのは
        // 何アンペア引く見込みかであり、VAとVはその材料である（12.7）
        assumed_current: match (c.assumed_voltage, c.assumed_va) {
            (Some(v), Some(va)) => Some(想定電流(v, va)),
            _ => None,
        },
        power_warning: 定格の範囲外(&state.db, &c).await?.map(|(kind, v)| {
            rust_i18n::t!(
                "catalog.warn_power_range",
                locale = l,
                current_type = kind,
                voltage = v
            )
            .to_string()
        }),
    })
}

/// 選んだPSUが対応する方式・電圧範囲から外れていないか（設計書12.8）。
///
/// `CONFIGURATION` → `CONFIGURATION_PART` → `PART_CATALOG`（category=PSU）→
/// `PART_PORT_SLOT`（port_kind=Power）→ `PORT_POWER_RATING` を辿る。
///
/// **ハードな禁止ではなく警告にする**（不変条件6）。実機が仕様の想定外である
/// ことはありうるし、誤って拒否すると事実を記録できなくなる。
///
/// **`PORT_POWER_RATING` が1件も無いなら判定しない。**#53の取込が入るまで
/// データが無く、「登録されていない」を「違反」と扱うと**警告が常に出る状態に
/// なり、それは警告が無いのと同じ**である（6.1のスロット超過と同じ考え方）。
async fn 定格の範囲外<C: ConnectionTrait>(
    db: &C,
    c: &configuration::Model,
) -> AppResult<Option<(String, i32)>> {
    let (Some(kind), Some(voltage)) = (c.current_type.as_deref(), c.assumed_voltage) else {
        return Ok(None);
    };

    let parts = configuration_part::Entity::find()
        .filter(configuration_part::Column::ConfigurationId.eq(c.id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut 定格あり = false;
    for r in parts {
        let psu = part_catalog::Entity::find_by_id(r.part_catalog_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .filter(|p| p.category == "PSU");
        let Some(psu) = psu else { continue };

        let ports = part_port_slot::Entity::find()
            .filter(part_port_slot::Column::PartCatalogId.eq(psu.id))
            .filter(part_port_slot::Column::PortKind.eq("Power"))
            .all(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

        for port in ports {
            let ratings = port_power_rating::Entity::find()
                .filter(port_power_rating::Column::PartPortSlotId.eq(port.id))
                .all(db)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

            for rating in ratings {
                定格あり = true;
                // **絶対値で比べない**（12.7）。`-72 ≤ -48 ≤ -40` が成り立つ
                if rating.current_type == kind
                    && rating.voltage_min <= voltage
                    && voltage <= rating.voltage_max
                {
                    return Ok(None);
                }
            }
        }
    }

    Ok(定格あり.then(|| (kind.to_owned(), voltage)))
}

/// 想定消費電力を保存する（設計書12.8）。
///
/// **18.2の「参照済みはスペックを編集できない」の対象にしない。**18.2が守ろうと
/// しているのは「構成がいつの間にか変わっていること」であり、想定消費電力は
/// 構成そのものではなく**その構成についての見積もり**である。実測や再計算で
/// 更新されうる値を凍結すると、正しい値を書けなくなる。
pub async fn update_power(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<PowerForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;

    let before = configuration::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let 電力 = match 電力を読む(&form.current_type, &form.assumed_voltage, &form.assumed_va) {
        Ok(v) => v,
        Err(key) => {
            let e = rust_i18n::t!(key, locale = l).to_string();
            return 構成の詳細を描く(&state, &current, id, Some(e), None).await;
        }
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        configuration::ActiveModel {
            id: Set(id),
            current_type: Set(電力.current_type.map(str::to_owned)),
            assumed_voltage: Set(電力.voltage),
            assumed_va: Set(電力.va),
            updated_at: Set(Utc::now()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **範囲外でも保存はする**（不変条件6）。警告は再描画で見せる
    Ok(Redirect::to(&format!("/catalog/configurations/{id}")).into_response())
}

#[derive(Debug, Deserialize)]
pub struct ConfigurationPartForm {
    pub part_catalog_id: i32,
    #[serde(default)]
    pub quantity: String,
}

pub async fn add_part(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<ConfigurationPartForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    let c = configuration::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let quantity = match form.quantity.trim().parse::<i32>() {
        Ok(n) if n > 0 => n,
        _ => {
            return 構成の詳細を描く(
                &state,
                &current,
                id,
                誤り("catalog.error_quantity"),
                None,
            )
            .await
        }
    };

    let p = part_catalog::Entity::find_by_id(form.part_catalog_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    // **同じ部品を複数行に分けず `quantity` で表す**（6.2）
    let 既存 = configuration_part::Entity::find()
        .filter(configuration_part::Column::ConfigurationId.eq(id))
        .filter(configuration_part::Column::PartCatalogId.eq(p.id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 既存.is_some() {
        return 構成の詳細を描く(
            &state,
            &current,
            id,
            誤り("catalog.error_part_duplicate"),
            None,
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(configuration_part::ActiveModel {
        configuration_id: Set(id),
        part_catalog_id: Set(p.id),
        quantity: Set(quantity),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **超過は登録を拒否せず警告する**（6.1、不変条件6）。実機の構成が仕様の
    // 想定外であること自体はありうるうえ、誤って拒否すると事実を記録できなくなる
    let notice = スロット超過(&state.db, c.chassis_model_id, id, &p.category)
        .await?
        .map(|(使用, 本数)| {
            rust_i18n::t!(
                "catalog.warn_slot_over",
                locale = l,
                category = p.category,
                used = 使用,
                slots = 本数
            )
            .to_string()
        });

    if notice.is_some() {
        return 構成の詳細を描く(&state, &current, id, None, notice).await;
    }

    Ok(Redirect::to(&format!("/catalog/configurations/{id}")).into_response())
}

pub async fn remove_part(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<RemoveChildForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

    let row = configuration_part::Entity::find_by_id(form.child_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|r| r.configuration_id == id)
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

    Ok(Redirect::to(&format!("/catalog/configurations/{id}")).into_response())
}

/// スロット本数を超えているか（設計書6.1）。
///
/// **超えていなければ `None`。**返すのは `(使用本数, スロット本数)`。
///
/// 次の2つの場合は判定しない。どちらも「本数が分からない」のであって
/// 「本数が0」ではなく、**警告が常に出る状態は警告が無いのと同じ**になる。
///
/// - 対応するスロット種別が無いカテゴリ（`PDU`）
/// - その種別の `CHASSIS_SLOT` が1件も登録されていない筐体モデル
async fn スロット超過<C: ConnectionTrait>(
    db: &C,
    chassis_model_id: i32,
    configuration_id: i32,
    category: &str,
) -> AppResult<Option<(i32, i32)>> {
    let Some((_, slot_type)) = カテゴリとスロット.iter().find(|(c, _)| *c == category)
    else {
        return Ok(None);
    };

    let 本数 = chassis_slot::Entity::find()
        .filter(chassis_slot::Column::ChassisModelId.eq(chassis_model_id))
        .filter(chassis_slot::Column::SlotType.eq(*slot_type))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .len() as i32;
    if 本数 == 0 {
        return Ok(None);
    }

    // 同じスロットを使う部品の数量を合算する
    let 行 = configuration_part::Entity::find()
        .filter(configuration_part::Column::ConfigurationId.eq(configuration_id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut 使用 = 0;
    for r in 行 {
        let 同じ種別 = part_catalog::Entity::find_by_id(r.part_catalog_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .is_some_and(|p| p.category == category);
        if 同じ種別 {
            使用 += r.quantity;
        }
    }

    Ok((使用 > 本数).then_some((使用, 本数)))
}

async fn 現役の部品<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    let parts = part_catalog::Entity::find()
        .filter(part_catalog::Column::RetiredAt.is_null())
        .filter(part_catalog::Column::MergedIntoPartCatalogId.is_null())
        .order_by_asc(part_catalog::Column::PartNumber)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for p in parts {
        out.push(Labeled {
            label: format!(
                "{} {} ({})",
                ベンダー名(db, p.vendor_id).await?,
                p.part_number,
                p.category
            ),
            value: p.id.to_string(),
        });
    }
    Ok(out)
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

/// どのカタログを廃番にするか。
///
/// **URLのパスパラメータでは受けない。**`/catalog/{kind}/retire` にすると
/// `/catalog/chassis-models/{id}`（詳細）と衝突し、静的セグメントが優先される
/// ぶん詳細側が勝って405になる。ルートごとに静的パスを置き、種別はここで渡す。
#[derive(Debug, Clone, Copy)]
enum Kind {
    Vendor,
    ChassisModel,
    Configuration,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Self::Vendor => "/catalog/vendors",
            Self::ChassisModel => "/catalog/chassis-models",
            Self::Configuration => "/catalog/configurations",
        }
    }
}

pub async fn retire_vendor(
    state: State<AppState>,
    current: Extension<CurrentUser>,
    form: Form<RetireForm>,
) -> AppResult<Response> {
    retire(state, current, Kind::Vendor, form).await
}

pub async fn retire_chassis_model(
    state: State<AppState>,
    current: Extension<CurrentUser>,
    form: Form<RetireForm>,
) -> AppResult<Response> {
    retire(state, current, Kind::ChassisModel, form).await
}

pub async fn retire_configuration(
    state: State<AppState>,
    current: Extension<CurrentUser>,
    form: Form<RetireForm>,
) -> AppResult<Response> {
    retire(state, current, Kind::Configuration, form).await
}

/// 廃番にする／取り消す。
///
/// **参照済みでも設定できる**（18.5）。18.2が禁じているのはスペックを定義する
/// フィールドの編集であり、選択可否はスペックではない。
async fn retire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    kind: Kind,
    Form(form): Form<RetireForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

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
/// 表記の正規化（18.4）。**取込と同じ実装を使う**（#77）。
///
/// 経路によって正規化が食い違うと、同じ値が別行になる。
pub(crate) use dioryga_catalog_format::正規化;

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
pub(crate) async fn 現役のベンダー<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    Ok(vendor::Entity::find()
        .filter(vendor::Column::RetiredAt.is_null())
        .filter(vendor::Column::MergedIntoVendorId.is_null())
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
pub(crate) fn 入場(_state: &AppState, current: &CurrentUser) -> AppResult<&'static str> {
    authorization::deny_system_admin(&current.user).map_err(|_| AppError::Forbidden)?;
    Ok(Locale::parse(&current.user.locale).as_str())
}

pub(crate) async fn 編集権(state: &AppState, current: &CurrentUser) -> AppResult<()> {
    authorization::require_catalog_editor(&state.db, &current.user)
        .await
        .map_err(|_| AppError::Forbidden)
}
