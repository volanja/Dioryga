//! 設備・什器の型番のカタログ（設計書12.10、16.1のD領域、#205）。
//!
//! # 型番で決まるものはカタログに置く
//!
//! 種別、収容能力（RackのU数、Shelvingの段数）、寸法・重量・静荷重は型番で
//! 決まる。設備・什器（`MOUNT_CONTAINER`）はこれを選ぶ。機器の機種
//! （`CHASSIS_MODEL`）と同じ置き方である。
//!
//! # 登録する項目は種別で分ける
//!
//! Rack は U数、Shelving は段数を持ち、Desk は収容能力を持たない（12.9の格子の
//! 違いと同じ）。ケーブルの電源・ネットワークと同じく、**一覧で選んだ種別で
//! 登録画面の欄を出し分け、種別に合わない値は拒否する。**
//!
//! # 登録した後は変えない
//!
//! 参照されたカタログ行はスペックを編集できない（18.2）。**編集の画面を持たず、
//! 誤って登録したものは廃番にして登録し直す**——機種（`CHASSIS_MODEL`）と同じ。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{container_model, mount_container, vendor};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::container::{CONTAINER_TYPES, RACK, SHELVING};
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{入場, 正規化, 現役のベンダー, 編集権, Labeled};
use crate::server::view::{render, Chrome};
use crate::server::AppState;

const NAV: &str = "container_models";

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub retired: Option<String>,
    /// 種別で絞る。既定は `Rack`。
    #[serde(default, rename = "type")]
    pub container_type: Option<String>,
}

struct ModelRow {
    id: i32,
    vendor: String,
    model_name: String,
    capacity: String,
    size: String,
    weight: String,
    static_load: String,
    used: u64,
    retired: bool,
}

#[derive(askama::Template)]
#[template(path = "catalog_container_models.html")]
struct ListPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_type: String,
    t_vendor: String,
    t_model_name: String,
    t_capacity: String,
    t_size: String,
    t_weight: String,
    t_static_load: String,
    t_used: String,
    t_actions: String,
    t_empty: String,
    t_new: String,
    t_retire: String,
    t_unretire: String,
    t_retired: String,
    t_show_retired: String,
    t_apply: String,
    rows: Vec<ModelRow>,
    types: Vec<&'static str>,
    container_type: String,
    /// 収容能力の列を出すか（Desk は持たない）。
    has_capacity: bool,
    show_retired: bool,
    can_edit: bool,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    let can_edit = 編集権(&state, &current).await.is_ok();
    let container_type = 種別(query.container_type.as_deref());
    let show_retired = query.retired.as_deref() == Some("1");

    let models = container_model::Entity::find()
        .filter(container_model::Column::ContainerType.eq(&container_type))
        .order_by_asc(container_model::Column::ModelName)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for m in models
        .into_iter()
        .filter(|m| show_retired || m.retired_at.is_none())
    {
        rows.push(ModelRow {
            vendor: ベンダー名(&state.db, m.vendor_id).await?,
            capacity: crate::container::収容能力(&m)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            size: 寸法の表示(&m),
            weight: kg(m.weight_g),
            static_load: kg(m.static_load_g),
            used: 使っている数(&state.db, m.id).await?,
            retired: m.retired_at.is_some(),
            id: m.id,
            model_name: m.model_name,
        });
    }

    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    render(&ListPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), NAV),
        t_title: t("container_models.title"),
        t_lead: t("container_models.lead"),
        t_type: t("container_models.container_type"),
        t_vendor: t("catalog.vendor"),
        t_model_name: t("container_models.model_name"),
        t_capacity: 収容能力の見出し(&container_type, l),
        t_size: t("container_models.size"),
        t_weight: t("container_models.weight"),
        t_static_load: t("container_models.static_load"),
        t_used: t("container_models.used"),
        t_actions: t("projects.actions"),
        t_empty: t("catalog.empty"),
        t_new: t("container_models.new"),
        t_retire: t("catalog.retire"),
        t_unretire: t("catalog.unretire"),
        t_retired: t("catalog.retired"),
        t_show_retired: t("catalog.show_retired"),
        t_apply: t("catalog.apply"),
        rows,
        types: CONTAINER_TYPES.to_vec(),
        has_capacity: container_type != crate::container::DESK,
        container_type,
        show_retired,
        can_edit,
    })
}

