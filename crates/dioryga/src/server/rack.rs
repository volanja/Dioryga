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

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    chassis_model, configuration, device, device_assignment, device_mount, mount_container, project,
};
use sea_orm::sea_query::{Expr, Query as SeaQuery};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 語彙（`vocabularies.md`、設計書12.2）。
const CONTAINER_TYPES: &[&str] = &["Rack", "Desk", "Shelving"];
const RACK: &str = "Rack";
const DESK: &str = "Desk";

/// `MOUNT_CONTAINER.location_type` / `DEVICE_ASSIGNMENT.location_type`。
const PROJECT: &str = "Project";

/// 予約中の機器（設計書11.6）。
const PLAN: &str = "plan";

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
    container_type: String,
    capacity: String,
    mounted: usize,
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
    t_container_type: String,
    t_capacity: String,
    t_capacity_hint: String,
    t_mounted: String,
    t_actions: String,
    t_open: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    rows: Vec<ContainerRow>,
    container_types: Vec<&'static str>,
    can_edit: bool,
    error: Option<String>,
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

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    一覧を描く(&state, &current, project_id, None).await
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
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
    for c in containers {
        let mounted = device_mount::Entity::find()
            .filter(device_mount::Column::ContainerId.eq(c.id))
            .filter(device_mount::Column::ToDate.is_null())
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .len();

        rows.push(ContainerRow {
            id: c.id,
            name: c.name,
            container_type: c.container_type,
            capacity: c.capacity.map(|v| v.to_string()).unwrap_or_default(),
            mounted,
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
        t_container_type: rust_i18n::t!("containers.container_type", locale = l).to_string(),
        t_capacity: rust_i18n::t!("containers.capacity", locale = l).to_string(),
        t_capacity_hint: rust_i18n::t!("containers.capacity_hint", locale = l).to_string(),
        t_mounted: rust_i18n::t!("containers.mounted", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_open: rust_i18n::t!("containers.open", locale = l).to_string(),
        t_empty: rust_i18n::t!("containers.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("containers.new", locale = l).to_string(),
        t_submit: rust_i18n::t!("containers.submit", locale = l).to_string(),
        rows,
        container_types: CONTAINER_TYPES.to_vec(),
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct ContainerForm {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub container_type: String,
    #[serde(default)]
    pub capacity: String,
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
        return 一覧を描く(&state, &current, project_id, 誤り("containers.error_name")).await;
    }
    // **語彙外は既定へ寄せず拒否する**（Q-21）
    if !CONTAINER_TYPES.contains(&form.container_type.as_str()) {
        return 一覧を描く(&state, &current, project_id, 誤り("containers.error_type")).await;
    }

    let capacity = match form.capacity.trim() {
        // Desk は格子を持たない（12.3）。空でよい
        "" => None,
        v => match v.parse::<i32>() {
            Ok(n) if n > 0 => Some(n),
            _ => {
                return 一覧を描く(
                    &state,
                    &current,
                    project_id,
                    誤り("containers.error_capacity"),
                )
                .await;
            }
        },
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(mount_container::ActiveModel {
        name: Set(form.name.trim().to_owned()),
        container_type: Set(form.container_type.clone()),
        location_type: Set(PROJECT.to_owned()),
        location_id: Set(project_id),
        capacity: Set(capacity),
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

    let 格子 = container.container_type != DESK;
    let capacity = container.capacity.unwrap_or(0);

    let mut side_items = Vec::new();
    let mut out_of_range = Vec::new();
    let mut loose = Vec::new();
    let mut front = Vec::new();
    let mut rear = Vec::new();

    for m in &搭載一覧 {
        let 名 = 表示名(m);
        let planned = m.device.status == PLAN;

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
    let 前後を分ける = container.container_type == RACK;
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
        container_type: container.container_type.clone(),
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
        if container.capacity.is_some_and(|cap| p + height_u - 1 > cap) {
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
