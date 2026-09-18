//! 電力集計（設計書16.1のB領域、12.5〜12.7）。
//!
//! # v1は什器単位のみ
//!
//! `DEVICE_MOUNT` と `DEVICE.power_watt` だけで計算できる範囲に限る。
//! **回路単位はv2**——`CABLE_CONNECTION` に依存し、ケーブル取込がv2のため
//! データが入らない（16.7）。冗長電源の2つの負荷表示（12.6）と電流ベースの
//! 容量管理（12.7）も同じ理由でv2である。
//!
//! # 仮想マシンとコンテナは電力を引かない
//!
//! **ホストが引いている。**`DEVICE_MOUNT.host_device_id` でハイパーバイザの上に
//! 載っているVMを、物理サーバと同じように合算すると**二重に数える。**
//! 13章でVM・コンテナを `DEVICE` として扱えるようにした結果、この取り違えが
//! 起こりうるようになった。
//!
//! **`device_type` で判定する。**`Physical` だけを合算し、`Virtual` /
//! `Container` は数えない。棚板の上に載る物理機器（`host_device_id` が埋まる
//! もう一方の場合）は自分で電力を引くため、こちらは合算する。
//!
//! # 予約中の機器は分けて出す
//!
//! `status=plan` の機器は**まだ電力を引いていないが、引く予定である**（11.6）。
//! 現在値に混ぜると平常時の負荷を過大に見せ、外すと増設後の見通しが立たない。
//! **両方を並べて出す。**23.5で部分的な粒度の取込に「分母を表示する」とした
//! のと同じ考え方で、利用者が判別できる状態にする。

use axum::extract::{Path, State};
use axum::response::Response;
use axum::Extension;
use entity::{device, device_mount, mount_container, project};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder};

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

const PHYSICAL: &str = "Physical";
const PLAN: &str = "plan";
const PROJECT: &str = "Project";

struct ContainerRow {
    id: i32,
    name: String,
    container_type: String,
    /// 稼働中の物理機器の台数。
    running: usize,
    /// 稼働中の合計（W）。
    watt: i32,
    /// 予約中（`status=plan`）の台数。
    planned: usize,
    /// 予約を含めた合計（W）。
    watt_with_plan: i32,
    /// 電力を引かない仮想マシン・コンテナの台数。**数えていないことを示す。**
    virtual_count: usize,
}

#[derive(askama::Template)]
#[template(path = "power.html")]
struct PowerPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_v2_hint: String,
    t_virtual_hint: String,
    t_container: String,
    t_type: String,
    t_running: String,
    t_watt: String,
    t_planned: String,
    t_watt_with_plan: String,
    t_virtual: String,
    t_total: String,
    t_empty: String,
    t_detail: String,
    t_actions: String,
    rows: Vec<ContainerRow>,
    total_watt: i32,
    total_with_plan: i32,
}

pub async fn show(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    let project = project::Entity::find_by_id(project_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    authorization::require_project_member(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;
    let l = Locale::parse(&current.user.locale).as_str();

    let containers = mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .order_by_asc(mount_container::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    let mut total_watt = 0;
    let mut total_with_plan = 0;

    for c in containers {
        let 集計 = 什器の電力(&state.db, c.id).await?;
        total_watt += 集計.watt;
        total_with_plan += 集計.watt_with_plan;

        rows.push(ContainerRow {
            id: c.id,
            name: c.name,
            container_type: c.container_type,
            running: 集計.running,
            watt: 集計.watt,
            planned: 集計.planned,
            watt_with_plan: 集計.watt_with_plan,
            virtual_count: 集計.virtual_count,
        });
    }

    render(&PowerPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "power",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("power.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("power.lead", locale = l).to_string(),
        t_v2_hint: rust_i18n::t!("power.v2_hint", locale = l).to_string(),
        t_virtual_hint: rust_i18n::t!("power.virtual_hint", locale = l).to_string(),
        t_container: rust_i18n::t!("racks.container", locale = l).to_string(),
        t_type: rust_i18n::t!("racks.container_type", locale = l).to_string(),
        t_running: rust_i18n::t!("power.running", locale = l).to_string(),
        t_watt: rust_i18n::t!("power.watt", locale = l).to_string(),
        t_planned: rust_i18n::t!("power.planned", locale = l).to_string(),
        t_watt_with_plan: rust_i18n::t!("power.watt_with_plan", locale = l).to_string(),
        t_virtual: rust_i18n::t!("power.virtual", locale = l).to_string(),
        t_total: rust_i18n::t!("power.total", locale = l).to_string(),
        t_empty: rust_i18n::t!("racks.empty", locale = l).to_string(),
        t_detail: rust_i18n::t!("devices.detail", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        rows,
        total_watt,
        total_with_plan,
    })
}

#[derive(Default)]
struct 集計結果 {
    running: usize,
    watt: i32,
    planned: usize,
    watt_with_plan: i32,
    virtual_count: usize,
}

/// 什器に載っている機器の電力を合算する（12.5）。
///
/// **仮想マシンとコンテナは数えない。**ホストが引いているため、合算すると
/// 二重に数える。棚板の上の物理機器は自分で引くため数える。
async fn 什器の電力<C: ConnectionTrait>(db: &C, container_id: i32) -> AppResult<集計結果> {
    let mounts = device_mount::Entity::find()
        .filter(device_mount::Column::ContainerId.eq(container_id))
        .filter(device_mount::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = 集計結果::default();
    for m in mounts {
        let Some(d) = device::Entity::find_by_id(m.device_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            continue;
        };

        // **仮想マシン・コンテナは電力を引かない**（13章、12.5）
        if d.device_type != PHYSICAL {
            out.virtual_count += 1;
            continue;
        }

        // **予約中はまだ引いていない**（11.6）。分けて数える
        if d.status == PLAN {
            out.planned += 1;
            out.watt_with_plan += d.power_watt;
        } else {
            out.running += 1;
            out.watt += d.power_watt;
            out.watt_with_plan += d.power_watt;
        }
    }
    Ok(out)
}
