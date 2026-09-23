//! コスト・契約管理（設計書16.1のB領域、10.2、10.3）。
//!
//! # 値は保存せず、都度計算する（不変条件2）
//!
//! 年間コストも減価償却費も保存しない。**保存すると、元になった契約や資産が
//! 直されたときに古い数字が残る。**計算そのものは [`crate::cost`] にある。
//!
//! # 計算できない行は合算から外して画面に列挙する（10.3）
//!
//! 定率法の資産（Q-12）、取得日の無い購入、期間や耐用年数が不正な行。**黙って除外しない**——金額が小さいのが実態なのかデータ不備なのかを、
//! 利用者が判別できる必要がある。23.5で部分的な粒度の取込に「分母を表示する」と
//! したのと同じ考え方である。
//!
//! **選択肢からは外さない。**定率法の資産も取得日の分からない購入も実在し、
//! 記録できなくすると事実を書けなくなる（不変条件6）。
//!
//! # 購入の記録は専用の一覧画面を持たない（10.2）
//!
//! 機器の登録と機器詳細から入れる。この画面群が扱うのは
//! 保守契約・固定資産・定期費用と、年間コストのダッシュボードである。
//!
//! # 多態的な参照（10.2）
//!
//! `FIXED_ASSET` / `MAINTENANCE_CONTRACT_ITEM` / `RECURRING_COST` は
//! `item_type` / `item_id` を持つ。**DB外部キー制約を張れない**ため、
//! 参照先の存在はアプリケーション層で確かめる（4章、C-7）。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::{Datelike, NaiveDate, Utc};
use entity::{
    device, device_assignment, fixed_asset, maintenance_contract, maintenance_contract_item,
    mount_container, project, purchase, recurring_cost, vendor,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::cost::{self, 除外の理由};
use crate::currency;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{正規化, Labeled};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

const DEVICE: &str = "Device";
const PROJECT: &str = "Project";
const MOUNT_CONTAINER: &str = "MountContainer";

/// `RECURRING_COST.item_type`（10.2）。**ラックレンタル等はプロジェクトか什器に付く。**
const RECURRING_ITEM_TYPES: &[&str] = &[MOUNT_CONTAINER, PROJECT];

// ---------------------------------------------------------------------------
// 年間コストダッシュボード（設計書10.3）
// ---------------------------------------------------------------------------

/// 合算できた費目。
struct 内訳 {
    label: String,
    amount: String,
}

/// 合算できなかった行（10.3）。**黙って除外しない。**
struct 除外行 {
    kind: String,
    name: String,
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct YearQuery {
    #[serde(default)]
    pub year: Option<i32>,
}

#[derive(askama::Template)]
#[template(path = "costs_dashboard.html")]
struct DashboardPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_year: String,
    t_apply: String,
    t_total: String,
    t_breakdown: String,
    t_excluded: String,
    t_excluded_lead: String,
    t_kind: String,
    t_name: String,
    t_reason: String,
    t_no_excluded: String,
    t_contracts: String,
    t_assets: String,
    t_recurring: String,
    currency: String,
    year: i32,
    total: String,
    rows: Vec<内訳>,
    excluded: Vec<除外行>,
}

pub async fn dashboard(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<YearQuery>,
) -> AppResult<Response> {
    let (project, _, l) = 入場(&state, &current, project_id).await?;
    let year = query.year.unwrap_or_else(|| Utc::now().year());
    let 通貨 = project.currency.clone();

    let mut 合計 = 0i64;
    let mut excluded = Vec::new();

    // 減価償却費（10.3）
    let mut 償却 = 0i64;
    for a in このプロジェクトの資産(&state, project_id).await? {
        match cost::減価償却費(
            a.acquisition_cost,
            &a.depreciation_method,
            a.useful_life_years,
            a.acquisition_date,
            year,
        ) {
            Ok(v) => 償却 += v,
            Err(理由) => excluded.push(除外行 {
                kind: rust_i18n::t!("costs.fixed_assets", locale = l).to_string(),
                name: 品目の表示(&state, &a.item_type, a.item_id).await?,
                reason: rust_i18n::t!(理由.key(), locale = l).to_string(),
            }),
        }
    }
    合計 += 償却;

    // 保守契約費用（10.3）
    let mut 保守 = 0i64;
    for c in このプロジェクトの契約(&state, project_id).await? {
        match cost::期間費用のその年の額(c.amount, c.start_date, c.end_date, year) {
            Ok(v) => 保守 += v,
            Err(理由) => excluded.push(除外行 {
                kind: rust_i18n::t!("costs.contracts", locale = l).to_string(),
                name: c.contract_number.clone(),
                reason: rust_i18n::t!(理由.key(), locale = l).to_string(),
            }),
        }
    }
    合計 += 保守;

    // 定期費用（10.3）
    let mut 定期 = 0i64;
    for r in このプロジェクトの定期費用(&state, project_id).await? {
        // **`end_date` が null なら継続中。**その年の末日まで続くものとして扱う
        let end = r
            .end_date
            .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 12, 31).expect("年末"));
        match cost::期間費用のその年の額(r.amount, r.start_date, end, year) {
            Ok(v) => 定期 += v,
            Err(理由) => excluded.push(除外行 {
                kind: rust_i18n::t!("costs.recurring", locale = l).to_string(),
                name: r.cost_type.clone(),
                reason: rust_i18n::t!(理由.key(), locale = l).to_string(),
            }),
        }
    }
    合計 += 定期;

    // **非資産計上品の即時費用**（10.3）。取得日の無い購入は年に寄せられない
    let (即時, 取得日なし) = 即時費用(&state, project_id, year).await?;
    合計 += 即時;
    for name in 取得日なし {
        excluded.push(除外行 {
            kind: rust_i18n::t!("costs.purchases", locale = l).to_string(),
            name,
            reason: rust_i18n::t!(除外の理由::取得日が無い.key(), locale = l).to_string(),
        });
    }

    let rows = vec![
        内訳 {
            label: rust_i18n::t!("costs.depreciation", locale = l).to_string(),
            amount: currency::表示(償却, &通貨),
        },
        内訳 {
            label: rust_i18n::t!("costs.immediate", locale = l).to_string(),
            amount: currency::表示(即時, &通貨),
        },
        内訳 {
            label: rust_i18n::t!("costs.contracts", locale = l).to_string(),
            amount: currency::表示(保守, &通貨),
        },
        内訳 {
            label: rust_i18n::t!("costs.recurring", locale = l).to_string(),
            amount: currency::表示(定期, &通貨),
        },
    ];

    render(&DashboardPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "costs",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("costs.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("costs.lead", locale = l).to_string(),
        t_year: rust_i18n::t!("costs.year", locale = l).to_string(),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_total: rust_i18n::t!("costs.total", locale = l).to_string(),
        t_breakdown: rust_i18n::t!("costs.breakdown", locale = l).to_string(),
        t_excluded: rust_i18n::t!("costs.excluded", locale = l).to_string(),
        t_excluded_lead: rust_i18n::t!("costs.excluded_lead", locale = l).to_string(),
        t_kind: rust_i18n::t!("costs.kind", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_reason: rust_i18n::t!("costs.reason", locale = l).to_string(),
        t_no_excluded: rust_i18n::t!("costs.no_excluded", locale = l).to_string(),
        t_contracts: rust_i18n::t!("costs.contracts", locale = l).to_string(),
        t_assets: rust_i18n::t!("costs.fixed_assets", locale = l).to_string(),
        t_recurring: rust_i18n::t!("costs.recurring", locale = l).to_string(),
        currency: 通貨.clone(),
        year,
        total: currency::表示(合計, &通貨),
        rows,
        excluded,
    })
}

