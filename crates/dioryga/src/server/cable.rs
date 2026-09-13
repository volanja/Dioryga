//! ケーブルカタログ（設計書16.1のD領域、8.3、8.7）。
//!
//! # 端ごとに異なるコネクタを持てる（8.7）
//!
//! `CABLE_END_SLOT` を両端で別レコードにしてある。**NEMA 5-15P と C13、
//! LC と SC のような非対称なケーブルが実在する**ためで、ブレイクアウト
//! （MPO12 の Trunk と LC×4 の Branch）も同じ仕組みで表せる。
//!
//! # ネットワークと電源は種別と画面で分ける（8.7）
//!
//! **テーブルは分けない。**配線の実体は `CABLE_INSTANCE` →
//! `CABLE_END_SLOT` → `PART_PORT_SLOT` という接続グラフであり、**流れて
//! いるのが電気か光かでグラフの形は変わらない。**分けると `CABLE_CONNECTION`
//! （履歴）が2組になり、接続を辿るクエリがすべてUNIONになる。
//!
//! 一方で、**電源コードの登録画面に `OM4` の選択肢が並び、光ファイバの登録
//! 画面に定格電流の欄がある状態は入力の妨げになる。**`cable_kind` で一覧と
//! フォームを割ることで、そこだけ解消する。
//!
//! # 取込は無いが手入力はできる（16.1、23.8）
//!
//! ケーブルの**インスタンスと接続**（`CABLE_INSTANCE` / `CABLE_CONNECTION`）は
//! v2である。取込の需要が薄いことがその理由で（23.8）、**カタログは別**——
//! 数十行の手入力マスタであり、22.2の「手入力が成立しない規模」に当たらない。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{cable_catalog, cable_end_slot, vendor};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{入場, 正規化, 現役のベンダー, 編集権, Labeled};
use crate::server::view::{render, Chrome};
use crate::server::AppState;

/// 閉じた語彙（8.6）。**`PART_PORT_SLOT.port_kind` と同じ値を使う。**
const CABLE_KINDS: &[&str] = &["Network", "Power", "Stack"];

const POWER: &str = "Power";

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub retired: Option<String>,
    /// 種別で絞る（8.7）。既定は `Network`。
    #[serde(default)]
    pub kind: Option<String>,
}

struct CableRow {
    id: i32,
    cable_type: String,
    length: String,
    color: String,
    vendor: String,
    part_number: String,
    /// `AC 250V 12A` のようにまとめたもの。`cable_kind=Power` のときだけ埋まる。
    rating: String,
    ends: usize,
    retired: bool,
}

#[derive(askama::Template)]
#[template(path = "catalog_cables.html")]
struct CablesPage {
    chrome: Chrome,
    t_apply: String,
    t_detail: String,
    t_title: String,
    t_lead: String,
    t_v2_hint: String,
    t_kind: String,
    t_cable_type: String,
    t_cable_type_hint: String,
    t_length: String,
    t_length_hint: String,
    t_color: String,
    t_vendor: String,
    t_part_number: String,
    t_rating: String,
    t_rated_voltage: String,
    t_rated_current: String,
    t_rating_hint: String,
    t_ends: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_show_retired: String,
    t_unset: String,
    rows: Vec<CableRow>,
    vendors: Vec<Labeled>,
    kinds: Vec<&'static str>,
    /// いま見ている種別。**フォームの既定値にもなる。**
    kind: String,
    /// 電源のときだけ定格の欄を出す（8.7）。
    is_power: bool,
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

    // **語彙外の種別で絞ろうとしたら既定に戻す。**表示の切り替えであって
    // 保存する値ではないため、ここは拒否せず既定に寄せてよい
    let kind = match query.kind.as_deref() {
        Some(k) if CABLE_KINDS.contains(&k) => k.to_owned(),
        _ => CABLE_KINDS[0].to_owned(),
    };
    let show_retired = query.retired.as_deref() == Some("1");

