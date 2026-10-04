//! 什器と搭載位置、およびラック図（設計書12章、12.9）。
//!
//! # 図は事実をそのまま映す
//!
//! 12.3が重複配置の検証をDB制約ではなくアプリ層に置き、6.1がスロット本数の
//! 超過を「エラーではなく警告」としたのと同じ方針で、**この画面も誤った状態を
//! 描いたうえで警告する。**収容能力を超えた位置の機器を隠すと、誤登録に気付け
//! なくなる（不変条件6）。
//!
//! # U番号は下から数える
//!
//! 実機のラックは下が1Uである（12.9）。`position` は実機のラベルと一致して
//! いなければ現場で図と実機を照合できないため、**データ側は反転させず、描画時に
//! 一度だけ上下を入れ替える。**
//!
//! # 前面と背面は別の図にする
//!
//! `depth_position` は同じ `position` に `Front` と `Rear` が同居しうる（12.3）。
//! 1枚に重ねるとどちらの機器か判別できないため、2枚並べる（12.9）。
//!
//! # 予約は破線と淡色の両方で描く
//!
//! `status=plan` の機器は予約中である（11.6）。**破線だけでは縮小・印刷で潰れ、
//! 色だけでは色覚特性によっては区別できない**ため、両方を使い、凡例に文字の
//! ラベルも置く（12.9）。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    chassis_model, configuration, container_model, device, device_assignment, device_mount,
    fixed_asset, mount_container, power_circuit, project, recurring_cost, vendor,
};
use sea_orm::sea_query::{Expr, Query as SeaQuery};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::container::{self as 設備, 撤去の結末};
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

use crate::container::{DESK, RACK};

/// `MOUNT_CONTAINER.location_type` / `DEVICE_ASSIGNMENT.location_type`。
const PROJECT: &str = "Project";

/// 予約中の機器（設計書11.6）。
/// 機器の状態（8.6）
const DEVICE_PLANNED: &str = "planned";

/// `CHASSIS_MODEL.mount_form`（設計書12.3）。
const RACK_SIDE: &str = "RackSide";
const HALF: &str = "Half";

const LEFT: &str = "Left";
const RIGHT: &str = "Right";
const FRONT: &str = "Front";
const REAR: &str = "Rear";

// ---------------------------------------------------------------------------
// 図の寸法（設計書12.9）
// ---------------------------------------------------------------------------

/// 1Uの高さ（px）。これを変えると図全体が拡縮する。
const U高: i32 = 22;
/// U番号を書く左端の幅。
const 番号欄: i32 = 30;
/// ラック1枚の内側の幅。
const 枠幅: i32 = 230;
/// 前面図と背面図の間隔。
const 図間: i32 = 24;
/// 見出しの高さ。
const 見出し高: i32 = 20;

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct ContainerRow {
    id: i32,
    name: String,
    /// 型番（ベンダー＋型番、#205）。
    model: String,
    installation_site: String,
    container_type: String,
    capacity: String,
    mounted: usize,
    /// 撤去済み（#204）。一覧の既定では出さない。
    retired: bool,
}

#[derive(askama::Template)]
#[template(path = "containers.html")]
struct ContainersPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_name: String,
    t_model: String,
    t_model_hint: String,
    t_site: String,
    t_site_hint: String,
    t_container_type: String,
    t_capacity: String,
    t_unset: String,
    t_mounted: String,
    t_actions: String,
    t_open: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_retired: String,
    t_show_retired: String,
    t_apply: String,
    t_retire: String,
    t_unretire: String,
    rows: Vec<ContainerRow>,
    /// 登録で選べる型番（廃番を除く、18.5）。
    models: Vec<Labeled>,
    can_edit: bool,
    show_retired: bool,
    /// **誤りのときは入力を保ったまま戻す**（16.1）。
    form: ContainerForm,
    error: Option<String>,
}

/// 撤去の確認画面（#204）。**何が起きるかを示してから実行させる**——同じ
/// 「撤去」が、設備によって行ごと消えるか、撤去として残るかに分かれる。
#[derive(askama::Template)]
#[template(path = "container_retire.html")]
struct ContainerRetirePage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    container_id: i32,
    name: String,
    t_title: String,
    t_back: String,
    t_lead: String,
    t_submit: String,
    can_retire: bool,
}

/// 図に描く1つの箱。**座標計算はRust側で終える**（テンプレートに算術を持ち込まない）。
struct Cell {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    label: String,
    /// `status=plan`。破線＋淡色で描く（設計書11.6、12.9）。
    planned: bool,
    /// 棚板の上に載っている機器（`host_device_id`）。**帯の中に**小さく並べる
    /// （設計書12.9）。1Uは22pxしかないので、ラベルと同じ行の右端へ寄せる。
    shelf_label: String,
}

/// U番号の目盛り。
struct Tick {
    y: i32,
    label: String,
}

/// 前面図・背面図の1枚。
///
/// **座標はすべてここで確定させる。**テンプレートに算術を持ち込むと、
/// 図の寸法を変えたときに直す場所が2箇所に分かれる。
struct Panel {
    /// この図の左端のx（U番号欄を含む）。
    x: i32,
    /// 枠の左端。U番号欄のぶん右にずれる。
    frame_x: i32,
    /// 枠の右端。目盛りの線をここまで引く。
    right_x: i32,
    frame_w: i32,
    title: String,
    cells: Vec<Cell>,
}

struct SideItem {
    label: String,
    planned: bool,
}