/// 非資産計上品の即時費用（10.3）。
///
/// **`FIXED_ASSET` を持たない品目**（ケーブル等）は、取得した年
/// （`PURCHASE.acquired_on` の年）に `amount` を全額計上する。
///
/// **取得日の無い購入は合算しない。**どの年に計上するか決められないため、
/// 除外した機器のホスト名を返して画面に列挙する。
async fn 即時費用(
    state: &AppState,
    project_id: i32,
    year: i32,
) -> AppResult<(i64, Vec<String>)> {
    let device_ids = このプロジェクトの機器id(state, project_id).await?;
    let 資産あり = fixed_asset::Entity::find()
        .filter(fixed_asset::Column::ItemType.eq(DEVICE))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|a| a.item_id)
        .collect::<Vec<_>>();

    let mut 合計 = 0i64;
    let mut 取得日なし = Vec::new();

    for p in purchase::Entity::find()
        .filter(purchase::Column::ItemType.eq(DEVICE))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        if !device_ids.contains(&p.item_id) || 資産あり.contains(&p.item_id) {
            continue;
        }
        match p.acquired_on {
            Some(d) if d.year() == year => 合計 += p.amount,
            Some(_) => {}
            None => {
                let name = device::Entity::find_by_id(p.item_id)
                    .one(&state.db)
                    .await
                    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                    .map(|d| d.hostname)
                    .unwrap_or_default();
                取得日なし.push(name);
            }
        }
    }

    Ok((合計, 取得日なし))
}