    let list = cable_catalog::Entity::find()
        .order_by_asc(cable_catalog::Column::CableType)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for c in list {
        // **種別が未設定の行も出す。**DB上はnullableであり（8.7）、隠すと
        // 直す手段が無くなる。既定の種別の一覧に混ぜる
        let 同じ種別 = match c.cable_kind.as_deref() {
            Some(k) => k == kind,
            None => kind == CABLE_KINDS[0],
        };
        if !同じ種別 || (!show_retired && c.retired_at.is_some()) {
            continue;
        }

        rows.push(CableRow {
            vendor: match c.vendor_id {
                Some(id) => ベンダー名(&state.db, id).await?,
                None => String::new(),
            },
            ends: 端の数(&state.db, c.id).await?,
            length: c
                .length_mm
                .map(|mm| format!("{:.2}m", mm as f64 / 1000.0))
                .unwrap_or_default(),
            rating: 定格の表示(c.rated_voltage, c.rated_current_ma),
            retired: c.retired_at.is_some(),
            id: c.id,
            cable_type: c.cable_type,
            color: c.color,
            part_number: c.part_number.unwrap_or_default(),
        });
    }

    let is_power = kind == POWER;

    render(&CablesPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "cables"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_title: rust_i18n::t!("cables.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("cables.lead", locale = l).to_string(),
        t_v2_hint: rust_i18n::t!("cables.v2_hint", locale = l).to_string(),
        t_kind: rust_i18n::t!("cables.kind", locale = l).to_string(),
        t_cable_type: rust_i18n::t!("cables.cable_type", locale = l).to_string(),
        t_cable_type_hint: rust_i18n::t!("cables.cable_type_hint", locale = l).to_string(),
        t_length: rust_i18n::t!("cables.length", locale = l).to_string(),
        t_length_hint: rust_i18n::t!("cables.length_hint", locale = l).to_string(),
        t_color: rust_i18n::t!("cables.color", locale = l).to_string(),
        t_vendor: rust_i18n::t!("catalog.vendor", locale = l).to_string(),
        t_part_number: rust_i18n::t!("parts.part_number", locale = l).to_string(),
        t_rating: rust_i18n::t!("cables.rating", locale = l).to_string(),
        t_rated_voltage: rust_i18n::t!("cables.rated_voltage", locale = l).to_string(),
        t_rated_current: rust_i18n::t!("cables.rated_current", locale = l).to_string(),
        t_rating_hint: rust_i18n::t!("cables.rating_hint", locale = l).to_string(),
        t_ends: rust_i18n::t!("cables.ends", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("cables.new", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        t_unset: rust_i18n::t!("catalog.power_unset", locale = l).to_string(),
        rows,
        vendors: 現役のベンダー(&state.db).await?,
        kinds: CABLE_KINDS.to_vec(),
        kind,
        is_power,
        show_retired,
        can_edit,
        error,
    })
}

/// `250V 12A`。**両方無ければ空。**
fn 定格の表示(voltage: Option<i32>, current_ma: Option<i32>) -> String {
    let mut 部品 = Vec::new();
    if let Some(v) = voltage {
        部品.push(format!("{v}V"));
    }
    if let Some(ma) = current_ma {
        // **mAで持ち、Aで見せる**（24.2.1）。12000mA → 12A
        部品.push(format!("{:.1}A", ma as f64 / 1000.0));
    }
    部品.join(" ")
}

// ---------------------------------------------------------------------------
// 登録
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CableForm {
    #[serde(default)]
    pub cable_kind: String,
    #[serde(default)]
    pub cable_type: String,
    /// メートルで受け、ミリメートルの整数で保存する（24.2.1）。
    #[serde(default)]
    pub length_m: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub vendor_id: String,
    #[serde(default)]
    pub part_number: String,
    #[serde(default)]
    pub rated_voltage: String,
    /// アンペアで受け、mAの整数で保存する（24.2.1）。
    #[serde(default)]
    pub rated_current_a: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<CableForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;

    // 誤りを返すときも、利用者が見ていた種別の一覧へ戻す
    let query = ListQuery {
        retired: None,
        kind: Some(form.cable_kind.clone()),
    };
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
    if !CABLE_KINDS.contains(&form.cable_kind.as_str()) {
        return 一覧を描く(&state, &current, &query, 誤り("cables.error_kind")).await;
    }

    // **開いた語彙。**正規化はするが語彙外でも拒否しない（8.6）
    let cable_type = 正規化(&form.cable_type);
    if cable_type.is_empty() {
        return 一覧を描く(&state, &current, &query, 誤り("cables.error_type")).await;
    }

    // **メートルで受けてミリメートルで保存する**（24.2.1）
    let length_mm = match form.length_m.trim() {
        "" => None,
        v => match v.parse::<f64>() {
            Ok(m) if m > 0.0 => Some((m * 1000.0).round() as i32),
            _ => return 一覧を描く(&state, &current, &query, 誤り("cables.error_length")).await,
        },
    };

    let (rated_voltage, rated_current_ma) = match 定格を読む(&form) {
        Ok(v) => v,
        Err(key) => return 一覧を描く(&state, &current, &query, 誤り(key)).await,
    };
    // **意味を持たない列に値が来たら拒否する**（8.7、Q-21）。
    // 取込は警告に留めるが、画面は選択肢をサーバが描いているため改竄しかない
    if form.cable_kind != POWER && (rated_voltage.is_some() || rated_current_ma.is_some()) {
        return 一覧を描く(&state, &current, &query, 誤り("cables.error_rating_unused")).await;
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
    tx.insert(cable_catalog::ActiveModel {
        cable_kind: Set(Some(form.cable_kind.clone())),
        cable_type: Set(cable_type),
        length_mm: Set(length_mm),
        color: Set(正規化(&form.color)),
        vendor_id: Set(vendor_id),
        part_number: Set(match 正規化(&form.part_number) {
            v if v.is_empty() => None,
            v => Some(v),
        }),
        rated_voltage: Set(rated_voltage),
        rated_current_ma: Set(rated_current_ma),
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

    Ok(Redirect::to(&format!("/catalog/cables?kind={}", form.cable_kind)).into_response())
}

/// 定格を読む（8.7）。**アンペアで受けてmAで保存する**（24.2.1）。
fn 定格を読む(form: &CableForm) -> Result<(Option<i32>, Option<i32>), &'static str> {
    let voltage = match form.rated_voltage.trim() {
        "" => None,
        v => Some(v.parse::<i32>().map_err(|_| "cables.error_rating_number")?),
    };
    let current_ma = match form.rated_current_a.trim() {
        "" => None,
        v => {
            let a: f64 = v.parse().map_err(|_| "cables.error_rating_number")?;
            if a <= 0.0 {
                return Err("cables.error_rating_positive");
            }
            Some((a * 1000.0).round() as i32)
        }
    };
    Ok((voltage, current_ma))
}

// ---------------------------------------------------------------------------
// 詳細（端、設計書8.7）
// ---------------------------------------------------------------------------

struct EndRow {
    id: i32,
    end_label: String,
    connector_type: String,
    port_speed: String,
}

#[derive(askama::Template)]
#[template(path = "catalog_cable_detail.html")]
struct CableDetailPage {
    chrome: Chrome,
    cable_id: i32,
    t_back: String,
    t_basic: String,
    t_ends: String,
    t_ends_hint: String,
    t_end_label: String,
    t_end_label_hint: String,
    t_connector_type: String,
    t_connector_hint: String,
    t_port_speed: String,
    t_add_end: String,
    t_remove: String,
    t_empty: String,
    t_actions: String,
    title: String,
    basic: Vec<Labeled>,
    ends: Vec<EndRow>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    詳細を描く(&state, &current, id, None).await
}

async fn 詳細を描く(
    state: &AppState,
    current: &CurrentUser,
    id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = 編集権(state, current).await.is_ok();

    let c = cable_catalog::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let mut basic = vec![
        Labeled {
            label: rust_i18n::t!("cables.kind", locale = l).to_string(),
            value: c.cable_kind.clone().unwrap_or_default(),
        },
        Labeled {
            label: rust_i18n::t!("cables.cable_type", locale = l).to_string(),
            value: c.cable_type.clone(),
        },
    ];
    if let Some(mm) = c.length_mm {
        basic.push(Labeled {
            label: rust_i18n::t!("cables.length", locale = l).to_string(),
            value: format!("{:.2}m", mm as f64 / 1000.0),
        });
    }
    if !c.color.is_empty() {
        basic.push(Labeled {
            label: rust_i18n::t!("cables.color", locale = l).to_string(),
            value: c.color.clone(),
        });
    }
    // **定格は電源のときだけ意味を持つ**（8.7）
    let rating = 定格の表示(c.rated_voltage, c.rated_current_ma);
    if !rating.is_empty() {
        basic.push(Labeled {
            label: rust_i18n::t!("cables.rating", locale = l).to_string(),
            value: rating,
        });
    }

    let ends = cable_end_slot::Entity::find()
        .filter(cable_end_slot::Column::CableCatalogId.eq(id))
        .order_by_asc(cable_end_slot::Column::Id)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|e| EndRow {
            id: e.id,
            end_label: e.end_label,
            connector_type: e.connector_type,
            port_speed: e.port_speed.unwrap_or_default(),
        })
        .collect();

    render(&CableDetailPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), "cables"),
        cable_id: id,
        t_back: rust_i18n::t!("cables.back", locale = l).to_string(),
        t_basic: rust_i18n::t!("devices.basic", locale = l).to_string(),
        t_ends: rust_i18n::t!("cables.ends", locale = l).to_string(),
        t_ends_hint: rust_i18n::t!("cables.ends_hint", locale = l).to_string(),
        t_end_label: rust_i18n::t!("cables.end_label", locale = l).to_string(),
        t_end_label_hint: rust_i18n::t!("cables.end_label_hint", locale = l).to_string(),
        t_connector_type: rust_i18n::t!("parts.connector_type", locale = l).to_string(),
        t_connector_hint: rust_i18n::t!("parts.connector_hint", locale = l).to_string(),
        t_port_speed: rust_i18n::t!("parts.port_speed", locale = l).to_string(),
        t_add_end: rust_i18n::t!("cables.add_end", locale = l).to_string(),
        t_remove: rust_i18n::t!("parts.remove_port", locale = l).to_string(),
        t_empty: rust_i18n::t!("cables.no_end", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        title: format!("{} {}", c.cable_type, c.color).trim().to_owned(),
        basic,
        ends,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct EndForm {
    #[serde(default)]
    pub end_label: String,
    #[serde(default)]
    pub connector_type: String,
    #[serde(default)]
    pub port_speed: String,
}

/// 端を足す。
///
/// **端の数を2に縛らない。**MPOブレイクアウトは Trunk 1本と Branch 4本を持つ
/// （8.7）。同じラベルだけは重複させない
pub async fn add_end(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<EndForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    cable_catalog::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let label = 正規化(&form.end_label);
    let connector = 正規化(&form.connector_type);
    if label.is_empty() || connector.is_empty() {
        return 詳細を描く(&state, &current, id, 誤り("cables.error_end_required")).await;
    }

    let 重複 = cable_end_slot::Entity::find()
        .filter(cable_end_slot::Column::CableCatalogId.eq(id))
        .filter(cable_end_slot::Column::EndLabel.eq(&label))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複.is_some() {
        return 詳細を描く(&state, &current, id, 誤り("cables.error_end_duplicate")).await;
    }

    let speed = 正規化(&form.port_speed);

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(cable_end_slot::ActiveModel {
        cable_catalog_id: Set(id),
        end_label: Set(label),
        connector_type: Set(connector),
        port_speed: Set((!speed.is_empty()).then_some(speed)),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/catalog/cables/{id}")).into_response())
}

#[derive(Debug, Deserialize)]
pub struct RemoveEndForm {
    pub end_id: i32,
}

pub async fn remove_end(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<RemoveEndForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

    let row = cable_end_slot::Entity::find_by_id(form.end_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|r| r.cable_catalog_id == id)
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

    Ok(Redirect::to(&format!("/catalog/cables/{id}")).into_response())
}

// ---------------------------------------------------------------------------
// 廃番（設計書18.5）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RetireForm {
    pub id: i32,
    #[serde(default)]
    pub undo: String,
    #[serde(default)]
    pub kind: String,
}

pub async fn retire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<RetireForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;

    let before = cable_catalog::Entity::find_by_id(form.id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        cable_catalog::ActiveModel {
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

    Ok(Redirect::to(&format!("/catalog/cables?kind={}", form.kind)).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 端の数<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<usize> {
    Ok(cable_end_slot::Entity::find()
        .filter(cable_end_slot::Column::CableCatalogId.eq(id))
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