// ---------------------------------------------------------------------------
// 登録
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ModelForm {
    #[serde(default)]
    pub container_type: String,
    #[serde(default)]
    pub vendor_id: String,
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub height_u: String,
    #[serde(default)]
    pub shelf_count: String,
    #[serde(default)]
    pub width_mm: String,
    #[serde(default)]
    pub depth_mm: String,
    #[serde(default)]
    pub height_mm: String,
    /// kg で受け、g の整数で保存する（24.2.1）。
    #[serde(default)]
    pub weight_kg: String,
    #[serde(default)]
    pub static_load_kg: String,
}

#[derive(askama::Template)]
#[template(path = "catalog_container_model_form.html")]
struct FormPage {
    chrome: Chrome,
    t_title: String,
    t_back: String,
    t_vendor: String,
    t_model_name: String,
    t_height_u: String,
    t_shelf_count: String,
    t_width: String,
    t_depth: String,
    t_height: String,
    t_size_hint: String,
    t_weight: String,
    t_static_load: String,
    t_weight_hint: String,
    t_unset: String,
    t_submit: String,
    vendors: Vec<Labeled>,
    container_type: String,
    is_rack: bool,
    is_shelving: bool,
    form: ModelForm,
    error: Option<String>,
}

async fn 登録を描く(
    state: &AppState,
    current: &CurrentUser,
    form: ModelForm,
    error: Option<String>,
) -> AppResult<Response> {
    let l = 入場(state, current)?;
    編集権(state, current).await?;
    let container_type = 種別(Some(&form.container_type));
    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();

    render(&FormPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), NAV),
        t_title: t("container_models.new"),
        t_back: t("container_models.back"),
        t_vendor: t("catalog.vendor"),
        t_model_name: t("container_models.model_name"),
        t_height_u: t("container_models.height_u"),
        t_shelf_count: t("container_models.shelf_count"),
        t_width: t("container_models.width"),
        t_depth: t("container_models.depth"),
        t_height: t("container_models.height"),
        t_size_hint: t("container_models.size_hint"),
        t_weight: t("container_models.weight"),
        t_static_load: t("container_models.static_load"),
        t_weight_hint: t("container_models.weight_hint"),
        t_unset: t("catalog.unset"),
        t_submit: t("catalog.submit"),
        vendors: 現役のベンダー(&state.db).await?,
        is_rack: container_type == RACK,
        is_shelving: container_type == SHELVING,
        container_type,
        form,
        error,
    })
}

pub async fn new_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let form = ModelForm {
        container_type: 種別(query.container_type.as_deref()),
        ..Default::default()
    };
    登録を描く(&state, &current, form, None).await
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<ModelForm>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    編集権(&state, &current).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    match 読み取る(&state, &form).await? {
        Err(key) => 登録を描く(&state, &current, form, 誤り(key)).await,
        Ok(新) => {
            let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
            let now = Utc::now();
            let m = tx
                .insert(container_model::ActiveModel {
                    created_by: Set(current.user.id),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..新
                })
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
            tx.commit()
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
            Ok(Redirect::to(&format!("/catalog/container-models/{}", m.id)).into_response())
        }
    }
}

