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

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    device, interface_role, interface_stack, interface_vlan, ip_address, os_interface,
    part_catalog, part_instance, part_instance_location, part_port_slot, project, subnet, vlan,
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
const INTERFACE_TYPES: &[&str] = &["Physical", "Bond", "Vlan", "Svi", "Bridge", "Virtual"];
const AGGREGATION_MODES: &[&str] = &["LACP", "Static", "ActiveBackup"];
const TAGGING_MODES: &[&str] = &["Untagged", "Tagged"];
const ZONES: &[&str] = &["DMZ", "WAN", "LAN", "Management", "Isolated"];

/// `INTERFACE_ROLE.role`（8.5）。**運用者が決めるため開いた語彙**だが、
/// 代表例を選択肢として出す。
const ROLES: &[&str] = &[
    "Service",
    "Management",
    "Backup",
    "Storage",
    "ClusterInterconnect",
    "vMotion",
];

const PHYSICAL: &str = "Physical";
const BOND: &str = "Bond";

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
// 機器のインターフェース（設計書8.5、8.6）
// ---------------------------------------------------------------------------

struct InterfaceRow {
    id: i32,
    name: String,
    interface_type: String,
    /// `Physical` のときだけ。`Broadcom P210P Port1` のような表示。
    port: String,
    /// `Bond` のときだけ。
    aggregation_mode: String,
    /// 束ねている下位のインターフェース（8.5）。
    members: String,
    vlans: String,
    roles: String,
    ips: String,
}