#[derive(askama::Template)]
#[template(path = "rack.html")]
struct RackPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    container_id: i32,
    container_name: String,
    container_type: String,
    t_back: String,
    t_diagram: String,
    t_legend_planned: String,
    t_legend_shelf: String,
    t_side: String,
    t_side_hint: String,
    t_out_of_range: String,
    t_mount: String,
    t_device: String,
    t_position: String,
    t_position_hint: String,
    t_horizontal: String,
    t_depth: String,
    t_host: String,
    t_host_hint: String,
    t_unset: String,
    t_submit: String,
    t_mounted: String,
    t_unmount: String,
    t_empty: String,
    t_no_grid: String,
    /// SVGの全体寸法。
    svg_width: i32,
    svg_height: i32,
    grid_top: i32,
    grid_height: i32,
    panels: Vec<Panel>,
    ticks: Vec<Tick>,
    /// 0Uサイドマウント。**格子の外に置く**（設計書12.9）。
    side_items: Vec<SideItem>,
    /// `capacity` を超えた位置の機器。描いたうえで警告する（不変条件6）。
    out_of_range: Vec<SideItem>,
    /// 格子を持たない什器（Desk）。並べるだけ（設計書12.9）。
    loose: Vec<SideItem>,
    has_grid: bool,
    /// 載せた機器の重量の合計と静荷重（#205）。
    load: String,
    /// 静荷重を超えている。**警告に留める**（不変条件6）。
    overloaded: bool,
    t_load: String,
    t_overloaded: String,
    /// 撤去済み（#204）。搭載の欄を出さない。
    retired: bool,
    t_retired_note: String,
    // 設置場所（#206）
    installation_site: String,
    t_site: String,
    t_site_hint: String,
    t_save: String,
    // 給電（回路、#206、設計書12.10）
    circuits: Vec<CircuitRow>,
    phases: Vec<&'static str>,
    power: String,
    t_power: String,
    t_circuits: String,
    t_circuits_hint: String,
    t_circuit_label: String,
    t_voltage: String,
    t_phase: String,
    t_breaker: String,
    t_continuous: String,
    t_connector: String,
    t_since: String,
    t_end_circuit: String,
    t_add_circuit: String,
    t_circuit_form_hint: String,
    t_no_circuits: String,
    // 費用（#206）。**列は足さず、購入の記録・固定資産・継続費用に寄せる**（10.2）
    purchase: Option<crate::server::device::PurchaseView>,
    purchase_form: crate::server::device::PurchaseForm,
    purchase_action: String,
    cost_rows: Vec<Labeled>,
    t_costs: String,
    t_costs_hint: String,
    t_to_assets: String,
    t_to_recurring: String,
    devices: Vec<Labeled>,
    hosts: Vec<Labeled>,
    mounted: Vec<MountedRow>,
    horizontals: Vec<&'static str>,
    depths: Vec<&'static str>,
    can_edit: bool,
    error: Option<String>,
    notice: Option<String>,
}

struct Labeled {
    label: String,
    value: String,
}

struct MountedRow {
    mount_id: i32,
    device: String,
    place: String,
}