// ---------------------------------------------------------------------------
// 保守契約（設計書10.2）
// ---------------------------------------------------------------------------

struct ContractRow {
    id: i32,
    contract_number: String,
    vendor: String,
    period: String,
    amount: String,
    quote_contact: String,
    failure_contact: String,
    /// カバーしている品目（10.2の中間テーブル）。
    items: String,
}

#[derive(askama::Template)]
#[template(path = "costs_contracts.html")]
struct ContractsPage {
    chrome: Chrome,
    project_id: i32,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_number: String,
    t_vendor: String,
    t_period: String,
    t_start: String,
    t_end: String,
    t_amount: String,
    t_amount_hint: String,
    t_quote_contact: String,
    t_failure_contact: String,
    t_contact_hint: String,
    t_items: String,
    t_item_hint: String,
    t_add_item: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_target: String,
    currency: String,
    rows: Vec<ContractRow>,
    vendors: Vec<Labeled>,
    devices: Vec<Labeled>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn contracts(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    契約を描く(&state, &current, project_id, None).await
}

async fn 契約を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit, l) = 入場(state, current, project_id).await?;
    let 通貨 = project.currency.clone();

    let mut rows = Vec::new();
    for c in このプロジェクトの契約(state, project_id).await? {
        rows.push(ContractRow {
            vendor: ベンダー名(&state.db, Some(c.vendor_id)).await?,
            period: format!("{} 〜 {}", c.start_date, c.end_date),
            amount: currency::表示(c.amount, &通貨),
            items: 契約の品目(state, c.id).await?,
            id: c.id,
            contract_number: c.contract_number,
            quote_contact: c.quote_contact,
            failure_contact: c.failure_contact,
        });
    }

    render(&ContractsPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "costs",
        )
        .await,
        project_id,
        t_title: rust_i18n::t!("costs.contracts", locale = l).to_string(),
        t_lead: rust_i18n::t!("costs.contracts_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("costs.title", locale = l).to_string(),
        t_number: rust_i18n::t!("costs.contract_number", locale = l).to_string(),
        t_vendor: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
        t_period: rust_i18n::t!("costs.period", locale = l).to_string(),
        t_start: rust_i18n::t!("costs.start_date", locale = l).to_string(),
        t_end: rust_i18n::t!("costs.end_date", locale = l).to_string(),
        t_amount: rust_i18n::t!("costs.amount", locale = l).to_string(),
        t_amount_hint: rust_i18n::t!("costs.amount_hint", locale = l).to_string(),
        t_quote_contact: rust_i18n::t!("costs.quote_contact", locale = l).to_string(),
        t_failure_contact: rust_i18n::t!("costs.failure_contact", locale = l).to_string(),
        t_contact_hint: rust_i18n::t!("costs.contact_hint", locale = l).to_string(),
        t_items: rust_i18n::t!("costs.items", locale = l).to_string(),
        t_item_hint: rust_i18n::t!("costs.item_hint", locale = l).to_string(),
        t_add_item: rust_i18n::t!("costs.add_item", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("costs.new_contract", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_target: rust_i18n::t!("network.target", locale = l).to_string(),
        currency: 通貨,
        rows,
        vendors: 現役のベンダー(&state.db).await?,
        devices: このプロジェクトの機器候補(state, project_id).await?,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct ContractForm {
    #[serde(default)]
    pub contract_number: String,
    pub vendor_id: i32,
    #[serde(default)]
    pub start_date: String,
    #[serde(default)]
    pub end_date: String,
    #[serde(default)]
    pub amount: String,
    #[serde(default)]
    pub quote_contact: String,
    #[serde(default)]
    pub failure_contact: String,
}

pub async fn create_contract(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<ContractForm>,
) -> AppResult<Response> {
    let (project, l) = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());
    let 戻る = |key: &str| 契約を描く(&state, &current, project_id, 誤り(key));

    let number = 正規化(&form.contract_number);
    if number.is_empty() {
        return 戻る("costs.error_contract_number").await;
    }
    let (Some(start), Some(end)) = (日付(&form.start_date), 日付(&form.end_date)) else {
        return 戻る("costs.error_date").await;
    };
    // **期間が0以下は按分できない**（24.2.2）。入力の時点で弾く
    if end < start {
        return 戻る("costs.error_period").await;
    }
    let Some(amount) = currency::最小単位へ(&form.amount, &project.currency) else {
        return 戻る("costs.error_amount").await;
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(maintenance_contract::ActiveModel {
        contract_number: Set(number),
        vendor_id: Set(form.vendor_id),
        start_date: Set(start),
        end_date: Set(end),
        amount: Set(amount),
        quote_contact: Set(正規化(&form.quote_contact)),
        failure_contact: Set(正規化(&form.failure_contact)),
        order_number: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!(
        "/projects/{project_id}/costs/maintenance-contracts"
    ))
    .into_response())
}

#[derive(Debug, Deserialize)]
pub struct ContractItemForm {
    pub maintenance_contract_id: i32,
    pub device_id: i32,
}

/// 契約がカバーする品目を足す（10.2）。**1契約で複数品目をカバーできる。**
pub async fn add_contract_item(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<ContractItemForm>,
) -> AppResult<Response> {
    let (_, l) = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **多態的参照はDB制約を張れない。**参照先の存在をここで確かめる（4章）
    if !このプロジェクトの機器id(&state, project_id)
        .await?
        .contains(&form.device_id)
    {
        return Err(AppError::NotFound);
    }
    let 契約 = このプロジェクトの契約(&state, project_id)
        .await?
        .into_iter()
        .find(|c| c.id == form.maintenance_contract_id);
    // 未カバーの契約にも足せる必要があるため、契約の存在だけを確かめる
    if 契約.is_none()
        && maintenance_contract::Entity::find_by_id(form.maintenance_contract_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .is_none()
    {
        return Err(AppError::NotFound);
    }

    let 重複 = maintenance_contract_item::Entity::find()
        .filter(
            maintenance_contract_item::Column::MaintenanceContractId
                .eq(form.maintenance_contract_id),
        )
        .filter(maintenance_contract_item::Column::ItemType.eq(DEVICE))
        .filter(maintenance_contract_item::Column::ItemId.eq(form.device_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複.is_some() {
        return 契約を描く(
            &state,
            &current,
            project_id,
            誤り("costs.error_item_duplicate"),
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(maintenance_contract_item::ActiveModel {
        maintenance_contract_id: Set(form.maintenance_contract_id),
        item_type: Set(DEVICE.to_owned()),
        item_id: Set(form.device_id),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!(
        "/projects/{project_id}/costs/maintenance-contracts"
    ))
    .into_response())
}

// ---------------------------------------------------------------------------
// 固定資産（設計書10.2）
// ---------------------------------------------------------------------------

struct AssetRow {
    item: String,
    acquisition_cost: String,
    method: String,
    useful_life_years: i32,
    acquisition_date: String,
    /// その年の減価償却費。計算できなければ理由を出す（10.3）。
    this_year: String,
    excluded: bool,
}

#[derive(askama::Template)]
#[template(path = "costs_assets.html")]
struct AssetsPage {
    chrome: Chrome,
    project_id: i32,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_item: String,
    t_cost: String,
    t_method: String,
    t_method_hint: String,
    t_life: String,
    t_date: String,
    t_this_year: String,
    t_amount_hint: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_target: String,
    currency: String,
    year: i32,
    rows: Vec<AssetRow>,
    devices: Vec<Labeled>,
    methods: Vec<&'static str>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn assets(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    資産を描く(&state, &current, project_id, None).await
}

async fn 資産を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit, l) = 入場(state, current, project_id).await?;
    let 通貨 = project.currency.clone();
    let year = Utc::now().year();

    let mut rows = Vec::new();
    for a in このプロジェクトの資産(state, project_id).await? {
        let (this_year, excluded) = match cost::減価償却費(
            a.acquisition_cost,
            &a.depreciation_method,
            a.useful_life_years,
            a.acquisition_date,
            year,
        ) {
            Ok(v) => (currency::表示(v, &通貨), false),
            Err(理由) => (rust_i18n::t!(理由.key(), locale = l).to_string(), true),
        };

        rows.push(AssetRow {
            item: 品目の表示(state, &a.item_type, a.item_id).await?,
            acquisition_cost: currency::表示(a.acquisition_cost, &通貨),
            method: a.depreciation_method.clone(),
            useful_life_years: a.useful_life_years,
            acquisition_date: a.acquisition_date.to_string(),
            this_year,
            excluded,
        });
    }

    render(&AssetsPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "costs",
        )
        .await,
        project_id,
        t_title: rust_i18n::t!("costs.fixed_assets", locale = l).to_string(),
        t_lead: rust_i18n::t!("costs.assets_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("costs.title", locale = l).to_string(),
        t_item: rust_i18n::t!("costs.item", locale = l).to_string(),
        t_cost: rust_i18n::t!("costs.acquisition_cost", locale = l).to_string(),
        t_method: rust_i18n::t!("costs.method", locale = l).to_string(),
        t_method_hint: rust_i18n::t!("costs.method_hint", locale = l).to_string(),
        t_life: rust_i18n::t!("costs.useful_life", locale = l).to_string(),
        t_date: rust_i18n::t!("costs.acquisition_date", locale = l).to_string(),
        t_this_year: rust_i18n::t!("costs.this_year", locale = l).to_string(),
        t_amount_hint: rust_i18n::t!("costs.amount_hint", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("costs.new_asset", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_target: rust_i18n::t!("network.target", locale = l).to_string(),
        currency: 通貨,
        year,
        rows,
        devices: このプロジェクトの機器候補(state, project_id).await?,
        methods: cost::DEPRECIATION_METHODS.to_vec(),
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct AssetForm {
    pub device_id: i32,
    #[serde(default)]
    pub acquisition_cost: String,
    #[serde(default)]
    pub depreciation_method: String,
    #[serde(default)]
    pub useful_life_years: String,
    #[serde(default)]
    pub acquisition_date: String,
}

pub async fn create_asset(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<AssetForm>,
) -> AppResult<Response> {
    let (project, l) = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());
    let 戻る = |key: &str| 資産を描く(&state, &current, project_id, 誤り(key));

    if !このプロジェクトの機器id(&state, project_id)
        .await?
        .contains(&form.device_id)
    {
        return Err(AppError::NotFound);
    }
    // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）。
    // **定率法も選べる**——資産は実在し、記録できなくすると事実を書けない（10.3）
    if !cost::DEPRECIATION_METHODS.contains(&form.depreciation_method.as_str()) {
        return 戻る("costs.error_method").await;
    }
    let Some(cost_value) = currency::最小単位へ(&form.acquisition_cost, &project.currency)
    else {
        return 戻る("costs.error_amount").await;
    };
    // **耐用年数が0以下は按分できない**（24.2.2）
    let life = match form.useful_life_years.trim().parse::<i32>() {
        Ok(n) if n > 0 => n,
        _ => return 戻る("costs.error_life").await,
    };
    let Some(date) = 日付(&form.acquisition_date) else {
        return 戻る("costs.error_date").await;
    };

    let 重複 = fixed_asset::Entity::find()
        .filter(fixed_asset::Column::ItemType.eq(DEVICE))
        .filter(fixed_asset::Column::ItemId.eq(form.device_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複.is_some() {
        return 戻る("costs.error_asset_duplicate").await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(fixed_asset::ActiveModel {
        item_type: Set(DEVICE.to_owned()),
        item_id: Set(form.device_id),
        acquisition_cost: Set(cost_value),
        depreciation_method: Set(form.depreciation_method.clone()),
        useful_life_years: Set(life),
        acquisition_date: Set(date),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/costs/fixed-assets")).into_response())
}

// ---------------------------------------------------------------------------
// 定期費用（設計書10.2）
// ---------------------------------------------------------------------------

struct RecurringRow {
    item: String,
    cost_type: String,
    vendor: String,
    amount: String,
    billing_cycle: String,
    period: String,
    this_year: String,
    excluded: bool,
}

#[derive(askama::Template)]
#[template(path = "costs_recurring.html")]
struct RecurringPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_item: String,
    t_item_hint: String,
    t_cost_type: String,
    t_cost_type_hint: String,
    t_vendor: String,
    t_amount: String,
    t_amount_hint: String,
    t_cycle: String,
    t_cycle_hint: String,
    t_period: String,
    t_start: String,
    t_end: String,
    t_end_hint: String,
    t_this_year: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_unset: String,
    currency: String,
    year: i32,
    rows: Vec<RecurringRow>,
    containers: Vec<Labeled>,
    vendors: Vec<Labeled>,
    cycles: Vec<&'static str>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn recurring(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    定期費用を描く(&state, &current, project_id, None).await
}

async fn 定期費用を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit, l) = 入場(state, current, project_id).await?;
    let 通貨 = project.currency.clone();
    let year = Utc::now().year();

    let mut rows = Vec::new();
    for r in このプロジェクトの定期費用(state, project_id).await? {
        let end = r
            .end_date
            .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 12, 31).expect("年末"));
        let (this_year, excluded) =
            match cost::期間費用のその年の額(r.amount, r.start_date, end, year) {
                Ok(v) => (currency::表示(v, &通貨), false),
                Err(理由) => (rust_i18n::t!(理由.key(), locale = l).to_string(), true),
            };

        rows.push(RecurringRow {
            item: 品目の表示(state, &r.item_type, r.item_id).await?,
            vendor: ベンダー名(&state.db, r.vendor_id).await?,
            amount: currency::表示(r.amount, &通貨),
            period: match r.end_date {
                Some(e) => format!("{} 〜 {e}", r.start_date),
                // **null は継続中**（10.2）
                None => format!("{} 〜", r.start_date),
            },
            cost_type: r.cost_type,
            billing_cycle: r.billing_cycle,
            this_year,
            excluded,
        });
    }

    render(&RecurringPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "costs",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("costs.recurring", locale = l).to_string(),
        t_lead: rust_i18n::t!("costs.recurring_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("costs.title", locale = l).to_string(),
        t_item: rust_i18n::t!("costs.item", locale = l).to_string(),
        t_item_hint: rust_i18n::t!("costs.recurring_item_hint", locale = l).to_string(),
        t_cost_type: rust_i18n::t!("costs.cost_type", locale = l).to_string(),
        t_cost_type_hint: rust_i18n::t!("costs.cost_type_hint", locale = l).to_string(),
        t_vendor: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
        t_amount: rust_i18n::t!("costs.amount", locale = l).to_string(),
        t_amount_hint: rust_i18n::t!("costs.amount_hint", locale = l).to_string(),
        t_cycle: rust_i18n::t!("costs.billing_cycle", locale = l).to_string(),
        t_cycle_hint: rust_i18n::t!("costs.cycle_hint", locale = l).to_string(),
        t_period: rust_i18n::t!("costs.period", locale = l).to_string(),
        t_start: rust_i18n::t!("costs.start_date", locale = l).to_string(),
        t_end: rust_i18n::t!("costs.end_date", locale = l).to_string(),
        t_end_hint: rust_i18n::t!("costs.end_hint", locale = l).to_string(),
        t_this_year: rust_i18n::t!("costs.this_year", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("costs.new_recurring", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_unset: rust_i18n::t!("catalog.power_unset", locale = l).to_string(),
        currency: 通貨,
        year,
        rows,
        containers: このプロジェクトの什器(state, project_id).await?,
        vendors: 現役のベンダー(&state.db).await?,
        cycles: cost::BILLING_CYCLES.to_vec(),
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct RecurringForm {
    /// `Project` か `MountContainer:{id}`。
    #[serde(default)]
    pub item: String,
    #[serde(default)]
    pub cost_type: String,
    #[serde(default)]
    pub vendor_id: String,
    #[serde(default)]
    pub amount: String,
    #[serde(default)]
    pub billing_cycle: String,
    #[serde(default)]
    pub start_date: String,
    #[serde(default)]
    pub end_date: String,
}

pub async fn create_recurring(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<RecurringForm>,
) -> AppResult<Response> {
    let (project, l) = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());
    let 戻る = |key: &str| 定期費用を描く(&state, &current, project_id, 誤り(key));

    // **多態的参照。**プロジェクト全体か、特定の什器に付く（10.2）
    let (item_type, item_id) = match form.item.trim() {
        "" | PROJECT => (PROJECT.to_owned(), project_id),
        v => match v
            .strip_prefix("MountContainer:")
            .and_then(|n| n.parse().ok())
        {
            Some(id)
                if このプロジェクトの什器id(&state, project_id)
                    .await?
                    .contains(&id) =>
            {
                (MOUNT_CONTAINER.to_owned(), id)
            }
            _ => return 戻る("costs.error_item").await,
        },
    };
    debug_assert!(RECURRING_ITEM_TYPES.contains(&item_type.as_str()));

    let cost_type = 正規化(&form.cost_type);
    if cost_type.is_empty() {
        return 戻る("costs.error_cost_type").await;
    }
    if !cost::BILLING_CYCLES.contains(&form.billing_cycle.as_str()) {
        return 戻る("costs.error_cycle").await;
    }
    let Some(amount) = currency::最小単位へ(&form.amount, &project.currency) else {
        return 戻る("costs.error_amount").await;
    };
    let Some(start) = 日付(&form.start_date) else {
        return 戻る("costs.error_date").await;
    };
    // **null は継続中**（10.2）。空欄を誤りにしない
    let end = match form.end_date.trim() {
        "" => None,
        v => match 日付(v) {
            Some(d) if d >= start => Some(d),
            Some(_) => return 戻る("costs.error_period").await,
            None => return 戻る("costs.error_date").await,
        },
    };

    let vendor_id = match form.vendor_id.trim() {
        "" => None,
        v => match v.parse::<i32>() {
            Ok(id) => Some(id),
            Err(_) => return 戻る("cables.error_vendor").await,
        },
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(recurring_cost::ActiveModel {
        item_type: Set(item_type),
        item_id: Set(item_id),
        cost_type: Set(cost_type),
        vendor_id: Set(vendor_id),
        amount: Set(amount),
        billing_cycle: Set(form.billing_cycle.clone()),
        start_date: Set(start),
        end_date: Set(end),
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

    Ok(Redirect::to(&format!("/projects/{project_id}/costs/recurring")).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// `2026-04-01`。**曖昧な書式を受け付けない。**
fn 日付(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
}

async fn 入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<(project::Model, bool, &'static str)> {
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

    Ok((
        project,
        can_edit,
        Locale::parse(&current.user.locale).as_str(),
    ))
}

async fn 編集入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<(project::Model, &'static str)> {
    let (project, can_edit, l) = 入場(state, current, project_id).await?;
    if !can_edit {
        return Err(AppError::Forbidden);
    }
    Ok((project, l))
}

/// **A-6により、過去に所属した機器も含める。**移設された機器の資産や契約が
/// 見えなくなっては困る。
async fn このプロジェクトの機器id(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<i32>> {
    Ok(device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|a| a.device_id)
        .collect())
}

async fn このプロジェクトの機器候補(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    let ids = このプロジェクトの機器id(state, project_id).await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(device::Entity::find()
        .filter(device::Column::Id.is_in(ids))
        .order_by_asc(device::Column::Hostname)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|d| Labeled {
            label: d.hostname,
            value: d.id.to_string(),
        })
        .collect())
}

async fn このプロジェクトの什器id(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<i32>> {
    Ok(mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|c| c.id)
        .collect())
}

async fn このプロジェクトの什器(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    Ok(mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .order_by_asc(mount_container::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|c| Labeled {
            label: c.name,
            value: format!("MountContainer:{}", c.id),
        })
        .collect())
}

async fn このプロジェクトの資産(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<fixed_asset::Model>> {
    let ids = このプロジェクトの機器id(state, project_id).await?;
    Ok(fixed_asset::Entity::find()
        .filter(fixed_asset::Column::ItemType.eq(DEVICE))
        .order_by_asc(fixed_asset::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .filter(|a| ids.contains(&a.item_id))
        .collect())
}

/// このプロジェクトの機器を1つでもカバーしている契約（10.2）。
///
/// **1契約で複数品目をカバーできる**ため、品目から契約へ逆に辿る。
///
/// **ダッシュボード（16.1）と共有する。**満了間近の契約をあちらにも出すが、
/// 判定を書き写すと**コスト画面には出るのにダッシュボードに出ない契約**が
/// できる。同じ関数を通す。
pub(super) async fn このプロジェクトの契約(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<maintenance_contract::Model>> {
    let ids = このプロジェクトの機器id(state, project_id).await?;

    let mut 契約id: Vec<i32> = maintenance_contract_item::Entity::find()
        .filter(maintenance_contract_item::Column::ItemType.eq(DEVICE))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .filter(|i| ids.contains(&i.item_id))
        .map(|i| i.maintenance_contract_id)
        .collect();
    契約id.sort_unstable();
    契約id.dedup();

    if 契約id.is_empty() {
        return Ok(Vec::new());
    }
    maintenance_contract::Entity::find()
        .filter(maintenance_contract::Column::Id.is_in(契約id))
        .order_by_asc(maintenance_contract::Column::ContractNumber)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn このプロジェクトの定期費用(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<recurring_cost::Model>> {
    let 什器 = このプロジェクトの什器id(state, project_id).await?;

    Ok(recurring_cost::Entity::find()
        .order_by_asc(recurring_cost::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .filter(|r| match r.item_type.as_str() {
            PROJECT => r.item_id == project_id,
            MOUNT_CONTAINER => 什器.contains(&r.item_id),
            _ => false,
        })
        .collect())
}

async fn 契約の品目(state: &AppState, contract_id: i32) -> AppResult<String> {
    let items = maintenance_contract_item::Entity::find()
        .filter(maintenance_contract_item::Column::MaintenanceContractId.eq(contract_id))
        .order_by_asc(maintenance_contract_item::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut names = Vec::new();
    for i in items {
        names.push(品目の表示(state, &i.item_type, i.item_id).await?);
    }
    Ok(names.join(", "))
}

/// 多態的な参照を人が読める形にする（10.2）。
async fn 品目の表示(state: &AppState, item_type: &str, item_id: i32) -> AppResult<String> {
    let name = match item_type {
        DEVICE => device::Entity::find_by_id(item_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|d| d.hostname),
        MOUNT_CONTAINER => mount_container::Entity::find_by_id(item_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|c| c.name),
        PROJECT => project::Entity::find_by_id(item_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|p| p.name),
        _ => None,
    };
    // **参照先が消えていても行を隠さない。**多態的参照はDB制約で守れないため、
    // 壊れていることが見えるほうがよい（4章のC-7）
    Ok(name.unwrap_or_else(|| format!("{item_type}#{item_id}")))
}

async fn ベンダー名<C: ConnectionTrait>(db: &C, id: Option<i32>) -> AppResult<String> {
    let Some(id) = id else {
        return Ok(String::new());
    };
    Ok(vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|v| v.name)
        .unwrap_or_default())
}

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