/// 入力を検証して行にする。**返すのは i18n のキー。**
async fn 読み取る(
    state: &AppState,
    form: &ModelForm,
) -> AppResult<Result<container_model::ActiveModel, &'static str>> {
    // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
    if !CONTAINER_TYPES.contains(&form.container_type.as_str()) {
        return Ok(Err("container_models.error_type"));
    }
    let Ok(vendor_id) = form.vendor_id.trim().parse::<i32>() else {
        return Ok(Err("container_models.error_vendor"));
    };
    let 現役 = vendor::Entity::find_by_id(vendor_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some_and(|v| v.retired_at.is_none() && v.merged_into_vendor_id.is_none());
    if !現役 {
        return Ok(Err("container_models.error_vendor"));
    }
    let model_name = 正規化(&form.model_name);
    if model_name.is_empty() {
        return Ok(Err("container_models.error_model_name"));
    }
    // 自然キー（ベンダー＋型番）。DBの一意索引でも止まるが、500ではなく入力に戻す
    let 重複 = container_model::Entity::find()
        .filter(container_model::Column::VendorId.eq(vendor_id))
        .filter(container_model::Column::ModelName.eq(&model_name))
        .count(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複 > 0 {
        return Ok(Err("container_models.error_duplicate"));
    }

    // **種別に合わない収容能力は拒否する。**画面は欄を出し分けているため、
    // 届くのは改竄か不具合しかない（Q-21）
    let is_rack = form.container_type == RACK;
    let is_shelving = form.container_type == SHELVING;
    let height_u = match 正の整数(&form.height_u) {
        Ok(v) => v,
        Err(()) => return Ok(Err("container_models.error_number")),
    };
    let shelf_count = match 正の整数(&form.shelf_count) {
        Ok(v) => v,
        Err(()) => return Ok(Err("container_models.error_number")),
    };
    if (!is_rack && height_u.is_some()) || (!is_shelving && shelf_count.is_some()) {
        return Ok(Err("container_models.error_capacity_unused"));
    }
    if (is_rack && height_u.is_none()) || (is_shelving && shelf_count.is_none()) {
        return Ok(Err("container_models.error_capacity_required"));
    }

    let mut 数 = Vec::new();
    for v in [&form.width_mm, &form.depth_mm, &form.height_mm] {
        match 正の整数(v) {
            Ok(n) => 数.push(n),
            Err(()) => return Ok(Err("container_models.error_number")),
        }
    }
    let (Ok(weight_g), Ok(static_load_g)) =
        (kgをgに(&form.weight_kg), kgをgに(&form.static_load_kg))
    else {
        return Ok(Err("container_models.error_number"));
    };

    Ok(Ok(container_model::ActiveModel {
        vendor_id: Set(vendor_id),
        model_name: Set(model_name),
        container_type: Set(form.container_type.clone()),
        height_u: Set(height_u),
        shelf_count: Set(shelf_count),
        width_mm: Set(数[0]),
        depth_mm: Set(数[1]),
        height_mm: Set(数[2]),
        weight_g: Set(weight_g),
        static_load_g: Set(static_load_g),
        retired_at: Set(None),
        ..Default::default()
    }))
}

// ---------------------------------------------------------------------------
// 詳細
// ---------------------------------------------------------------------------

#[derive(askama::Template)]
#[template(path = "catalog_container_model_detail.html")]
struct DetailPage {
    chrome: Chrome,
    t_back: String,
    t_basic: String,
    t_retired: String,
    title: String,
    retired: bool,
    basic: Vec<Labeled>,
}

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let l = 入場(&state, &current)?;
    let m = container_model::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;
    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();

    let mut basic = vec![
        Labeled {
            label: t("catalog.vendor"),
            value: ベンダー名(&state.db, m.vendor_id).await?,
        },
        Labeled {
            label: t("container_models.container_type"),
            value: m.container_type.clone(),
        },
    ];
    if let Some(c) = crate::container::収容能力(&m) {
        basic.push(Labeled {
            label: 収容能力の見出し(&m.container_type, l),
            value: c.to_string(),
        });
    }
    for (key, value) in [
        ("container_models.size", 寸法の表示(&m)),
        ("container_models.weight", kg(m.weight_g)),
        ("container_models.static_load", kg(m.static_load_g)),
        (
            "container_models.used",
            使っている数(&state.db, m.id).await?.to_string(),
        ),
    ] {
        if !value.is_empty() {
            basic.push(Labeled {
                label: t(key),
                value,
            });
        }
    }

    render(&DetailPage {
        chrome: Chrome::catalog(&current.user, current.csrf_token.clone(), NAV),
        t_back: t("container_models.back"),
        t_basic: t("devices.basic"),
        t_retired: t("catalog.retired"),
        title: format!(
            "{} {}",
            ベンダー名(&state.db, m.vendor_id).await?,
            m.model_name
        ),
        retired: m.retired_at.is_some(),
        basic,
    })
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

/// 廃番にする／取り消す。**参照済みでも設定できる**（18.5）。既に指している
/// 設備・什器はそのまま残り、新たに選べなくなるだけである。
pub async fn retire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<RetireForm>,
) -> AppResult<Response> {
    入場(&state, &current)?;
    編集権(&state, &current).await?;
    let before = container_model::Entity::find_by_id(form.id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &before,
        container_model::ActiveModel {
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

    Ok(Redirect::to(&format!(
        "/catalog/container-models?type={}",
        before.container_type
    ))
    .into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// **語彙外の種別で絞ろうとしたら既定に戻す。**表示の切り替えであって、
/// 保存する値ではない（登録の検証は別に行う）。
fn 種別(value: Option<&str>) -> String {
    match value {
        Some(t) if CONTAINER_TYPES.contains(&t) => t.to_owned(),
        _ => RACK.to_owned(),
    }
}

fn 収容能力の見出し(container_type: &str, l: &str) -> String {
    let key = if container_type == SHELVING {
        "container_models.shelf_count"
    } else {
        "container_models.height_u"
    };
    rust_i18n::t!(key, locale = l).to_string()
}

/// 空なら `None`、正の整数でなければ誤り。
fn 正の整数(v: &str) -> Result<Option<i32>, ()> {
    match v.trim() {
        "" => Ok(None),
        v => match v.parse::<i32>() {
            Ok(n) if n > 0 => Ok(Some(n)),
            _ => Err(()),
        },
    }
}

/// kg で受けて g の整数にする（24.2.1）。
pub(crate) fn kgをgに(v: &str) -> Result<Option<i32>, ()> {
    match v.trim() {
        "" => Ok(None),
        v => match v.parse::<f64>() {
            Ok(kg) if kg > 0.0 && kg < 2_000_000.0 => Ok(Some((kg * 1000.0).round() as i32)),
            _ => Err(()),
        },
    }
}

/// g を kg で見せる。無ければ空。
pub(crate) fn kg(g: Option<i32>) -> String {
    g.map(|g| format!("{:.1}kg", f64::from(g) / 1000.0))
        .unwrap_or_default()
}

/// `600×1200×2000mm`。どれかが欠けていれば `-` で埋める。すべて無ければ空。
fn 寸法の表示(m: &container_model::Model) -> String {
    if m.width_mm.is_none() && m.depth_mm.is_none() && m.height_mm.is_none() {
        return String::new();
    }
    let f = |v: Option<i32>| v.map(|v| v.to_string()).unwrap_or_else(|| "-".to_owned());
    format!("{}×{}×{}mm", f(m.width_mm), f(m.depth_mm), f(m.height_mm))
}

async fn 使っている数<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<u64> {
    mount_container::Entity::find()
        .filter(mount_container::Column::ContainerModelId.eq(id))
        .count(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn ベンダー名<C: ConnectionTrait>(db: &C, id: i32) -> AppResult<String> {
    Ok(vendor::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|v| v.name)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kgをgの整数にする() {
        assert_eq!(kgをgに("18.5"), Ok(Some(18_500)));
        assert_eq!(kgをgに(" 0.8 "), Ok(Some(800)));
        assert_eq!(kgをgに(""), Ok(None));
        assert_eq!(kgをgに("0"), Err(()));
        assert_eq!(kgをgに("abc"), Err(()));
    }
}