#[derive(askama::Template)]
#[template(path = "network_interfaces.html")]
struct InterfacesPage {
    chrome: Chrome,
    project_id: i32,
    device_id: i32,
    hostname: String,
    t_back: String,
    t_title: String,
    t_lead: String,
    t_name: String,
    t_name_hint: String,
    t_type: String,
    t_port: String,
    t_port_hint: String,
    t_aggregation: String,
    t_aggregation_hint: String,
    t_members: String,
    t_member_hint: String,
    t_vlans: String,
    t_vlan_hint: String,
    t_tagging: String,
    t_roles: String,
    t_role_hint: String,
    t_ips: String,
    t_address: String,
    t_prefix: String,
    t_subnet: String,
    t_actions: String,
    t_empty: String,
    t_add: String,
    t_add_vlan: String,
    t_add_role: String,
    t_add_ip: String,
    t_add_member: String,
    t_close: String,
    t_unset: String,
    t_target: String,
    rows: Vec<InterfaceRow>,
    interface_types: Vec<&'static str>,
    aggregation_modes: Vec<&'static str>,
    tagging_modes: Vec<&'static str>,
    roles: Vec<&'static str>,
    ports: Vec<Labeled>,
    vlans: Vec<Labeled>,
    subnets: Vec<Labeled>,
    /// 下位に選べるインターフェース。
    candidates: Vec<Labeled>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn interfaces(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    インターフェースを描く(&state, &current, project_id, device_id, None).await
}

async fn インターフェースを描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    device_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit, l) = 入場(state, current, project_id).await?;
    let d = 対象の機器(state, project_id, device_id).await?;

    let list = 現在のインターフェース(&state.db, device_id).await?;

    let mut rows = Vec::new();
    let mut candidates = Vec::new();
    for i in &list {
        candidates.push(Labeled {
            label: i.os_interface_name.clone(),
            value: i.id.to_string(),
        });

        let vlans = 現在のvlan(&state.db, i.id).await?;
        rows.push(InterfaceRow {
            // **意味を持つ列だけを見せる**（8.5）
            port: match i.interface_type.as_str() {
                PHYSICAL => ポートの表示(&state.db, i).await?,
                _ => String::new(),
            },
            aggregation_mode: match i.interface_type.as_str() {
                BOND => i.aggregation_mode.clone().unwrap_or_default(),
                _ => String::new(),
            },
            members: 下位の表示(&state.db, i.id).await?,
            vlans: vlans
                .iter()
                .map(|(v, link)| format!("{} ({})", v.vlan_tag, link.tagging_mode))
                .collect::<Vec<_>>()
                .join(", "),
            roles: 役割の表示(&state.db, i.id).await?,
            ips: 現在のip(&state.db, i.id)
                .await?
                .iter()
                .map(|ip| format!("{}/{}", ip.ip_address, ip.prefix_length))
                .collect::<Vec<_>>()
                .join(", "),
            id: i.id,
            name: i.os_interface_name.clone(),
            interface_type: i.interface_type.clone(),
        });
    }

    render(&InterfacesPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "devices",
        )
        .await,
        project_id,
        device_id,
        hostname: d.hostname,
        t_back: rust_i18n::t!("devices.back", locale = l).to_string(),
        t_title: rust_i18n::t!("network.interfaces", locale = l).to_string(),
        t_lead: rust_i18n::t!("network.interfaces_lead", locale = l).to_string(),
        t_name: rust_i18n::t!("network.interface", locale = l).to_string(),
        t_name_hint: rust_i18n::t!("network.interface_hint", locale = l).to_string(),
        t_type: rust_i18n::t!("network.interface_type", locale = l).to_string(),
        t_port: rust_i18n::t!("network.port", locale = l).to_string(),
        t_port_hint: rust_i18n::t!("network.port_hint", locale = l).to_string(),
        t_aggregation: rust_i18n::t!("network.aggregation", locale = l).to_string(),
        t_aggregation_hint: rust_i18n::t!("network.aggregation_hint", locale = l).to_string(),
        t_members: rust_i18n::t!("network.members", locale = l).to_string(),
        t_member_hint: rust_i18n::t!("network.member_hint", locale = l).to_string(),
        t_vlans: rust_i18n::t!("vlans.title", locale = l).to_string(),
        t_vlan_hint: rust_i18n::t!("network.vlan_hint", locale = l).to_string(),
        t_tagging: rust_i18n::t!("network.tagging", locale = l).to_string(),
        t_roles: rust_i18n::t!("network.roles", locale = l).to_string(),
        t_role_hint: rust_i18n::t!("network.role_hint", locale = l).to_string(),
        t_ips: rust_i18n::t!("network.ip_addresses", locale = l).to_string(),
        t_address: rust_i18n::t!("network.address", locale = l).to_string(),
        t_prefix: rust_i18n::t!("network.prefix", locale = l).to_string(),
        t_subnet: rust_i18n::t!("network.subnet", locale = l).to_string(),
        t_actions: rust_i18n::t!("projects.actions", locale = l).to_string(),
        t_empty: rust_i18n::t!("network.no_interface", locale = l).to_string(),
        t_add: rust_i18n::t!("network.add_interface", locale = l).to_string(),
        t_add_vlan: rust_i18n::t!("network.add_vlan", locale = l).to_string(),
        t_add_role: rust_i18n::t!("network.add_role", locale = l).to_string(),
        t_add_ip: rust_i18n::t!("network.add_ip", locale = l).to_string(),
        t_add_member: rust_i18n::t!("network.add_member", locale = l).to_string(),
        t_close: rust_i18n::t!("network.close", locale = l).to_string(),
        t_unset: rust_i18n::t!("catalog.power_unset", locale = l).to_string(),
        t_target: rust_i18n::t!("network.target", locale = l).to_string(),
        rows,
        interface_types: INTERFACE_TYPES.to_vec(),
        aggregation_modes: AGGREGATION_MODES.to_vec(),
        tagging_modes: TAGGING_MODES.to_vec(),
        roles: ROLES.to_vec(),
        ports: 空きポート(state, device_id).await?,
        vlans: 現役のvlan(&state.db).await?,
        subnets: 使えるサブネット(&state.db, project_id).await?,
        candidates,
        can_edit,
        error,
    })
}

