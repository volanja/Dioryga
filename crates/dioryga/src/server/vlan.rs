//! VLAN（設計書16.1のD領域、8.3、8.5）。
//!
//! # 同じタグを2つ登録できる
//!
//! **一意制約を張っていない。**VLANタグはL2ドメインごとに独立しており、
//! **拠点が違えば同じ `VLAN 100` が別物として存在する。**一意にすると、
//! 複数拠点を1つの台帳で扱えなくなる。
//!
//! そのぶん取り違えが起きやすいので、**同じタグが既にあるときは警告する**
//! （不変条件6）。禁止ではない——同じタグの併存は正当な状態である。
//!
//! # `zone` はネットワーク側の属性（8.5）
//!
//! `DMZ` / `WAN` / `LAN` / `Management` / `Isolated`。ネットワーク設計者が
//! 決める。ホスト側の `INTERFACE_ROLE` とは別軸であり、**両者の食い違い
//! （役割がBackupなのにDMZゾーンに載っている等）の検出**に使う。

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{interface_vlan, vlan};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{入場, 正規化, 編集権};
use crate::server::view::{render, Chrome};
use crate::server::AppState;

/// 閉じた語彙（`vocabularies.md`、設計書8.5）。
const ZONES: &[&str] = &["DMZ", "WAN", "LAN", "Management", "Isolated"];

/// IEEE 802.1Q。**0と4095は予約されている。**
const タグの下限: i32 = 1;
const タグの上限: i32 = 4094;

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub retired: Option<String>,
}

struct VlanRow {
    id: i32,
    vlan_tag: i32,
    name: String,
    zone: String,
    description: String,
    retired: bool,
    /// `INTERFACE_VLAN` から参照されている（18.2）。
    referenced: bool,
}

#[derive(askama::Template)]
#[template(path = "catalog_vlans.html")]
struct VlansPage {
    chrome: Chrome,
    t_apply: String,
    t_title: String,
    t_lead: String,
    t_duplicate_hint: String,
    t_tag: String,
    t_tag_hint: String,
    t_name: String,
    t_zone: String,
    t_zone_hint: String,
    t_description: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_referenced: String,
    t_show_retired: String,
    t_unset: String,
    rows: Vec<VlanRow>,
    zones: Vec<&'static str>,
    show_retired: bool,
    can_edit: bool,
    error: Option<String>,
    /// 同じタグが既にある、という警告（不変条件6）。**禁止ではない。**
    notice: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    一覧を描く(&state, &current, &query, None, None).await
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    query: &ListQuery,
    error: Option<String>,
    notice: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    let can_edit = 編集権(state, current).await.is_ok();
    let show_retired = query.retired.as_deref() == Some("1");

    let list = vlan::Entity::find()
        .order_by_asc(vlan::Column::VlanTag)
        .order_by_asc(vlan::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for v in list {
        if !show_retired && v.retired_at.is_some() {
            continue;
        }
        rows.push(VlanRow {
            referenced: 参照されている(&state.db, v.id).await?,
            retired: v.retired_at.is_some(),
            zone: v.zone.unwrap_or_default(),
            id: v.id,
            vlan_tag: v.vlan_tag,
            name: v.name,
            description: v.description,
        });
    }

    render(&VlansPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "catalog"),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_title: rust_i18n::t!("vlans.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("vlans.lead", locale = l).to_string(),
        t_duplicate_hint: rust_i18n::t!("vlans.duplicate_hint", locale = l).to_string(),
        t_tag: rust_i18n::t!("vlans.tag", locale = l).to_string(),
        t_tag_hint: rust_i18n::t!("vlans.tag_hint", locale = l).to_string(),
        t_name: rust_i18n::t!("catalog.name", locale = l).to_string(),
        t_zone: rust_i18n::t!("vlans.zone", locale = l).to_string(),
        t_zone_hint: rust_i18n::t!("vlans.zone_hint", locale = l).to_string(),
        t_description: rust_i18n::t!("vlans.description", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("vlans.new", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_retire: rust_i18n::t!("catalog.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("catalog.unretire", locale = l).to_string(),
        t_retired: rust_i18n::t!("catalog.retired", locale = l).to_string(),
        t_referenced: rust_i18n::t!("catalog.referenced", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("catalog.show_retired", locale = l).to_string(),
        t_unset: rust_i18n::t!("catalog.power_unset", locale = l).to_string(),
        rows,
        zones: ZONES.to_vec(),
        show_retired,
        can_edit,
        error,
        notice,
    })
}

#[derive(Debug, Deserialize)]
pub struct VlanForm {
    #[serde(default)]
    pub vlan_tag: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub description: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<VlanForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let query = ListQuery::default();
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **802.1Qの範囲外は拒否する。**0と4095は予約されており、実機に設定できない
    let tag = match form.vlan_tag.trim().parse::<i32>() {
        Ok(n) if (タグの下限..=タグの上限).contains(&n) => n,
        _ => return 一覧を描く(&state, &current, &query, 誤り("vlans.error_tag"), None).await,
    };

    let name = 正規化(&form.name);
    if name.is_empty() {
        return 一覧を描く(&state, &current, &query, 誤り("catalog.error_name"), None).await;
    }

    // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）。未設定は許す
    let zone = match form.zone.trim() {
        "" => None,
        z if ZONES.contains(&z) => Some(z.to_owned()),
        _ => return 一覧を描く(&state, &current, &query, 誤り("vlans.error_zone"), None).await,
    };

    // **同じタグは登録できる**（拠点が違えば同じタグが別物として存在する）。
    // ただし取り違えが起きやすいので警告する（不変条件6）
    let 既存 = vlan::Entity::find()
        .filter(vlan::Column::VlanTag.eq(tag))
        .filter(vlan::Column::RetiredAt.is_null())
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(vlan::ActiveModel {
        vlan_tag: Set(tag),
        name: Set(name),
        zone: Set(zone),
        description: Set(正規化(&form.description)),
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

    if let Some(既存) = 既存 {
        let notice = rust_i18n::t!(
            "vlans.warn_duplicate_tag",
            locale = l,
            tag = tag,
            name = 既存.name
        )
        .to_string();
        return 一覧を描く(&state, &current, &query, None, Some(notice)).await;
    }

    Ok(Redirect::to("/catalog/vlans").into_response())
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

    let before = vlan::Entity::find_by_id(form.id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        vlan::ActiveModel {
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

    Ok(Redirect::to("/catalog/vlans").into_response())
}

/// 18.2の判定。
async fn 参照されている<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<bool> {
    Ok(interface_vlan::Entity::find()
        .filter(interface_vlan::Column::VlanId.eq(id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some())
}