// ---------------------------------------------------------------------------
// 什器の一覧・登録
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// `1` なら撤去済みも出す（#204）。
    #[serde(default)]
    pub retired: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let show_retired = query.retired.as_deref() == Some("1");
    一覧を描く(
        &state,
        &current,
        project_id,
        show_retired,
        ContainerForm::default(),
        None,
    )
    .await
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    show_retired: bool,
    form: ContainerForm,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let containers = mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .order_by_asc(mount_container::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    // **撤去した設備は既定で隠す**（12.10）。履歴から名前を引くために行は残っている
    for c in containers
        .into_iter()
        .filter(|c| show_retired || c.retired_at.is_none())
    {
        let mounted = device_mount::Entity::find()
            .filter(device_mount::Column::ContainerId.eq(c.id))
            .filter(device_mount::Column::ToDate.is_null())
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .len();

        let 型 = 設備::型を引く(&state.db, &c)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        rows.push(ContainerRow {
            id: c.id,
            name: c.name,
            model: 型番の表示(&state.db, 型.model.as_ref()).await?,
            installation_site: c.installation_site.clone().unwrap_or_default(),
            container_type: 型.container_type,
            capacity: 型.capacity.map(|v| v.to_string()).unwrap_or_default(),
            mounted,
            retired: c.retired_at.is_some(),
        });
    }

    render(&ContainersPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "containers",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("containers.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("containers.lead", locale = l).to_string(),
        t_name: rust_i18n::t!("containers.name", locale = l).to_string(),
        t_model: rust_i18n::t!("containers.model", locale = l).to_string(),
        t_model_hint: rust_i18n::t!("containers.model_hint", locale = l).to_string(),
        t_site: rust_i18n::t!("containers.installation_site", locale = l).to_string(),
        t_site_hint: rust_i18n::t!("containers.installation_site_hint", locale = l).to_string(),
        t_container_type: rust_i18n::t!("containers.container_type", locale = l).to_string(),
        t_capacity: rust_i18n::t!("containers.capacity", locale = l).to_string(),
        t_unset: rust_i18n::t!("catalog.unset", locale = l).to_string(),
        t_mounted: rust_i18n::t!("containers.mounted", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_open: rust_i18n::t!("containers.open", locale = l).to_string(),
        t_empty: rust_i18n::t!("containers.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("containers.new", locale = l).to_string(),
        t_submit: rust_i18n::t!("containers.submit", locale = l).to_string(),
        t_retired: rust_i18n::t!("containers.retired", locale = l).to_string(),
        t_show_retired: rust_i18n::t!("containers.show_retired", locale = l).to_string(),
        t_apply: rust_i18n::t!("catalog.apply", locale = l).to_string(),
        t_retire: rust_i18n::t!("containers.retire", locale = l).to_string(),
        t_unretire: rust_i18n::t!("containers.unretire", locale = l).to_string(),
        rows,
        models: 選べる型番(&state.db).await?,
        can_edit,
        show_retired,
        form,
        error,
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct ContainerForm {
    #[serde(default)]
    pub name: String,
    /// 型番（`CONTAINER_MODEL.id`、#205）。種別と収容能力は型番が持つ。
    #[serde(default)]
    pub container_model_id: String,
    /// 設置場所（自由記述、#206）。
    #[serde(default)]
    pub installation_site: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<ContainerForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let l = Locale::parse(&current.user.locale).as_str();
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    if form.name.trim().is_empty() {
        let e = 誤り("containers.error_name");
        return 一覧を描く(&state, &current, project_id, false, form, e).await;
    }
    // **型番は必須。廃番は選べない**（12.10、18.5）。画面は候補を絞るが、POSTは直接叩ける
    let model = match form.container_model_id.trim().parse::<i32>() {
        Ok(id) => container_model::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .filter(|m| m.retired_at.is_none()),
        Err(_) => None,
    };
    let Some(model) = model else {
        let e = 誤り("containers.error_model");
        return 一覧を描く(&state, &current, project_id, false, form, e).await;
    };
    // **名前はプロジェクトの中で一意、大文字小文字を区別しない**（12.10、#204）。
    // DBの一意インデックスでも止まるが、500ではなく入力に戻す
    if 設備::同じ名前の設備(&state.db, PROJECT, project_id, &form.name, None)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some()
    {
        let e = 誤り("containers.error_name_taken");
        return 一覧を描く(&state, &current, project_id, false, form, e).await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(mount_container::ActiveModel {
        name: Set(form.name.trim().to_owned()),
        container_model_id: Set(Some(model.id)),
        location_type: Set(PROJECT.to_owned()),
        location_id: Set(project_id),
        installation_site: Set({
            let site = crate::server::catalog::正規化(&form.installation_site);
            (!site.is_empty()).then_some(site)
        }),
        created_by: Set(current.user.id),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/containers")).into_response())
}

// ---------------------------------------------------------------------------
// ラック図
// ---------------------------------------------------------------------------

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    図を描く(&state, &current, project_id, container_id, None, None).await
}

/// 搭載中の1台ぶんの情報。図と一覧の両方で使う。
struct 搭載 {
    mount: device_mount::Model,
    device: device::Model,
    height_u: i32,
    mount_form: String,
}

async fn 図を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    container_id: i32,
    error: Option<String>,
    notice: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let container = 什器(state, project_id, container_id).await?;

    let 搭載一覧 = 搭載を引く(&state.db, container_id).await?;
    let 棚上 = 棚の上の機器(&state.db, &搭載一覧).await?;
    let 購入 =
        crate::server::device::購入の記録(&state.db, MOUNT_CONTAINER_ITEM, container_id).await?;

    // 種別と収容能力は型番から引く（#205）
    let 型 = 設備::型を引く(&state.db, &container)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let 格子 = 型.container_type != DESK;
    let capacity = 型.capacity.unwrap_or(0);
    let 重さ = 重量を集計する(&state.db, &搭載一覧).await?;

    let mut side_items = Vec::new();
    let mut out_of_range = Vec::new();
    let mut loose = Vec::new();
    let mut front = Vec::new();
    let mut rear = Vec::new();

    for m in &搭載一覧 {
        let 名 = 表示名(m);
        let planned = m.device.status == DEVICE_PLANNED;

        // **0UサイドマウントはU数を消費しない**（12.3）。格子の外に置く（12.9）
        if m.mount_form == RACK_SIDE {
            side_items.push(SideItem {
                label: 名, planned
            });
            continue;
        }

        let Some(position) = m.mount.position.filter(|_| 格子) else {
            // 格子を持たない什器、または位置未設定。並べるだけ（12.9）
            loose.push(SideItem {
                label: 名, planned
            });
            continue;
        };

        // **収容能力を超えていても隠さない**（不変条件6）。描いたうえで警告する
        if position < 1 || position + m.height_u - 1 > capacity {
            out_of_range.push(SideItem {
                label: format!("{名}（{position}U）"),
                planned,
            });
            continue;
        }

        let cell = 箱を作る(m, position, capacity, &名, planned, &棚上);
        match m.mount.depth_position.as_deref() {
            // 前面図・背面図のどちらか一方だけに現れる（12.9）
            Some(FRONT) => front.push(cell),
            Some(REAR) => rear.push(cell),
            // Full と未設定は両方に現れる
            _ => {
                front.push(箱を作る(m, position, capacity, &名, planned, &棚上));
                rear.push(cell);
            }
        }
    }

    let ticks = (1..=capacity)
        .map(|u| Tick {
            // 下が1U（12.9）。描画時に一度だけ上下を入れ替える
            y: (capacity - u) * U高 + 見出し高,
            label: u.to_string(),
        })
        .collect();

    // Rack だけ前面・背面を分ける。Shelving に前後の区別は無い（12.9）
    let 前後を分ける = 型.container_type == RACK;
    let mut panels = vec![Panel {
        x: 0,
        frame_x: 番号欄,
        right_x: 番号欄 + 枠幅,
        frame_w: 枠幅,
        title: if 前後を分ける {
            rust_i18n::t!("rack.front", locale = l).to_string()
        } else {
            container.name.clone()
        },
        cells: front,
    }];
    if 前後を分ける {
        let x = 番号欄 + 枠幅 + 図間;
        panels.push(Panel {
            x,
            frame_x: x + 番号欄,
            right_x: x + 番号欄 + 枠幅,
            frame_w: 枠幅,
            title: rust_i18n::t!("rack.rear", locale = l).to_string(),
            cells: rear,
        });
    }

    let grid_height = capacity * U高;
    let svg_width = if 前後を分ける {
        (番号欄 + 枠幅) * 2 + 図間
    } else {
        番号欄 + 枠幅
    };

    render(&RackPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "containers",
        )
        .await,
        project_id,
        container_id,
        container_name: container.name.clone(),
        container_type: format!(
            "{}（{}）",
            型番の表示(&state.db, 型.model.as_ref()).await?,
            型.container_type
        ),
        load: 荷重の表示(&重さ, 型.model.as_ref().and_then(|m| m.static_load_g), l),
        overloaded: 型
            .model
            .as_ref()
            .and_then(|m| m.static_load_g)
            .is_some_and(|limit| 重さ.total_g > i64::from(limit)),
        t_load: rust_i18n::t!("containers.load", locale = l).to_string(),
        t_overloaded: rust_i18n::t!("containers.overloaded", locale = l).to_string(),
        project_name: project.name,
        t_back: rust_i18n::t!("containers.back", locale = l).to_string(),
        t_diagram: rust_i18n::t!("rack.diagram", locale = l).to_string(),
        t_legend_planned: rust_i18n::t!("rack.legend_planned", locale = l).to_string(),
        t_legend_shelf: rust_i18n::t!("rack.legend_shelf", locale = l).to_string(),
        t_side: rust_i18n::t!("rack.side", locale = l).to_string(),
        t_side_hint: rust_i18n::t!("rack.side_hint", locale = l).to_string(),
        t_out_of_range: rust_i18n::t!("rack.out_of_range", locale = l).to_string(),
        t_mount: rust_i18n::t!("rack.mount", locale = l).to_string(),
        t_device: rust_i18n::t!("rack.device", locale = l).to_string(),
        t_position: rust_i18n::t!("rack.position", locale = l).to_string(),
        t_position_hint: rust_i18n::t!("rack.position_hint", locale = l).to_string(),
        t_horizontal: rust_i18n::t!("rack.horizontal", locale = l).to_string(),
        t_depth: rust_i18n::t!("rack.depth", locale = l).to_string(),
        t_host: rust_i18n::t!("rack.host", locale = l).to_string(),
        t_host_hint: rust_i18n::t!("rack.host_hint", locale = l).to_string(),
        t_unset: rust_i18n::t!("work_orders.unset", locale = l).to_string(),
        t_submit: rust_i18n::t!("rack.submit", locale = l).to_string(),
        t_mounted: rust_i18n::t!("rack.mounted", locale = l).to_string(),
        t_unmount: rust_i18n::t!("rack.unmount", locale = l).to_string(),
        t_empty: rust_i18n::t!("rack.empty", locale = l).to_string(),
        t_no_grid: rust_i18n::t!("rack.no_grid", locale = l).to_string(),
        retired: container.retired_at.is_some(),
        t_retired_note: rust_i18n::t!("containers.retired_note", locale = l).to_string(),
        svg_width,
        svg_height: grid_height + 見出し高,
        grid_top: 見出し高,
        grid_height,
        panels,
        ticks,
        side_items,
        out_of_range,
        loose,
        has_grid: 格子 && capacity > 0,
        devices: 搭載候補(state, project_id, &搭載一覧).await?,
        hosts: 搭載一覧
            .iter()
            .map(|m| Labeled {
                label: m.device.hostname.clone(),
                value: m.device.id.to_string(),
            })
            .collect(),
        mounted: 搭載一覧.iter().map(搭載行).collect(),
        horizontals: vec![LEFT, RIGHT, "Full"],
        depths: vec![FRONT, REAR, "Full"],
        installation_site: container.installation_site.clone().unwrap_or_default(),
        t_site: rust_i18n::t!("containers.installation_site", locale = l).to_string(),
        t_site_hint: rust_i18n::t!("containers.installation_site_hint", locale = l).to_string(),
        t_save: rust_i18n::t!("common.save", locale = l).to_string(),
        circuits: 回路の行(&state.db, container_id, state.タイムゾーン(&current.user)).await?,
        phases: PHASES.to_vec(),
        power: {
            let 電力 = crate::server::power::什器の電力(&state.db, container_id).await?;
            let mut v = rust_i18n::t!(
                "containers.power_total",
                watt = 電力.watt,
                with_plan = 電力.watt_with_plan,
                locale = l
            )
            .to_string();
            // **外した台数を黙らせない**（12.5）
            if 電力.stored > 0 {
                v.push_str(&rust_i18n::t!(
                    "containers.power_stored",
                    count = 電力.stored,
                    locale = l
                ));
            }
            if 電力.standby > 0 {
                v.push_str(&rust_i18n::t!(
                    "containers.power_standby",
                    count = 電力.standby,
                    locale = l
                ));
            }
            v
        },
        t_power: rust_i18n::t!("containers.power", locale = l).to_string(),
        t_circuits: rust_i18n::t!("containers.circuits", locale = l).to_string(),
        t_circuits_hint: rust_i18n::t!("containers.circuits_hint", locale = l).to_string(),
        t_circuit_label: rust_i18n::t!("containers.circuit_label", locale = l).to_string(),
        t_voltage: rust_i18n::t!("containers.voltage", locale = l).to_string(),
        t_phase: rust_i18n::t!("containers.phase", locale = l).to_string(),
        t_breaker: rust_i18n::t!("containers.breaker", locale = l).to_string(),
        t_continuous: rust_i18n::t!("containers.continuous", locale = l).to_string(),
        t_connector: rust_i18n::t!("containers.connector", locale = l).to_string(),
        t_since: rust_i18n::t!("containers.since", locale = l).to_string(),
        t_end_circuit: rust_i18n::t!("containers.end_circuit", locale = l).to_string(),
        t_add_circuit: rust_i18n::t!("containers.add_circuit", locale = l).to_string(),
        t_circuit_form_hint: rust_i18n::t!("containers.circuit_form_hint", locale = l).to_string(),
        t_no_circuits: rust_i18n::t!("containers.no_circuits", locale = l).to_string(),
        purchase: crate::server::device::購入の表示(&購入, &project.currency),
        purchase_form: {
            // 説明は機器向けの文言なので、設備・什器向けに差し替える
            let mut f = crate::server::device::購入の入力欄(&購入, &project.currency, l);
            f.t_hint = rust_i18n::t!("containers.purchase_hint", locale = l).to_string();
            f.t_order_number_hint =
                rust_i18n::t!("containers.order_number_hint", locale = l).to_string();
            f
        },
        purchase_action: format!("/projects/{project_id}/containers/{container_id}/purchase"),
        cost_rows: 費用の行(&state.db, container_id, &project.currency, l).await?,
        t_costs: rust_i18n::t!("containers.costs", locale = l).to_string(),
        t_costs_hint: rust_i18n::t!("containers.costs_hint", locale = l).to_string(),
        t_to_assets: rust_i18n::t!("containers.to_assets", locale = l).to_string(),
        t_to_recurring: rust_i18n::t!("containers.to_recurring", locale = l).to_string(),
        can_edit,
        error,
        notice,
    })
}

/// 1台ぶんの箱を組み立てる。**上下の反転はここでだけ行う**（設計書12.9）。
fn 箱を作る(
    m: &搭載,
    position: i32,
    capacity: i32,
    名: &str,
    planned: bool,
    棚上: &[(i32, String)],
) -> Cell {
    // 半width は幅を半分ずつ使う。Full と未設定は全幅（12.9）
    let (x, w) = match m.mount.horizontal_position.as_deref() {
        Some(LEFT) => (番号欄, 枠幅 / 2),
        Some(RIGHT) => (番号欄 + 枠幅 / 2, 枠幅 / 2),
        _ => (番号欄, 枠幅),
    };

    Cell {
        x,
        // position は開始U。占めるのは position 〜 position+height_u-1
        y: (capacity - (position + m.height_u - 1)) * U高 + 見出し高,
        w,
        h: m.height_u * U高,
        label: format!("{名}  {position}U"),
        planned,
        // **棚板の上の機器は棚板の帯の中に描く**（12.9）
        shelf_label: 棚上
            .iter()
            .filter(|(host, _)| *host == m.device.id)
            .map(|(_, name)| format!("▸ {name}"))
            .collect::<Vec<_>>()
            .join("  "),
    }
}

fn 表示名(m: &搭載) -> String {
    m.device.hostname.clone()
}

fn 搭載行(m: &搭載) -> MountedRow {
    let mut place = Vec::new();
    if let Some(p) = m.mount.position {
        place.push(format!("{p}U"));
    }
    if let Some(h) = &m.mount.horizontal_position {
        place.push(h.clone());
    }
    if let Some(d) = &m.mount.depth_position {
        place.push(d.clone());
    }
    if m.mount_form == RACK_SIDE {
        place.push(RACK_SIDE.to_owned());
    }
    MountedRow {
        mount_id: m.mount.id,
        device: m.device.hostname.clone(),
        place: place.join(" / "),
    }
}

// ---------------------------------------------------------------------------
// 搭載・解除
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MountForm {
    pub device_id: i32,
    #[serde(default)]
    pub position: String,
    #[serde(default)]
    pub horizontal_position: String,
    #[serde(default)]
    pub depth_position: String,
    #[serde(default)]
    pub host_device_id: String,
}

pub async fn mount(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
    Form(form): Form<MountForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let l = Locale::parse(&current.user.locale).as_str();
    let 訳 = |key: &str| rust_i18n::t!(key, locale = l).to_string();

    let 結果 = 搭載を試みる(
        &state,
        current.user.id,
        搭載要求 {
            project_id,
            container_id,
            device_id: form.device_id,
            position: &form.position,
            horizontal: &form.horizontal_position,
            depth: &form.depth_position,
            host_device_id: &form.host_device_id,
            // 直接の登録。変更管理チケット経由の予約は [`crate::server::work_order`]
            work_order_id: None,
        },
    )
    .await?;

    match 結果 {
        Err(key) => {
            図を描く(
                &state,
                &current,
                project_id,
                container_id,
                Some(訳(key)),
                None,
            )
            .await
        }
        Ok(warnings) if !warnings.is_empty() => {
            // **警告はリダイレクトで消さない。**見落とすと誤配置が残る
            let notice = warnings.iter().map(|k| 訳(k)).collect::<Vec<_>>().join(" ");
            図を描く(
                &state,
                &current,
                project_id,
                container_id,
                None,
                Some(notice),
            )
            .await
        }
        Ok(_) => Ok(
            Redirect::to(&format!("/projects/{project_id}/containers/{container_id}"))
                .into_response(),
        ),
    }
}

/// 搭載の依頼。**直接の登録と、変更管理チケットからの予約で同じ経路を通す**
/// （設計書11.6）。検証を二重に持つと、片方だけ直したときに規則が食い違う。
pub(crate) struct 搭載要求<'a> {
    pub project_id: i32,
    pub container_id: i32,
    pub device_id: i32,
    pub position: &'a str,
    pub horizontal: &'a str,
    pub depth: &'a str,
    pub host_device_id: &'a str,
    /// 予約として作る場合のWORK_ORDER（11.6）。直接の登録では `None`。
    pub work_order_id: Option<i32>,
}

/// 12.3の業務ルールを検証し、通れば `DEVICE_MOUNT` を開く。
///
/// **返すのは i18n のキー**であり、文言の組み立ては呼び出し側が行う。予約と
/// 直接登録で画面が違うため、ここでレスポンスまで作らない。
///
/// `Err` は登録を行わなかった場合、`Ok` は行った場合で、中身は警告のキー。
/// **警告があっても登録は通す**（不変条件6、12.3）。
pub(crate) async fn 搭載を試みる(
    state: &AppState,
    actor_id: i32,
    req: 搭載要求<'_>,
) -> AppResult<Result<Vec<&'static str>, &'static str>> {
    let container = 什器(state, req.project_id, req.container_id).await?;
    // **撤去した設備には載せない**（12.10）。予約（変更管理チケット）もこの経路を通る
    if container.retired_at.is_some() {
        return Ok(Err("rack.error_retired"));
    }

    let 対象 = device::Entity::find_by_id(req.device_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    // **このプロジェクトの機器だけを搭載できる。**画面は候補を絞るが、
    // POSTは直接叩ける
    if !このプロジェクトにいる(&state.db, req.project_id, 対象.id).await? {
        return Ok(Err("rack.error_not_in_project"));
    }

    // 既に現行の搭載行があるなら二重に開かない。移設は閉じてから
    if 現在の搭載(&state.db, 対象.id).await?.is_some() {
        return Ok(Err("rack.error_already_mounted"));
    }

    let (height_u, mount_form, rack_width) = 型情報(&state.db, &対象).await?;
    let host_device_id = req.host_device_id.trim().parse::<i32>().ok();

    let position = match req.position.trim() {
        "" => None,
        v => match v.parse::<i32>() {
            Ok(n) if n > 0 => Some(n),
            // **黙って未設定に落とさない**（Q-21）
            _ => return Ok(Err("rack.error_position")),
        },
    };

    let horizontal = 語彙(req.horizontal, &[LEFT, RIGHT, "Full"]);
    let depth = 語彙(req.depth, &[FRONT, REAR, "Full"]);

    // **0Uサイドマウントは位置を持たない**（12.3）
    if mount_form == RACK_SIDE && (position.is_some() || horizontal.is_some() || depth.is_some()) {
        return Ok(Err("rack.error_rack_side"));
    }

    // **棚板の上に載る機器は container_id / position を使わない**（12.3）
    if host_device_id.is_some() && position.is_some() {
        return Ok(Err("rack.error_host_and_position"));
    }

    // **半width同居は rack_width="Half" のときだけ**（12.3）
    if matches!(horizontal.as_deref(), Some(LEFT) | Some(RIGHT))
        && rack_width.as_deref() != Some(HALF)
    {
        return Ok(Err("rack.error_half_width"));
    }

    let mut warnings: Vec<&'static str> = Vec::new();

    if let Some(p) = position {
        match 重複を調べる(
            &state.db,
            req.container_id,
            p,
            height_u,
            horizontal.as_deref(),
            depth.as_deref(),
        )
        .await?
        {
            重複::衝突 => return Ok(Err("rack.error_occupied")),
            // **排熱上は推奨しないが実在する。**許可したうえで注意喚起（12.3）
            重複::前後同居 => warnings.push("rack.warn_front_rear"),
            重複::なし => {}
        }

        // **超過はエラーではなく警告**（不変条件6、12.3）
        let 型 = 設備::型を引く(&state.db, &container)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 型.capacity.is_some_and(|cap| p + height_u - 1 > cap) {
            warnings.push("rack.warn_capacity");
        }
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(actor_id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(device_mount::ActiveModel {
        device_id: Set(対象.id),
        // 棚板の上に載る場合は什器に直接紐づけない（12.3）
        container_id: Set(host_device_id.is_none().then_some(req.container_id)),
        position: Set(position),
        horizontal_position: Set(horizontal),
        depth_position: Set(depth),
        host_device_id: Set(host_device_id),
        work_order_id: Set(req.work_order_id),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Ok(warnings))
}

#[derive(Debug, Deserialize)]
pub struct UnmountForm {
    pub mount_id: i32,
}

pub async fn unmount(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
    Form(form): Form<UnmountForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;
    什器(&state, project_id, container_id).await?;

    let row = device_mount::Entity::find_by_id(form.mount_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|r| r.to_date.is_none())
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    // **行は消さず閉じる**（不変条件1）。いつまでそこにあったかが事実である
    tx.update(
        &row,
        device_mount::ActiveModel {
            id: Set(row.id),
            to_date: Set(Some(Utc::now())),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/containers/{container_id}")).into_response())
}

// ---------------------------------------------------------------------------
// 重複配置の検証（設計書12.3）
// ---------------------------------------------------------------------------

enum 重複 {
    なし,
    /// 前後で同居している。**許可したうえで注意喚起する**（12.3）。
    前後同居,
    衝突,
}

/// 同じUに置けるのは、左右の組か前後の組のときだけ（設計書12.3）。
///
/// **両方の占有範囲を突き合わせる。**当初は既存側の開始Uだけを見ていたが、
/// それでは「10Uに2Uの機器がある状態で11Uに置く」を見逃す。既存側の高さは
/// `CHASSIS_MODEL` から引けるので、引かない理由がない。
async fn 重複を調べる<C: ConnectionTrait>(
    db: &C,
    container_id: i32,
    position: i32,
    height_u: i32,
    horizontal: Option<&str>,
    depth: Option<&str>,
) -> AppResult<重複> {
    let 現行 = device_mount::Entity::find()
        .filter(device_mount::Column::ContainerId.eq(container_id))
        .filter(device_mount::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let 上端 = position + height_u - 1;
    let mut result = 重複::なし;

    for r in 現行 {
        let Some(p) = r.position else { continue };

        // 既存側の高さも引く。**開始Uだけで見ると跨ぎを見逃す**
        let 相手の高さ = match device::Entity::find_by_id(r.device_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            Some(d) => 型情報(db, &d).await?.0,
            None => continue,
        };

        // 区間が重なるか
        if 上端 < p || position > p + 相手の高さ - 1 {
            continue;
        }

        let 左右で分かれている = matches!(
            (horizontal, r.horizontal_position.as_deref()),
            (Some(LEFT), Some(RIGHT)) | (Some(RIGHT), Some(LEFT))
        );
        let 前後で分かれている = matches!(
            (depth, r.depth_position.as_deref()),
            (Some(FRONT), Some(REAR)) | (Some(REAR), Some(FRONT))
        );

        if 左右で分かれている {
            continue;
        }
        if 前後で分かれている {
            result = 重複::前後同居;
            continue;
        }
        return Ok(重複::衝突);
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 搭載を引く<C: ConnectionTrait>(db: &C, container_id: i32) -> AppResult<Vec<搭載>> {
    let mounts = device_mount::Entity::find()
        .filter(device_mount::Column::ContainerId.eq(container_id))
        .filter(device_mount::Column::ToDate.is_null())
        .order_by_asc(device_mount::Column::Position)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for m in mounts {
        let Some(d) = device::Entity::find_by_id(m.device_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            continue;
        };
        let (height_u, mount_form, _) = 型情報(db, &d).await?;
        out.push(搭載 {
            mount: m,
            device: d,
            height_u,
            mount_form,
        });
    }
    Ok(out)
}

/// 棚板の上に載っている機器（設計書12.3）。`(host_device_id, hostname)` を返す。
async fn 棚の上の機器<C: ConnectionTrait>(
    db: &C,
    搭載一覧: &[搭載],
) -> AppResult<Vec<(i32, String)>> {
    let hosts: Vec<i32> = 搭載一覧.iter().map(|m| m.device.id).collect();
    if hosts.is_empty() {
        return Ok(Vec::new());
    }

    let mounts = device_mount::Entity::find()
        .filter(device_mount::Column::HostDeviceId.is_in(hosts))
        .filter(device_mount::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for m in mounts {
        let Some(host) = m.host_device_id else {
            continue;
        };
        if let Some(d) = device::Entity::find_by_id(m.device_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            // **ラック図は物理の図。**VM・コンテナは描かない（12.9）
            if d.device_type == "Physical" {
                out.push((host, d.hostname));
            }
        }
    }
    Ok(out)
}

/// 登録で選べる型番（廃番を除く）。表示は「ベンダー 型番（種別）」。
async fn 選べる型番<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    let models = container_model::Entity::find()
        .filter(container_model::Column::RetiredAt.is_null())
        .order_by_asc(container_model::Column::ModelName)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mut out = Vec::new();
    for m in models {
        out.push(Labeled {
            label: format!(
                "{}（{}）",
                型番の表示(db, Some(&m)).await?,
                m.container_type
            ),
            value: m.id.to_string(),
        });
    }
    Ok(out)
}

/// 「ベンダー 型番」。型番が無ければ空。
async fn 型番の表示<C: ConnectionTrait>(
    db: &C,
    m: Option<&container_model::Model>,
) -> AppResult<String> {
    let Some(m) = m else {
        return Ok(String::new());
    };
    let v = vendor::Entity::find_by_id(m.vendor_id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map(|v| v.name)
        .unwrap_or_default();
    Ok(format!("{v} {}", m.model_name).trim().to_owned())
}

/// 載せた物理機器の重量の合計（#205）。棚板の上の機器も含める。
struct 重量 {
    total_g: i64,
    /// 重量が未入力（機種に `weight_g` が無い）で、合計から外した台数。
    unknown: usize,
}

async fn 重量を集計する<C: ConnectionTrait>(
    db: &C, 搭載一覧: &[搭載]
) -> AppResult<重量> {
    let mut 対象: Vec<device::Model> = 搭載一覧.iter().map(|m| m.device.clone()).collect();

    // 棚板の上の機器（12.3）。ラックの荷重になる
    let hosts: Vec<i32> = 搭載一覧.iter().map(|m| m.device.id).collect();
    if !hosts.is_empty() {
        for m in device_mount::Entity::find()
            .filter(device_mount::Column::HostDeviceId.is_in(hosts))
            .filter(device_mount::Column::ToDate.is_null())
            .all(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            if let Some(d) = device::Entity::find_by_id(m.device_id)
                .one(db)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            {
                対象.push(d);
            }
        }
    }

    let mut total_g = 0i64;
    let mut unknown = 0;
    for d in 対象 {
        // **重さを持つのは物理機器だけ。**VM・コンテナは数えない（12.5と同じ）
        if d.device_type != "Physical" {
            continue;
        }
        match 機器の重量(db, &d).await? {
            Some(g) => total_g += i64::from(g),
            None => unknown += 1,
        }
    }
    Ok(重量 { total_g, unknown })
}

async fn 機器の重量<C: ConnectionTrait>(db: &C, d: &device::Model) -> AppResult<Option<i32>> {
    let Some(cid) = d.configuration_id else {
        return Ok(None);
    };
    let Some(c) = configuration::Entity::find_by_id(cid)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok(None);
    };
    Ok(chassis_model::Entity::find_by_id(c.chassis_model_id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .and_then(|m| m.weight_g))
}

/// 「載せた重量 123.4kg / 静荷重 1,000.0kg（重量が未入力の機器 2台）」。
/// **未入力の台数を必ず出す**——合計が小さいのが実態なのか未入力なのかを
/// 判別できるようにする（12.7の「分母を出す」と同じ）。
fn 荷重の表示(重さ: &重量, static_load_g: Option<i32>, l: &str) -> String {
    let kg = |g: i64| format!("{:.1}kg", g as f64 / 1000.0);
    let mut s = kg(重さ.total_g);
    if let Some(limit) = static_load_g {
        s.push_str(&format!(" / {}", kg(i64::from(limit))));
    }
    if 重さ.unknown > 0 {
        s.push_str(
            rust_i18n::t!("containers.load_unknown", count = 重さ.unknown, locale = l).as_ref(),
        );
    }
    s
}

/// `CHASSIS_MODEL` から高さ・搭載形態・幅を引く。
///
/// **`configuration_id` を持たない機器（仮想アプライアンス等）は1U・全幅として
/// 扱う。**型が分からないものを描かないより、既定で描いて人に見せるほうがよい。
async fn 型情報<C: ConnectionTrait>(
    db: &C,
    d: &device::Model,
) -> AppResult<(i32, String, Option<String>)> {
    let Some(cid) = d.configuration_id else {
        return Ok((1, "RackU".to_owned(), None));
    };

    let Some(c) = configuration::Entity::find_by_id(cid)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok((1, "RackU".to_owned(), None));
    };

    let Some(m) = chassis_model::Entity::find_by_id(c.chassis_model_id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok((1, "RackU".to_owned(), None));
    };

    Ok((Ord::max(m.height_u, 1), m.mount_form, m.rack_width))
}

async fn 現在の搭載<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Option<device_mount::Model>> {
    device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(device_id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn このプロジェクトにいる<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    device_id: i32,
) -> AppResult<bool> {
    Ok(device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some())
}

/// 搭載できる機器。**既にどこかに載っているものは出さない。**
async fn 搭載候補(
    state: &AppState,
    project_id: i32,
    搭載一覧: &[搭載],
) -> AppResult<Vec<Labeled>> {
    let 現在ここにいる = SeaQuery::select()
        .column(device_assignment::Column::DeviceId)
        .from(device_assignment::Entity)
        .and_where(Expr::col(device_assignment::Column::LocationType).eq(PROJECT))
        .and_where(Expr::col(device_assignment::Column::LocationId).eq(project_id))
        .and_where(Expr::col(device_assignment::Column::ToDate).is_null())
        .to_owned();

    let 搭載済み = SeaQuery::select()
        .column(device_mount::Column::DeviceId)
        .from(device_mount::Entity)
        .and_where(Expr::col(device_mount::Column::ToDate).is_null())
        .to_owned();

    let _ = 搭載一覧;

    Ok(device::Entity::find()
        .filter(device::Column::Id.in_subquery(現在ここにいる))
        .filter(device::Column::Id.not_in_subquery(搭載済み))
        .filter(device::Column::MergedIntoDeviceId.is_null())
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

/// 語彙に無い値は `None` にせず、**呼び出し側で拒否できるよう空を返す**。
fn 語彙(value: &str, allowed: &[&str]) -> Option<String> {
    let v = value.trim();
    if v.is_empty() || !allowed.contains(&v) {
        return None;
    }
    Some(v.to_owned())
}

// ---------------------------------------------------------------------------
// 設置場所・給電・費用（#206、設計書12.10）
// ---------------------------------------------------------------------------

/// `PURCHASE` / `FIXED_ASSET` / `RECURRING_COST` が設備・什器を指すときの `item_type`。
const MOUNT_CONTAINER_ITEM: &str = "MountContainer";

use crate::container::{PHASES, SINGLE};

/// 連続負荷として使えるのはブレーカー定格の80%まで（12.7）。**係数は保存しない**
/// （不変条件2）。
const 連続負荷の率: f64 = 0.8;

struct CircuitRow {
    id: i32,
    circuit_label: String,
    voltage: i32,
    phase: String,
    breaker: String,
    continuous: String,
    connector_type: String,
    since: String,
}

async fn 回路の行<C: ConnectionTrait>(
    db: &C,
    container_id: i32,
    tz: chrono_tz::Tz,
) -> AppResult<Vec<CircuitRow>> {
    Ok(現在の回路(db, container_id)
        .await?
        .into_iter()
        .map(|c| {
            let a = f64::from(c.breaker_current_ma) / 1000.0;
            // 単相だけ VA を出す。**三相は式が違い、v2で扱う**（12.7）
            let continuous = if c.phase == SINGLE {
                format!(
                    "{:.1}A / {:.0}VA",
                    a * 連続負荷の率,
                    f64::from(c.voltage) * a * 連続負荷の率
                )
            } else {
                format!("{:.1}A", a * 連続負荷の率)
            };
            CircuitRow {
                id: c.id,
                breaker: format!("{a:.1}A"),
                continuous,
                connector_type: c.connector_type.unwrap_or_default(),
                since: crate::tz::日付(c.from_date, tz).to_string(),
                circuit_label: c.circuit_label,
                voltage: c.voltage,
                phase: c.phase,
            }
        })
        .collect())
}

async fn 現在の回路<C: ConnectionTrait>(
    db: &C,
    container_id: i32,
) -> AppResult<Vec<power_circuit::Model>> {
    設備::現在の回路(db, container_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// この設備・什器の固定資産と継続費用。**合算は年間コスト（10.3）が行う**。
async fn 費用の行<C: ConnectionTrait>(
    db: &C,
    container_id: i32,
    通貨: &str,
    l: &str,
) -> AppResult<Vec<Labeled>> {
    let mut out = Vec::new();
    for a in fixed_asset::Entity::find()
        .filter(fixed_asset::Column::ItemType.eq(MOUNT_CONTAINER_ITEM))
        .filter(fixed_asset::Column::ItemId.eq(container_id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        out.push(Labeled {
            label: rust_i18n::t!("costs.fixed_assets", locale = l).to_string(),
            value: format!(
                "{} {通貨}（{}、{}年）",
                crate::currency::表示(a.acquisition_cost, 通貨),
                a.acquisition_date,
                a.useful_life_years
            ),
        });
    }
    for r in recurring_cost::Entity::find()
        .filter(recurring_cost::Column::ItemType.eq(MOUNT_CONTAINER_ITEM))
        .filter(recurring_cost::Column::ItemId.eq(container_id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        out.push(Labeled {
            label: rust_i18n::t!("costs.recurring", locale = l).to_string(),
            value: format!(
                "{}：{} {通貨} / {}（{}〜{}）",
                r.cost_type,
                crate::currency::表示(r.amount, 通貨),
                r.billing_cycle,
                r.start_date,
                r.end_date.map(|d| d.to_string()).unwrap_or_default()
            ),
        });
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct SiteForm {
    #[serde(default)]
    pub installation_site: String,
}

/// 設置場所を保存する。**自由記述**（12.10）。空なら未設定に戻す。
pub async fn save_site(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
    Form(form): Form<SiteForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let c = 什器(&state, project_id, container_id).await?;
    let site = crate::server::catalog::正規化(&form.installation_site);

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &c,
        mount_container::ActiveModel {
            id: Set(container_id),
            installation_site: Set((!site.is_empty()).then_some(site)),
            updated_at: Set(Utc::now()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(図へ戻る(project_id, container_id))
}

#[derive(Debug, Default, Deserialize)]
pub struct CircuitForm {
    #[serde(default)]
    pub circuit_label: String,
    #[serde(default)]
    pub voltage: String,
    #[serde(default)]
    pub phase: String,
    /// A で受けて mA の整数で保存する（24.2.1）。
    #[serde(default)]
    pub breaker_current_a: String,
    #[serde(default)]
    pub connector_type: String,
}

impl CircuitForm {
    fn 検証(&self) -> Result<設備::回路の値, &'static str> {
        設備::回路を検証する(
            &self.circuit_label,
            &self.voltage,
            &self.phase,
            &self.breaker_current_a,
            &self.connector_type,
        )
    }
}

/// 回路を足す、または変える。**同じ系統名の現在の回路があれば、閉じて開く**
/// （履歴、4章）。値が同じなら何もしない。
pub async fn save_circuit(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
    Form(form): Form<CircuitForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let c = 什器(&state, project_id, container_id).await?;
    if c.retired_at.is_some() {
        let e = Some(rust_i18n::t!("containers.retired_note", locale = l).to_string());
        return 図を描く(&state, &current, project_id, container_id, e, None).await;
    }
    let 値 = match form.検証() {
        Ok(v) => v,
        Err(key) => {
            let e = Some(rust_i18n::t!(key, locale = l).to_string());
            return 図を描く(&state, &current, project_id, container_id, e, None).await;
        }
    };

    // **読み取りは開く前に済ませる**（SQLiteで自分のロックを待たないため）
    let 現在 = 現在の回路(&state.db, container_id)
        .await?
        .into_iter()
        .find(|r| r.circuit_label == 値.circuit_label);

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    設備::回路を書く(&tx, container_id, 現在, 値, Utc::now())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(図へ戻る(project_id, container_id))
}

#[derive(Debug, Deserialize)]
pub struct EndCircuitForm {
    pub circuit_id: i32,
}

/// 回路を終える（撤去・解約）。**行は消さず閉じる**（不変条件1）。
pub async fn end_circuit(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
    Form(form): Form<EndCircuitForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    什器(&state, project_id, container_id).await?;
    let r = power_circuit::Entity::find_by_id(form.circuit_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|r| r.container_id == container_id && r.to_date.is_none())
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &r,
        power_circuit::ActiveModel {
            id: Set(r.id),
            to_date: Set(Some(Utc::now())),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(図へ戻る(project_id, container_id))
}

/// 購入の記録を保存する（10.2）。**機器と同じ検証と書き込みを通す**。
pub async fn save_purchase(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
    Form(form): Form<crate::server::device::PurchaseInput>,
) -> AppResult<Response> {
    let (project, _) = 入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    什器(&state, project_id, container_id).await?;

    let 値 = match form.検証(&project.currency) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(図へ戻る(project_id, container_id)),
        Err(key) => {
            let e = Some(rust_i18n::t!(key, locale = l).to_string());
            return 図を描く(&state, &current, project_id, container_id, e, None).await;
        }
    };
    let 既存 =
        crate::server::device::購入の記録(&state.db, MOUNT_CONTAINER_ITEM, container_id).await?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    crate::server::device::購入を書く(
        &tx,
        MOUNT_CONTAINER_ITEM,
        container_id,
        既存,
        値,
        Utc::now(),
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(図へ戻る(project_id, container_id))
}

fn 図へ戻る(project_id: i32, container_id: i32) -> Response {
    Redirect::to(&format!("/projects/{project_id}/containers/{container_id}")).into_response()
}

// ---------------------------------------------------------------------------
// 撤去（#204、設計書12.10）
// ---------------------------------------------------------------------------

/// 撤去の確認画面。
pub async fn retire_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    let (project, _) = 入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let c = 什器(&state, project_id, container_id).await?;
    let 結末 = 設備::撤去の判定(&state.db, container_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let lead = match 結末 {
        撤去の結末::使用中 => "containers.retire_in_use",
        撤去の結末::物理削除 => "containers.retire_hard",
        撤去の結末::撤去 => "containers.retire_soft",
    };
    render(&ContainerRetirePage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "containers",
        )
        .await,
        project_id,
        project_name: project.name,
        container_id,
        name: c.name,
        t_title: rust_i18n::t!("containers.retire_title", locale = l).to_string(),
        t_back: rust_i18n::t!("containers.back", locale = l).to_string(),
        t_lead: rust_i18n::t!(lead, locale = l).to_string(),
        t_submit: rust_i18n::t!("containers.retire", locale = l).to_string(),
        can_retire: 結末 != 撤去の結末::使用中,
    })
}

/// 撤去を実行する。**載っていれば拒否し、使ったことが無ければ行ごと消し、
/// 使ったことがあれば `retired_at` を立てる**（12.10）。
pub async fn retire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let c = 什器(&state, project_id, container_id).await?;

    let 結末 = 設備::撤去の判定(&state.db, container_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 結末 == 撤去の結末::使用中 {
        let e = Some(rust_i18n::t!("containers.error_in_use", locale = l).to_string());
        return 一覧を描く(
            &state,
            &current,
            project_id,
            false,
            ContainerForm::default(),
            e,
        )
        .await;
    }

    // **読み取りは開く前に済ませてある**（SQLiteで自分のロックを待たないため）
    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    match 結末 {
        撤去の結末::物理削除 => tx
            .delete(c.clone())
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?,
        _ => {
            tx.update(
                &c,
                mount_container::ActiveModel {
                    id: Set(container_id),
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

    Ok(Redirect::to(&format!("/projects/{project_id}/containers")).into_response())
}

/// 撤去を取り消す。**同じ名前の設備が既にあれば取り消せない**——撤去の間に
/// 同じ名前で新しい設備が作られていると、名前の一意（12.10）が崩れる。
pub async fn unretire(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, container_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let c = 什器(&state, project_id, container_id).await?;

    if 設備::同じ名前の設備(&state.db, PROJECT, project_id, &c.name, Some(c.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some()
    {
        let e = Some(rust_i18n::t!("containers.error_unretire_taken", locale = l).to_string());
        return 一覧を描く(
            &state,
            &current,
            project_id,
            true,
            ContainerForm::default(),
            e,
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.update(
        &c,
        mount_container::ActiveModel {
            id: Set(container_id),
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

    Ok(Redirect::to(&format!("/projects/{project_id}/containers?retired=1")).into_response())
}

async fn 編集権(state: &AppState, current: &CurrentUser, project_id: i32) -> AppResult<()> {
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)
}

async fn 什器(
    state: &AppState,
    project_id: i32,
    container_id: i32,
) -> AppResult<mount_container::Model> {
    mount_container::Entity::find_by_id(container_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|c| c.location_type == PROJECT && c.location_id == project_id)
        .ok_or(AppError::NotFound)
}

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