#[derive(Debug, Deserialize)]
pub struct InterfaceForm {
    #[serde(default)]
    pub os_interface_name: String,
    #[serde(default)]
    pub interface_type: String,
    #[serde(default)]
    pub port: String,
    #[serde(default)]
    pub aggregation_mode: String,
}

pub async fn create_interface(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<InterfaceForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    対象の機器(&state, project_id, device_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());
    let 戻る = |key: &str| {
        インターフェースを描く(&state, &current, project_id, device_id, 誤り(key))
    };

    let name = 正規化(&form.os_interface_name);
    if name.is_empty() {
        return 戻る("network.error_name").await;
    }
    // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
    if !INTERFACE_TYPES.contains(&form.interface_type.as_str()) {
        return 戻る("network.error_type").await;
    }

    // **物理ポートを持てるのは Physical だけ**（8.5）。VMには PART_INSTANCE が
    // 無く、bond も物理ポートに1対1で対応しない
    let 物理 = form.interface_type == PHYSICAL;
    let (part_instance_id, port_slot_id) = match form.port.trim() {
        "" => (None, None),
        v if !物理 => {
            let _ = v;
            return 戻る("network.error_port_unused").await;
        }
        v => match v.split_once(':') {
            Some((a, b)) => match (a.parse::<i32>(), b.parse::<i32>()) {
                (Ok(a), Ok(b)) => (Some(a), Some(b)),
                _ => return 戻る("network.error_port").await,
            },
            None => return 戻る("network.error_port").await,
        },
    };

    // **`aggregation_mode` を持てるのは Bond だけ**（8.5）
    let aggregation_mode = match form.aggregation_mode.trim() {
        "" => None,
        _ if form.interface_type != BOND => return 戻る("network.error_aggregation_unused").await,
        v if AGGREGATION_MODES.contains(&v) => Some(v.to_owned()),
        _ => return 戻る("network.error_aggregation").await,
    };

    // 同じ機器に同じ名前のインターフェースを2本持たない
    let 重複 = 現在のインターフェース(&state.db, device_id)
        .await?
        .into_iter()
        .any(|i| i.os_interface_name == name);
    if 重複 {
        return 戻る("network.error_interface_duplicate").await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(os_interface::ActiveModel {
        device_id: Set(device_id),
        interface_type: Set(form.interface_type.clone()),
        part_instance_id: Set(part_instance_id),
        port_slot_id: Set(port_slot_id),
        os_interface_name: Set(name),
        aggregation_mode: Set(aggregation_mode),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id, device_id))
}

// ---------------------------------------------------------------------------
// インターフェースに載るもの（VLAN・役割・IP・下位）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct VlanForm {
    pub os_interface_id: i32,
    pub vlan_id: i32,
    #[serde(default)]
    pub tagging_mode: String,
}

