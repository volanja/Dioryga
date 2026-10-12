//! ネットワーク管理（設計書16.1のB領域、8.5、8.6、14章）。
//!
//! # 答えたい問いから逆算する（8.5）
//!
//! > サーバにNICカードが2枚あってポートが計8個ある。**各ポートがOSからどう
//! > 見えていて、何のネットワークに繋がっていて、何の仕事をしているのか。**
//!
//! この経路は `PART_CATALOG` → `PART_INSTANCE` → `PART_PORT_SLOT` →
//! `OS_INTERFACE` → `IP_ADDRESS` → `VLAN`/`SUBNET` であり、**`CHASSIS_SLOT` を
//! 一度も通らない。**画面もこの順で辿れるように組む。
//!
//! # 物理ポートではなく `OS_INTERFACE` を中心に置く（8.5）
//!
//! **IPアドレスは `bond0` に付き、物理ポートには付かない。**8ポートのNICが
//! 4本のbondに束ねられる構成が実務では普通で、旧設計のように
//! 「`OS_INTERFACE` は物理ポートに必ず紐づく」とすると、IPをどれか1本の
//! 物理ポートに恣意的に紐づけるしかなくなる。
//!
//! - `interface_type=Physical` のときだけ `part_instance_id` / `port_slot_id` を持つ
//! - `interface_type=Bond` のときだけ `aggregation_mode` を持つ
//! - VMには `PART_INSTANCE` が無いため `Virtual` とし、物理ポートを持たない
//!
//! # 積み重ねは中間テーブルで両方向を扱う（8.5）
//!
//! **単一のFKでは表せない。**ボンドは1つのupperに複数のlowerを持ち、VLANサブ
//! インターフェースは1つのlowerに複数のupperを持ちうる。
//!
//! # 役割とゾーンは別軸（8.5）
//!
//! `INTERFACE_ROLE` はホスト側（サーバ運用者が決める）、`VLAN.zone` /
//! `SUBNET.zone` はネットワーク側（ネットワーク設計者が決める）。**分けておく
//! ことで両者の食い違いを検出できる**——「役割がBackupなのにDMZゾーンに
//! 載っている」はセキュリティレビューで実際に問いたくなる指摘である。
//!
//! # すべて履歴テーブルである（4章）
//!
//! `OS_INTERFACE` / `INTERFACE_STACK` / `INTERFACE_VLAN` / `INTERFACE_ROLE` /
//! `IP_ADDRESS` はいずれも `from_date` / `to_date` を持つ。**行を消さず、
//! 閉じる。**ボンドの構成メンバーが変わったこと（ポート故障時の切り離し等）も
//! 事実として残る。
//!
//! # インターフェースの編集は機器の詳細に置く（16.1）
//!
//! 機器ごとのインターフェースの編集は、機器の詳細の「ポートとIP」の節
//! （`server/device_sections.rs`）にある。ここにはサブネットと、プロジェクトを
//! 横断するIPアドレスの一覧を置く。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    device, interface_role, interface_vlan, ip_address, os_interface, project, subnet, vlan,
};
use sea_orm::{ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::{正規化, Labeled};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 閉じた語彙（`vocabularies.md`、設計書8.5）。
const ZONES: &[&str] = &["DMZ", "WAN", "LAN", "Management", "Isolated"];

// ---------------------------------------------------------------------------
// サブネット（設計書14章）
// ---------------------------------------------------------------------------

struct SubnetRow {
    cidr: String,
    vlan: String,
    /// 実効のゾーン。**両方あるときは `SUBNET` を優先する**（14.2）。
    zone: String,
    /// ゾーンがVLAN由来であることを示す。
    zone_from_vlan: bool,
    description: String,
    /// 現在このサブネットに載っているIPの数。
    used: usize,
    shared: bool,
}

#[derive(askama::Template)]
#[template(path = "network_subnets.html")]
struct SubnetsPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_cidr: String,
    t_cidr_hint: String,
    t_vlan: String,
    t_zone: String,
    t_zone_hint: String,
    t_description: String,
    t_used: String,
    t_shared: String,
    t_from_vlan: String,
    t_empty: String,
    t_new: String,
    t_submit: String,
    t_unset: String,
    t_ip_addresses: String,
    rows: Vec<SubnetRow>,
    vlans: Vec<Labeled>,
    zones: Vec<&'static str>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn subnets(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    サブネットを描く(&state, &current, project_id, None).await
}