pub async fn add_vlan(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<VlanForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let i = このインターフェース(&state, project_id, device_id, form.os_interface_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    if !TAGGING_MODES.contains(&form.tagging_mode.as_str()) {
        return インターフェースを描く(
            &state,
            &current,
            project_id,
            device_id,
            誤り("network.error_tagging"),
        )
        .await;
    }

    // **同じVLANを2行載せない。**トランクポートは複数のVLANを持つが、同じ
    // VLANを2回持つことに意味は無い（8.6）
    let 重複 = 現在のvlan(&state.db, i.id)
        .await?
        .iter()
        .any(|(v, _)| v.id == form.vlan_id);
    if 重複 {
        return インターフェースを描く(
            &state,
            &current,
            project_id,
            device_id,
            誤り("network.error_vlan_duplicate"),
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(interface_vlan::ActiveModel {
        os_interface_id: Set(i.id),
        vlan_id: Set(form.vlan_id),
        tagging_mode: Set(form.tagging_mode.clone()),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id, device_id))
}

#[derive(Debug, Deserialize)]
pub struct RoleForm {
    pub os_interface_id: i32,
    #[serde(default)]
    pub role: String,
}

/// 役割を付ける（8.5）。
///
/// **`SOFTWARE_ROLE_ASSIGNMENT` と同じ形。**1つのインターフェースに複数付与
/// できる。ゾーン（`VLAN`/`SUBNET`）とは別軸である。
pub async fn add_role(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<RoleForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let i = このインターフェース(&state, project_id, device_id, form.os_interface_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **役割は開いた語彙**（8.5）。運用者が決めるため、表に無い役割もありうる
    let role = 正規化(&form.role);
    if role.is_empty() {
        return インターフェースを描く(
            &state,
            &current,
            project_id,
            device_id,
            誤り("network.error_role"),
        )
        .await;
    }

    let 重複 = 現在の役割(&state.db, i.id)
        .await?
        .iter()
        .any(|r| r.role == role);
    if 重複 {
        return インターフェースを描く(
            &state,
            &current,
            project_id,
            device_id,
            誤り("network.error_role_duplicate"),
        )
        .await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(interface_role::ActiveModel {
        os_interface_id: Set(i.id),
        role: Set(role),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id, device_id))
}

#[derive(Debug, Deserialize)]
pub struct IpForm {
    pub os_interface_id: i32,
    #[serde(default)]
    pub ip_address: String,
    #[serde(default)]
    pub prefix_length: String,
    #[serde(default)]
    pub subnet_id: String,
}

pub async fn add_ip(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<IpForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let i = このインターフェース(&state, project_id, device_id, form.os_interface_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());
    let 戻る = |key: &str| {
        インターフェースを描く(&state, &current, project_id, device_id, 誤り(key))
    };

    let address = 正規化(&form.ip_address);
    if address.is_empty()
        || !address
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':')
    {
        return 戻る("network.error_ip").await;
    }
    let prefix = match form.prefix_length.trim().parse::<i32>() {
        Ok(n) if (0..=128).contains(&n) => n,
        _ => return 戻る("network.error_prefix").await,
    };

    let subnet_id = match form.subnet_id.trim() {
        "" => None,
        v => match v.parse::<i32>() {
            Ok(id) => Some(id),
            Err(_) => return 戻る("network.error_ip").await,
        },
    };

    // **同一サブネット内の重複はDB制約が禁じている**（14.2の部分インデックス）。
    // それでも先に見るのは、制約違反の500ではなく理由を返すため
    if let Some(sid) = subnet_id {
        let 重複 = ip_address::Entity::find()
            .filter(ip_address::Column::SubnetId.eq(sid))
            .filter(ip_address::Column::IpAddress.eq(&address))
            .filter(ip_address::Column::ToDate.is_null())
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 重複.is_some() {
            return 戻る("network.error_ip_duplicate").await;
        }
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(ip_address::ActiveModel {
        os_interface_id: Set(i.id),
        ip_address: Set(address),
        prefix_length: Set(prefix),
        subnet_id: Set(subnet_id),
        work_order_id: Set(None),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id, device_id))
}

#[derive(Debug, Deserialize)]
pub struct MemberForm {
    /// 束ねる側。
    pub os_interface_id: i32,
    /// 束ねられる側。
    pub lower_interface_id: i32,
}

/// 下位のインターフェースを束ねる（8.5）。
///
/// **中間テーブルで両方向を扱う。**ボンドは1つのupperに複数のlowerを持ち、
/// VLANサブインターフェースは1つのlowerに複数のupperを持ちうる。
pub async fn add_member(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<MemberForm>,
) -> AppResult<Response> {
    let l = 編集入場(&state, &current, project_id).await?;
    let upper = このインターフェース(&state, project_id, device_id, form.os_interface_id).await?;
    let lower =
        このインターフェース(&state, project_id, device_id, form.lower_interface_id).await?;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());
    let 戻る = |key: &str| {
        インターフェースを描く(&state, &current, project_id, device_id, 誤り(key))
    };

    // **自分自身は束ねられない。**単純だが、選択肢に自分が並ぶ以上は起こりうる
    if upper.id == lower.id {
        return 戻る("network.error_member_self").await;
    }
    // **循環を作らせない。**bond0 の下に bond0.100 を入れると、上下を辿る
    // 表示が終わらなくなる
    if 上位を辿ると含む(&state.db, upper.id, lower.id).await? {
        return 戻る("network.error_member_cycle").await;
    }

    let 重複 = 現在の下位(&state.db, upper.id)
        .await?
        .iter()
        .any(|s| s.lower_interface_id == lower.id);
    if 重複 {
        return 戻る("network.error_member_duplicate").await;
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(interface_stack::ActiveModel {
        upper_interface_id: Set(upper.id),
        lower_interface_id: Set(lower.id),
        from_date: Set(Utc::now()),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id, device_id))
}

// ---------------------------------------------------------------------------
// 閉じる（設計書4章）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CloseForm {
    /// `interface` / `vlan` / `role` / `ip` / `member`。
    pub target: String,
    pub id: i32,
}

/// 現在有効な行を閉じる。
///
/// **消さない**（4章）。「いつまでそうだったか」は事実であり、ボンドの構成
/// メンバーが変わったこと、IPが付け替えられたことは履歴として残す。
///
/// **インターフェースを閉じるときは、その上に載っているものも閉じる。**
/// 閉じたインターフェースにIPやVLANがぶら下がったままだと、現在の状態を
/// 計算したときに存在しないインターフェースのIPが出る。
pub async fn close(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<CloseForm>,
) -> AppResult<Response> {
    編集入場(&state, &current, project_id).await?;
    対象の機器(&state, project_id, device_id).await?;

    // **読み取りはトランザクションを開く前に済ませる。**接続を1本しか持たない
    // 構成（SQLiteのインメモリ）では、開いたトランザクションの内側で
    // `state.db` を引くと自分自身の書き込みロックを待って止まる
    let 閉じるもの = 対象を集める(&state, project_id, device_id, &form).await?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    for row in 閉じるもの.vlans {
        閉じるvlan(&tx, row, now).await?;
    }
    for row in 閉じるもの.roles {
        tx.update(
            &row,
            interface_role::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    for row in 閉じるもの.ips {
        tx.update(
            &row,
            ip_address::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    for row in 閉じるもの.stacks {
        tx.update(
            &row,
            interface_stack::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    if let Some(row) = 閉じるもの.interface {
        tx.update(
            &row,
            os_interface::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(戻り先(project_id, device_id))
}

/// 閉じる対象。**インターフェースを閉じるときは、その上に載っているものも含む。**
#[derive(Default)]
struct 閉じる対象 {
    interface: Option<os_interface::Model>,
    vlans: Vec<interface_vlan::Model>,
    roles: Vec<interface_role::Model>,
    ips: Vec<ip_address::Model>,
    stacks: Vec<interface_stack::Model>,
}

async fn 対象を集める(
    state: &AppState,
    project_id: i32,
    device_id: i32,
    form: &CloseForm,
) -> AppResult<閉じる対象> {
    let mut out = 閉じる対象::default();

    match form.target.as_str() {
        "interface" => {
            let i = このインターフェース(state, project_id, device_id, form.id).await?;
            out.vlans = 現在のvlan(&state.db, i.id)
                .await?
                .into_iter()
                .map(|(_, link)| link)
                .collect();
            out.roles = 現在の役割(&state.db, i.id).await?;
            out.ips = 現在のip(&state.db, i.id).await?;
            // **上位・下位の両方向を閉じる。**片方だけ残すと、閉じた
            // インターフェースがボンドのメンバーとして残り続ける
            out.stacks = 関わる積み重ね(&state.db, i.id).await?;
            out.interface = Some(i);
        }
        "vlan" => {
            let row = 子の行(
                state,
                project_id,
                device_id,
                interface_vlan::Entity::find_by_id(form.id),
                |r: &interface_vlan::Model| r.os_interface_id,
            )
            .await?;
            out.vlans.push(row);
        }
        "role" => {
            let row = 子の行(
                state,
                project_id,
                device_id,
                interface_role::Entity::find_by_id(form.id),
                |r: &interface_role::Model| r.os_interface_id,
            )
            .await?;
            out.roles.push(row);
        }
        "ip" => {
            let row = 子の行(
                state,
                project_id,
                device_id,
                ip_address::Entity::find_by_id(form.id),
                |r: &ip_address::Model| r.os_interface_id,
            )
            .await?;
            out.ips.push(row);
        }
        "member" => {
            let row = 子の行(
                state,
                project_id,
                device_id,
                interface_stack::Entity::find_by_id(form.id),
                |r: &interface_stack::Model| r.upper_interface_id,
            )
            .await?;
            out.stacks.push(row);
        }
        _ => return Err(AppError::NotFound),
    }

    Ok(out)
}

async fn 閉じるvlan(
    tx: &AuditedTx,
    row: interface_vlan::Model,
    now: chrono::DateTime<Utc>,
) -> AppResult<()> {
    tx.update(
        &row,
        interface_vlan::ActiveModel {
            id: Set(row.id),
            to_date: Set(Some(now)),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(())
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

fn 戻り先(project_id: i32, device_id: i32) -> Response {
    Redirect::to(&format!(
        "/projects/{project_id}/devices/{device_id}/interfaces"
    ))
    .into_response()
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

/// **このプロジェクトから見える機器に限る**（A-6）。
async fn 対象の機器(
    state: &AppState,
    project_id: i32,
    device_id: i32,
) -> AppResult<device::Model> {
    let 見える = entity::device_assignment::Entity::find()
        .filter(entity::device_assignment::Column::DeviceId.eq(device_id))
        .filter(entity::device_assignment::Column::LocationType.eq("Project"))
        .filter(entity::device_assignment::Column::LocationId.eq(project_id))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .is_some();
    if !見える {
        return Err(AppError::NotFound);
    }

    device::Entity::find_by_id(device_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)
}

/// **この機器のインターフェースであることを確かめる。**IDだけを信じると、
/// 別の機器のインターフェースを操作できてしまう。
async fn このインターフェース(
    state: &AppState,
    project_id: i32,
    device_id: i32,
    id: i32,
) -> AppResult<os_interface::Model> {
    対象の機器(state, project_id, device_id).await?;
    os_interface::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|i| i.device_id == device_id && i.to_date.is_none())
        .ok_or(AppError::NotFound)
}

/// 子の行を引き、そのインターフェースがこの機器のものか確かめる。
async fn 子の行<E, F>(
    state: &AppState,
    project_id: i32,
    device_id: i32,
    query: sea_orm::Select<E>,
    親: F,
) -> AppResult<E::Model>
where
    E: EntityTrait,
    F: Fn(&E::Model) -> i32,
{
    let row = query
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;
    このインターフェース(state, project_id, device_id, 親(&row)).await?;
    Ok(row)
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

async fn 現在の下位<C: ConnectionTrait>(
    db: &C,
    upper: i32,
) -> AppResult<Vec<interface_stack::Model>> {
    interface_stack::Entity::find()
        .filter(interface_stack::Column::UpperInterfaceId.eq(upper))
        .filter(interface_stack::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// そのインターフェースが上位・下位のどちらとして関わっている行も返す。
async fn 関わる積み重ね<C: ConnectionTrait>(
    db: &C,
    id: i32,
) -> AppResult<Vec<interface_stack::Model>> {
    interface_stack::Entity::find()
        .filter(
            Condition::any()
                .add(interface_stack::Column::UpperInterfaceId.eq(id))
                .add(interface_stack::Column::LowerInterfaceId.eq(id)),
        )
        .filter(interface_stack::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// `lower` を辿って `upper` に行き着くか。**循環の検出に使う。**
async fn 上位を辿ると含む<C: ConnectionTrait>(
    db: &C,
    upper: i32,
    lower: i32,
) -> AppResult<bool> {
    let mut 見る = vec![lower];
    let mut 見た = Vec::new();

    while let Some(id) = 見る.pop() {
        if id == upper {
            return Ok(true);
        }
        if 見た.contains(&id) {
            continue;
        }
        見た.push(id);
        for s in 現在の下位(db, id).await? {
            見る.push(s.lower_interface_id);
        }
    }
    Ok(false)
}

async fn 下位の表示<C: ConnectionTrait>(db: &C, upper: i32) -> AppResult<String> {
    let mut names = Vec::new();
    for s in 現在の下位(db, upper).await? {
        if let Some(i) = os_interface::Entity::find_by_id(s.lower_interface_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            names.push(i.os_interface_name);
        }
    }
    Ok(names.join(", "))
}

async fn 役割の表示<C: ConnectionTrait>(db: &C, interface_id: i32) -> AppResult<String> {
    Ok(現在の役割(db, interface_id)
        .await?
        .into_iter()
        .map(|r| r.role)
        .collect::<Vec<_>>()
        .join(", "))
}

/// `Broadcom P210P Port1`。
async fn ポートの表示<C: ConnectionTrait>(
    db: &C,
    i: &os_interface::Model,
) -> AppResult<String> {
    let (Some(instance_id), Some(slot_id)) = (i.part_instance_id, i.port_slot_id) else {
        return Ok(String::new());
    };

    let slot = part_port_slot::Entity::find_by_id(slot_id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let catalog = match part_instance::Entity::find_by_id(instance_id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        Some(pi) => part_catalog::Entity::find_by_id(pi.part_catalog_id)
            .one(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?,
        None => None,
    };

    Ok(format!(
        "{} {}",
        catalog.map(|c| c.part_number).unwrap_or_default(),
        slot.map(|s| s.port_label).unwrap_or_default()
    )
    .trim()
    .to_owned())
}

/// この機器に載っている部品のネットワークポートのうち、まだ使われていないもの。
///
/// **値は `part_instance_id:port_slot_id`。**どのカードのどのポートかを1つの
/// 選択肢で選べるようにする（8.5の経路をそのまま辿る）。
async fn 空きポート(state: &AppState, device_id: i32) -> AppResult<Vec<Labeled>> {
    let 使用中: Vec<(i32, i32)> = 現在のインターフェース(&state.db, device_id)
        .await?
        .into_iter()
        .filter_map(|i| i.part_instance_id.zip(i.port_slot_id))
        .collect();

    let locations = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq("Device"))
        .filter(part_instance_location::Column::LocationId.eq(device_id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut out = Vec::new();
    for loc in locations {
        let Some(instance) = part_instance::Entity::find_by_id(loc.part_instance_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        else {
            continue;
        };
        let catalog = part_catalog::Entity::find_by_id(instance.part_catalog_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        let 型番 = catalog.map(|c| c.part_number).unwrap_or_default();

        let ports = part_port_slot::Entity::find()
            .filter(part_port_slot::Column::PartCatalogId.eq(instance.part_catalog_id))
            .filter(part_port_slot::Column::PortKind.eq("Network"))
            .order_by_asc(part_port_slot::Column::Id)
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

        for p in ports {
            if 使用中.contains(&(instance.id, p.id)) {
                continue;
            }
            out.push(Labeled {
                label: format!("{型番} {}", p.port_label),
                value: format!("{}:{}", instance.id, p.id),
            });
        }
    }
    Ok(out)
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

/// 自分のプロジェクトのサブネットと、共有のもの（14.1）。
async fn 使えるサブネット<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    Ok(subnet::Entity::find()
        .filter(自分か共有(project_id))
        .order_by_asc(subnet::Column::Cidr)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|s| Labeled {
            label: s.cidr,
            value: s.id.to_string(),
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