async fn サブネットを描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit, l) = 入場(state, current, project_id).await?;

    // **共有バックボーン等は `project_id` が null**（14.1）。自分のものと
    // 共有のものを両方見せる
    let list = subnet::Entity::find()
        .filter(自分か共有(project_id))
        .order_by_asc(subnet::Column::Cidr)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for s in list {
        let v = match s.vlan_id {
            Some(id) => vlan::Entity::find_by_id(id)
                .one(&state.db)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?,
            None => None,
        };
        // **両方あるときは `SUBNET.zone` を優先する**（14.2）
        let (zone, zone_from_vlan) = match (&s.zone, v.as_ref().and_then(|v| v.zone.clone())) {
            (Some(z), _) => (z.clone(), false),
            (None, Some(z)) => (z, true),
            (None, None) => (String::new(), false),
        };

        rows.push(SubnetRow {
            used: 使用中のip数(&state.db, s.id).await?,
            vlan: v
                .map(|v| format!("{} {}", v.vlan_tag, v.name))
                .unwrap_or_default(),
            shared: s.project_id.is_none(),
            zone,
            zone_from_vlan,
            cidr: s.cidr,
            description: s.description,
        });
    }

    render(&SubnetsPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "ip_addresses",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("network.subnets", locale = l).to_string(),
        t_lead: rust_i18n::t!("network.subnets_lead", locale = l).to_string(),
        t_cidr: rust_i18n::t!("network.cidr", locale = l).to_string(),
        t_cidr_hint: rust_i18n::t!("network.cidr_hint", locale = l).to_string(),
        t_vlan: rust_i18n::t!("vlans.title", locale = l).to_string(),
        t_zone: rust_i18n::t!("vlans.zone", locale = l).to_string(),
        t_zone_hint: rust_i18n::t!("network.zone_hint", locale = l).to_string(),
        t_description: rust_i18n::t!("vlans.description", locale = l).to_string(),
        t_used: rust_i18n::t!("network.used", locale = l).to_string(),
        t_shared: rust_i18n::t!("network.shared", locale = l).to_string(),
        t_from_vlan: rust_i18n::t!("network.zone_from_vlan", locale = l).to_string(),
        t_empty: rust_i18n::t!("catalog.empty", locale = l).to_string(),
        t_new: rust_i18n::t!("network.new_subnet", locale = l).to_string(),
        t_submit: rust_i18n::t!("catalog.submit", locale = l).to_string(),
        t_unset: rust_i18n::t!("catalog.power_unset", locale = l).to_string(),
        t_ip_addresses: rust_i18n::t!("network.ip_addresses", locale = l).to_string(),
        rows,
        vlans: 現役のvlan(&state.db).await?,
        zones: ZONES.to_vec(),
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct SubnetForm {
    #[serde(default)]
    pub cidr: String,
    #[serde(default)]
    pub vlan_id: String,
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub description: String,
}

pub async fn create_subnet(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<SubnetForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    let cidr = 正規化(&form.cidr);
    if !cidrとして読める(&cidr) {
        return サブネットを描く(&state, &current, project_id, 誤り("network.error_cidr")).await;
    }

    let zone = match form.zone.trim() {
        "" => None,
        z if ZONES.contains(&z) => Some(z.to_owned()),
        // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
        _ => {
            return サブネットを描く(&state, &current, project_id, 誤り("vlans.error_zone")).await
        }
    };

    let vlan_id = match form.vlan_id.trim() {
        "" => None,
        v => match v.parse::<i32>() {
            Ok(id) => Some(id),
            Err(_) => {
                return サブネットを描く(
                    &state,
                    &current,
                    project_id,
                    誤り("cables.error_vendor"),
                )
                .await
            }
        },
    };

    // **同じプロジェクト内で同じCIDRを2つ持たない。**プロジェクトをまたぐ重複は
    // 正当である——異なるプロジェクトが同じプライベート帯を独立に使える（14.2）
    let 重複 = subnet::Entity::find()
        .filter(subnet::Column::ProjectId.eq(project_id))
        .filter(subnet::Column::Cidr.eq(&cidr))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if 重複.is_some() {
        return サブネットを描く(
            &state,
            &current,
            project_id,
            誤り("network.error_subnet_duplicate"),
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();
    tx.insert(subnet::ActiveModel {
        project_id: Set(Some(project_id)),
        vlan_id: Set(vlan_id),
        cidr: Set(cidr),
        zone: Set(zone),
        description: Set(正規化(&form.description)),
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

    Ok(Redirect::to(&format!("/projects/{project_id}/network/subnets")).into_response())
}

/// `192.168.1.0/24` の形をしているか。
///
/// **正しさの検査ではなく、書き間違いを弾くだけ。**アドレス計算はv1で扱わない
/// （14.2の空きIP算出はv2）。ここで厳密なパースを持ち込むと、IPv6や特殊な
/// 表記で正当な入力を拒否する側の誤りが増える。
pub(crate) fn cidrとして読める(value: &str) -> bool {
    let Some((addr, prefix)) = value.split_once('/') else {
        return false;
    };
    if addr.is_empty()
        || !addr
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':')
    {
        return false;
    }
    matches!(prefix.parse::<u32>(), Ok(n) if n <= 128)
}

// ---------------------------------------------------------------------------
// IPアドレス一覧（設計書16.1、14章）
// ---------------------------------------------------------------------------

struct IpRow {
    device_id: i32,
    hostname: String,
    interface: String,
    interface_type: String,
    address: String,
    subnet: String,
    vlan: String,
    zone: String,
    roles: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: Option<String>,
}

#[derive(askama::Template)]
#[template(path = "network_ip_addresses.html")]
struct IpAddressesPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_keyword: String,
    t_apply: String,
    t_device: String,
    t_interface: String,
    t_type: String,
    t_address: String,
    t_subnet: String,
    t_vlan: String,
    t_zone: String,
    t_roles: String,
    t_actions: String,
    t_detail: String,
    t_empty: String,
    t_subnets: String,
    rows: Vec<IpRow>,
    q: String,
}

pub async fn ip_addresses(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Response> {
    let (project, _, l) = 入場(&state, &current, project_id).await?;
    let keyword = query.q.clone().unwrap_or_default();
    let 絞る = keyword.trim().to_lowercase();

    let mut rows = Vec::new();
    for d in このプロジェクトの機器(&state, project_id).await? {
        for i in 現在のインターフェース(&state.db, d.id).await? {
            let roles = 役割の表示(&state.db, i.id).await?;
            let vlans = 現在のvlan(&state.db, i.id).await?;

            for ip in 現在のip(&state.db, i.id).await? {
                let s = match ip.subnet_id {
                    Some(id) => subnet::Entity::find_by_id(id)
                        .one(&state.db)
                        .await
                        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?,
                    None => None,
                };
                let zone = 実効のゾーン(&state.db, s.as_ref(), &vlans).await?;

                let address = format!("{}/{}", ip.ip_address, ip.prefix_length);
                if !絞る.is_empty()
                    && !address.to_lowercase().contains(&絞る)
                    && !d.hostname.to_lowercase().contains(&絞る)
                {
                    continue;
                }

                rows.push(IpRow {
                    device_id: d.id,
                    hostname: d.hostname.clone(),
                    interface: i.os_interface_name.clone(),
                    interface_type: i.interface_type.clone(),
                    address,
                    subnet: s.map(|s| s.cidr).unwrap_or_default(),
                    vlan: vlans
                        .iter()
                        .map(|(v, _)| v.vlan_tag.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    zone,
                    roles: roles.clone(),
                });
            }
        }
    }
    rows.sort_by(|a, b| (&a.hostname, &a.address).cmp(&(&b.hostname, &b.address)));

    render(&IpAddressesPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "ip_addresses",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("network.ip_addresses", locale = l).to_string(),
        t_lead: rust_i18n::t!("network.ip_addresses_lead", locale = l).to_string(),
        t_keyword: rust_i18n::t!("components.keyword", locale = l).to_string(),
        t_apply: rust_i18n::t!("common.search", locale = l).to_string(),
        t_device: rust_i18n::t!("components.device", locale = l).to_string(),
        t_interface: rust_i18n::t!("network.interface", locale = l).to_string(),
        t_type: rust_i18n::t!("network.interface_type", locale = l).to_string(),
        t_address: rust_i18n::t!("network.address", locale = l).to_string(),
        t_subnet: rust_i18n::t!("network.subnet", locale = l).to_string(),
        t_vlan: rust_i18n::t!("vlans.title", locale = l).to_string(),
        t_zone: rust_i18n::t!("vlans.zone", locale = l).to_string(),
        t_roles: rust_i18n::t!("network.roles", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_detail: rust_i18n::t!("network.interfaces", locale = l).to_string(),
        t_empty: rust_i18n::t!("network.no_ip", locale = l).to_string(),
        t_subnets: rust_i18n::t!("network.subnets", locale = l).to_string(),
        rows,
        q: keyword,
    })
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// 自分のプロジェクトのものと、共有バックボーン等（`project_id` が null）（14.1）。
fn 自分か共有(project_id: i32) -> Condition {
    Condition::any()
        .add(subnet::Column::ProjectId.eq(project_id))
        .add(subnet::Column::ProjectId.is_null())
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

/// **ApproverとViewerはここで止まる。**
async fn 編集入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<&'static str> {
    let (_, can_edit, l) = 入場(state, current, project_id).await?;
    if !can_edit {
        return Err(AppError::Forbidden);
    }
    Ok(l)
}

async fn このプロジェクトの機器(
    state: &AppState,
    project_id: i32,
) -> AppResult<Vec<device::Model>> {
    let ids: Vec<i32> = entity::device_assignment::Entity::find()
        .filter(entity::device_assignment::Column::LocationType.eq("Project"))
        .filter(entity::device_assignment::Column::LocationId.eq(project_id))
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|a| a.device_id)
        .collect();

    if ids.is_empty() {
        return Ok(Vec::new());
    }
    device::Entity::find()
        .filter(device::Column::Id.is_in(ids))
        .order_by_asc(device::Column::Hostname)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn 現在のインターフェース<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Vec<os_interface::Model>> {
    os_interface::Entity::find()
        .filter(os_interface::Column::DeviceId.eq(device_id))
        .filter(os_interface::Column::ToDate.is_null())
        .order_by_asc(os_interface::Column::OsInterfaceName)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// そのインターフェースに現在載っているVLANと、載せている行（8.6）。
async fn 現在のvlan<C: ConnectionTrait>(
    db: &C,
    interface_id: i32,
) -> AppResult<Vec<(vlan::Model, interface_vlan::Model)>> {
    let links = interface_vlan::Entity::find()
        .filter(interface_vlan::Column::OsInterfaceId.eq(interface_id))
        .filter(interface_vlan::Column::ToDate.is_null())
        .order_by_asc(interface_vlan::Column::Id)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for link in links {
        let Some(v) = vlan::Entity::find_by_id(link.vlan_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            continue;
        };
        out.push((v, link));
    }
    Ok(out)
}

async fn 現在の役割<C: ConnectionTrait>(
    db: &C,
    interface_id: i32,
) -> AppResult<Vec<interface_role::Model>> {
    interface_role::Entity::find()
        .filter(interface_role::Column::OsInterfaceId.eq(interface_id))
        .filter(interface_role::Column::ToDate.is_null())
        .order_by_asc(interface_role::Column::Id)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn 現在のip<C: ConnectionTrait>(
    db: &C,
    interface_id: i32,
) -> AppResult<Vec<ip_address::Model>> {
    ip_address::Entity::find()
        .filter(ip_address::Column::OsInterfaceId.eq(interface_id))
        .filter(ip_address::Column::ToDate.is_null())
        .order_by_asc(ip_address::Column::Id)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn 役割の表示<C: ConnectionTrait>(db: &C, interface_id: i32) -> AppResult<String> {
    Ok(現在の役割(db, interface_id)
        .await?
        .into_iter()
        .map(|r| r.role)
        .collect::<Vec<_>>()
        .join(", "))
}

async fn 現役のvlan<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    Ok(vlan::Entity::find()
        .filter(vlan::Column::RetiredAt.is_null())
        .order_by_asc(vlan::Column::VlanTag)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|v| Labeled {
            label: format!("{} {}", v.vlan_tag, v.name),
            value: v.id.to_string(),
        })
        .collect())
}

async fn 使用中のip数<C: ConnectionTrait>(db: &C, subnet_id: i32) -> AppResult<usize> {
    Ok(ip_address::Entity::find()
        .filter(ip_address::Column::SubnetId.eq(subnet_id))
        .filter(ip_address::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .len())
}

/// 実効のゾーン。**`SUBNET` を優先し、無ければVLAN側を見る**（14.2）。
async fn 実効のゾーン<C: ConnectionTrait>(
    _db: &C,
    s: Option<&subnet::Model>,
    vlans: &[(vlan::Model, interface_vlan::Model)],
) -> AppResult<String> {
    if let Some(z) = s.and_then(|s| s.zone.clone()) {
        return Ok(z);
    }
    Ok(vlans
        .iter()
        .find_map(|(v, _)| v.zone.clone())
        .unwrap_or_default())
}
