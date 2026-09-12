//! ネットワークの取込（設計書23.5、8.3、8.5、14.2）。
//!
//! `SUBNET` / `OS_INTERFACE` / `INTERFACE_STACK` / `INTERFACE_VLAN` /
//! `IP_ADDRESS` を扱う。**`VLAN` はカタログYAML側**（23.5）——プロジェクトを
//! 横断するマスタであり、「1ファイルは1プロジェクトに閉じる」と噛み合わない。
//!
//! # 親子関係は独立した行で書く（23.5）
//!
//! ```csv
//! hostname,upper_interface,lower_interface
//! web01,bond0,ens1f0
//! web01,bond0.100,bond0
//! ```
//!
//! `OS_INTERFACE` のCSVに親の列を足す形にしないのは、**子が親を指す形では
//! 多段の構成を表せない**ためである。bond の上に VLAN インタフェースが乗る
//! 構成は実在し（8.3）、1行が親を1つしか持てないと「bond0 は ens1f0 の親で
//! あり、同時に bond0.100 の子でもある」が書けない。
//!
//! # インタフェースの種別が変わったら、閉じて開く
//!
//! `OS_INTERFACE` は履歴である。種別や集約モードが変われば**閉じて新しい行を
//! 開く**（4章）。画面側も同じで、属性を書き換える経路を持たない。
//!
//! **閉じるときは、そのインタフェースに乗っているものも閉じる**（8.5）。
//! 束ね・VLAN・IPアドレス・役割が、閉じた行を指したまま残らないようにする。
//! **同じ取込の中でそれらを書き直せば、新しい行に対して開き直る。**書き直さ
//! なければ消えるため、**閉じた件数を警告に出す**（不変条件6）。
//!
//! # 同一サブネット内のIP重複はDBが禁じている（14.2）
//!
//! 部分一意インデックスがあるため、取込が素通りさせるとDBのエラーで落ちる。
//! **利用者が直せる言葉で止める**ほうがよいので、取込側でも判定する。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{device, interface_stack, interface_vlan, ip_address, os_interface, subnet, vlan};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use super::instances::語彙;
use super::placement::{
    このプロジェクトの機器, 機器キー, 空ならnone, 解決する機器, 読み取る
};
use super::{Entry, ImportError, Outcome, Report};
use crate::repository::AuditedTx;
use crate::server::network::cidrとして読める;

/// 閉じた語彙（`vocabularies.md`、設計書8.3・8.5）。
const INTERFACE_TYPES: &[&str] = &["Physical", "Bond", "Vlan", "Svi", "Bridge", "Virtual"];
const AGGREGATION_MODES: &[&str] = &["LACP", "Static", "ActiveBackup"];
const TAGGING_MODES: &[&str] = &["Untagged", "Tagged"];
const ZONES: &[&str] = &["DMZ", "WAN", "LAN", "Management", "Isolated"];

const BOND: &str = "Bond";

// ---------------------------------------------------------------------------
// 行の定義
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct SubnetRow {
    pub cidr: String,
    /// VLANを伴わないサブネットがありうる（8.5）。
    #[serde(default)]
    pub vlan_tag: String,
    /// 同じタグが複数あるときに絞る（8.5）。
    #[serde(default)]
    pub vlan_name: String,
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InterfaceRow {
    pub hostname: String,
    pub os_interface_name: String,
    pub interface_type: String,
    /// `interface_type=Bond` のときだけ。
    #[serde(default)]
    pub aggregation_mode: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StackRow {
    pub hostname: String,
    /// 束ねる側（bond0、bond0.100）。
    pub upper_interface: String,
    /// 束ねられる側（ens1f0、bond0）。
    pub lower_interface: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InterfaceVlanRow {
    pub hostname: String,
    pub os_interface_name: String,
    pub vlan_tag: String,
    #[serde(default)]
    pub vlan_name: String,
    pub tagging_mode: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpRow {
    pub hostname: String,
    pub os_interface_name: String,
    pub ip_address: String,
    pub prefix_length: String,
    /// 属するサブネット。**空でもよい**が、空だとIP重複を検出できない（14.2）。
    #[serde(default)]
    pub subnet_cidr: String,
}

pub fn parse_subnets(source: &str) -> Result<Vec<SubnetRow>, ImportError> {
    読み取る(source)
}
pub fn parse_interfaces(source: &str) -> Result<Vec<InterfaceRow>, ImportError> {
    読み取る(source)
}
pub fn parse_stacks(source: &str) -> Result<Vec<StackRow>, ImportError> {
    読み取る(source)
}
pub fn parse_interface_vlans(source: &str) -> Result<Vec<InterfaceVlanRow>, ImportError> {
    読み取る(source)
}
pub fn parse_ips(source: &str) -> Result<Vec<IpRow>, ImportError> {
    読み取る(source)
}

// ---------------------------------------------------------------------------
// サブネット（14.2）
// ---------------------------------------------------------------------------

/// サブネットを取り込む。**履歴ではない**ため、変更はその行の更新になる。
pub async fn サブネットを取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[SubnetRow],
    actor: i32,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let now = Utc::now();

    for row in rows {
        let cidr = row.cidr.trim().to_owned();
        let target = format!("SUBNET {cidr}");

        if !cidrとして読める(&cidr) {
            report.push(Entry::new(
                Outcome::Error,
                target,
                format!("cidr「{cidr}」の形が正しくありません"),
            ));
            continue;
        }

        let zone = match 語彙(&row.zone, ZONES, "") {
            Ok(z) => 空ならnone(&z).map(str::to_owned),
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, format!("zone: {理由}")));
                continue;
            }
        };

        let vlan_id = match 解決するvlan(tx, &row.vlan_tag, &row.vlan_name).await? {
            Ok(v) => v,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let 既存 = subnet::Entity::find()
            .filter(subnet::Column::ProjectId.eq(project_id))
            .filter(subnet::Column::Cidr.eq(&cidr))
            .one(tx.reader())
            .await?;

        match 既存 {
            Some(s) => {
                let 同じ = s.vlan_id == vlan_id
                    && s.zone == zone
                    && s.description == row.description.trim();
                if 同じ {
                    report.push(Entry::new(Outcome::Unchanged, target, ""));
                    continue;
                }
                let mut active: subnet::ActiveModel = s.clone().into();
                active.vlan_id = Set(vlan_id);
                active.zone = Set(zone);
                active.description = Set(row.description.trim().to_owned());
                active.updated_at = Set(now);
                tx.update(&s, active).await?;
                report.push(Entry::new(Outcome::Updated, target, ""));
            }
            None => {
                tx.insert(subnet::ActiveModel {
                    project_id: Set(Some(project_id)),
                    vlan_id: Set(vlan_id),
                    cidr: Set(cidr),
                    zone: Set(zone),
                    description: Set(row.description.trim().to_owned()),
                    created_by: Set(actor),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await?;
                report.push(Entry::new(Outcome::Created, target, ""));
            }
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// インタフェース（8.3）
// ---------------------------------------------------------------------------

pub async fn インタフェースを取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[InterfaceRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let 機器 = このプロジェクトの機器(tx.reader(), project_id).await?;

    for row in rows {
        let name = row.os_interface_name.trim().to_owned();
        let target = format!("{} {}", row.hostname.trim(), name);

        if name.is_empty() {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "os_interface_name が空です",
            ));
            continue;
        }

        let d = match 機器を引く(tx, &機器, &row.hostname).await? {
            Ok(d) => d,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let interface_type = match 語彙(&row.interface_type, INTERFACE_TYPES, "") {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    "interface_type が空です",
                ));
                continue;
            }
            Err(理由) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!("interface_type: {理由}"),
                ));
                continue;
            }
        };

        // **集約モードは Bond のときだけ意味を持つ**（8.3）。他の種別に付いて
        // いたら、種別かモードのどちらかが誤っている
        let aggregation_mode = match (interface_type.as_str(), 空ならnone(&row.aggregation_mode))
        {
            (BOND, Some(m)) => match 語彙(m, AGGREGATION_MODES, "") {
                Ok(v) => Some(v),
                Err(理由) => {
                    report.push(Entry::new(
                        Outcome::Error,
                        target,
                        format!("aggregation_mode: {理由}"),
                    ));
                    continue;
                }
            },
            (BOND, None) => None,
            (_, Some(_)) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    "aggregation_mode は interface_type=Bond のときだけ指定できます",
                ));
                continue;
            }
            (_, None) => None,
        };

        let 現在 = 現在のインタフェース(tx, d.id, &name).await?;
        match 現在 {
            Some(i)
                if i.interface_type == interface_type && i.aggregation_mode == aggregation_mode =>
            {
                report.push(Entry::new(Outcome::Unchanged, target, ""));
            }
            Some(i) => {
                // **閉じるときは乗っているものも閉じる**（8.5）
                let 閉じた = 依存を閉じる(tx, i.id, as_of).await?;
                let mut active: os_interface::ActiveModel = i.clone().into();
                active.to_date = Set(Some(as_of));
                tx.update(&i, active).await?;
                新しいインタフェース(
                    tx,
                    d.id,
                    &name,
                    &interface_type,
                    &aggregation_mode,
                    as_of,
                )
                .await?;

                if 閉じた > 0 {
                    report.push(Entry::new(
                        Outcome::Warning,
                        target,
                        format!(
                            "種別が変わったため開き直しました。乗っていた{閉じた}件（束ね・VLAN・IP・役割）も閉じています"
                        ),
                    ));
                } else {
                    report.push(Entry::new(Outcome::Updated, target, ""));
                }
            }
            None => {
                新しいインタフェース(
                    tx,
                    d.id,
                    &name,
                    &interface_type,
                    &aggregation_mode,
                    as_of,
                )
                .await?;
                report.push(Entry::new(Outcome::Created, target, ""));
            }
        }
    }

    Ok(report)
}

async fn 新しいインタフェース(
    tx: &AuditedTx,
    device_id: i32,
    name: &str,
    interface_type: &str,
    aggregation_mode: &Option<String>,
    as_of: DateTime<Utc>,
) -> Result<os_interface::Model, ImportError> {
    Ok(tx
        .insert(os_interface::ActiveModel {
            device_id: Set(device_id),
            interface_type: Set(interface_type.to_owned()),
            // 物理ポートとの紐付けは取込では扱わない（本モジュールの方針）
            part_instance_id: Set(None),
            port_slot_id: Set(None),
            os_interface_name: Set(name.to_owned()),
            aggregation_mode: Set(aggregation_mode.clone()),
            work_order_id: Set(None),
            from_date: Set(as_of),
            to_date: Set(None),
            ..Default::default()
        })
        .await?)
}

/// インタフェースに乗っているものを閉じる（8.5）。閉じた件数を返す。
async fn 依存を閉じる(
    tx: &AuditedTx,
    os_interface_id: i32,
    as_of: DateTime<Utc>,
) -> Result<usize, ImportError> {
    let mut 件数 = 0;

    let stacks = interface_stack::Entity::find()
        .filter(interface_stack::Column::ToDate.is_null())
        .filter(
            sea_orm::Condition::any()
                .add(interface_stack::Column::UpperInterfaceId.eq(os_interface_id))
                .add(interface_stack::Column::LowerInterfaceId.eq(os_interface_id)),
        )
        .all(tx.reader())
        .await?;
    for s in stacks {
        let mut active: interface_stack::ActiveModel = s.clone().into();
        active.to_date = Set(Some(as_of));
        tx.update(&s, active).await?;
        件数 += 1;
    }

    let vlans = interface_vlan::Entity::find()
        .filter(interface_vlan::Column::OsInterfaceId.eq(os_interface_id))
        .filter(interface_vlan::Column::ToDate.is_null())
        .all(tx.reader())
        .await?;
    for v in vlans {
        let mut active: interface_vlan::ActiveModel = v.clone().into();
        active.to_date = Set(Some(as_of));
        tx.update(&v, active).await?;
        件数 += 1;
    }

    let ips = ip_address::Entity::find()
        .filter(ip_address::Column::OsInterfaceId.eq(os_interface_id))
        .filter(ip_address::Column::ToDate.is_null())
        .all(tx.reader())
        .await?;
    for i in ips {
        let mut active: ip_address::ActiveModel = i.clone().into();
        active.to_date = Set(Some(as_of));
        tx.update(&i, active).await?;
        件数 += 1;
    }

    let roles = entity::interface_role::Entity::find()
        .filter(entity::interface_role::Column::OsInterfaceId.eq(os_interface_id))
        .filter(entity::interface_role::Column::ToDate.is_null())
        .all(tx.reader())
        .await?;
    for r in roles {
        let mut active: entity::interface_role::ActiveModel = r.clone().into();
        active.to_date = Set(Some(as_of));
        tx.update(&r, active).await?;
        件数 += 1;
    }

    Ok(件数)
}

// ---------------------------------------------------------------------------
// 束ね（8.5）
// ---------------------------------------------------------------------------

pub async fn 束ねを取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[StackRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let 機器 = このプロジェクトの機器(tx.reader(), project_id).await?;

    for row in rows {
        let target = format!(
            "{} {} ← {}",
            row.hostname.trim(),
            row.upper_interface.trim(),
            row.lower_interface.trim()
        );

        let d = match 機器を引く(tx, &機器, &row.hostname).await? {
            Ok(d) => d,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let upper = match 要るインタフェース(tx, d.id, &row.upper_interface).await? {
            Ok(i) => i,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };
        let lower = match 要るインタフェース(tx, d.id, &row.lower_interface).await? {
            Ok(i) => i,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        // **自分自身は束ねられない。**辿る表示が終わらなくなる
        if upper.id == lower.id {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "upper_interface と lower_interface が同じです",
            ));
            continue;
        }
        // **循環を作らせない**（8.5）
        if 上位を辿ると含む(tx, upper.id, lower.id).await? {
            report.push(Entry::new(
                Outcome::Error,
                target,
                "この組み合わせは束ねる関係の循環になります",
            ));
            continue;
        }

        let 既存 = interface_stack::Entity::find()
            .filter(interface_stack::Column::UpperInterfaceId.eq(upper.id))
            .filter(interface_stack::Column::LowerInterfaceId.eq(lower.id))
            .filter(interface_stack::Column::ToDate.is_null())
            .one(tx.reader())
            .await?;
        if 既存.is_some() {
            report.push(Entry::new(Outcome::Unchanged, target, ""));
            continue;
        }

        tx.insert(interface_stack::ActiveModel {
            upper_interface_id: Set(upper.id),
            lower_interface_id: Set(lower.id),
            from_date: Set(as_of),
            to_date: Set(None),
            ..Default::default()
        })
        .await?;
        report.push(Entry::new(Outcome::Created, target, ""));
    }

    Ok(report)
}

/// `lower` を辿って `upper` に行き着くか。**循環の検出に使う**（8.5）。
async fn 上位を辿ると含む(
    tx: &AuditedTx,
    upper: i32,
    lower: i32,
) -> Result<bool, ImportError> {
    let mut 見る = vec![lower];
    let mut 見た: Vec<i32> = Vec::new();

    while let Some(id) = 見る.pop() {
        if id == upper {
            return Ok(true);
        }
        if 見た.contains(&id) {
            continue;
        }
        見た.push(id);
        for s in interface_stack::Entity::find()
            .filter(interface_stack::Column::UpperInterfaceId.eq(id))
            .filter(interface_stack::Column::ToDate.is_null())
            .all(tx.reader())
            .await?
        {
            見る.push(s.lower_interface_id);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// インタフェースのVLAN（8.5）
// ---------------------------------------------------------------------------

pub async fn インタフェースのvlanを取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[InterfaceVlanRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let 機器 = このプロジェクトの機器(tx.reader(), project_id).await?;

    for row in rows {
        let target = format!(
            "{} {} VLAN{}",
            row.hostname.trim(),
            row.os_interface_name.trim(),
            row.vlan_tag.trim()
        );

        let d = match 機器を引く(tx, &機器, &row.hostname).await? {
            Ok(d) => d,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };
        let i = match 要るインタフェース(tx, d.id, &row.os_interface_name).await? {
            Ok(i) => i,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let vlan_id = match 解決するvlan(tx, &row.vlan_tag, &row.vlan_name).await? {
            Ok(Some(id)) => id,
            Ok(None) => {
                report.push(Entry::new(Outcome::Error, target, "vlan_tag が空です"));
                continue;
            }
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let tagging_mode = match 語彙(&row.tagging_mode, TAGGING_MODES, "") {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => {
                report.push(Entry::new(Outcome::Error, target, "tagging_mode が空です"));
                continue;
            }
            Err(理由) => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!("tagging_mode: {理由}"),
                ));
                continue;
            }
        };

        let 現在 = interface_vlan::Entity::find()
            .filter(interface_vlan::Column::OsInterfaceId.eq(i.id))
            .filter(interface_vlan::Column::VlanId.eq(vlan_id))
            .filter(interface_vlan::Column::ToDate.is_null())
            .one(tx.reader())
            .await?;

        match 現在 {
            Some(v) if v.tagging_mode == tagging_mode => {
                report.push(Entry::new(Outcome::Unchanged, target, ""));
                continue;
            }
            Some(v) => {
                // **閉じて開く**（4章）
                let mut active: interface_vlan::ActiveModel = v.clone().into();
                active.to_date = Set(Some(as_of));
                tx.update(&v, active).await?;
                report.push(Entry::new(Outcome::Updated, target, ""));
            }
            None => report.push(Entry::new(Outcome::Created, target, "")),
        }

        tx.insert(interface_vlan::ActiveModel {
            os_interface_id: Set(i.id),
            vlan_id: Set(vlan_id),
            tagging_mode: Set(tagging_mode),
            work_order_id: Set(None),
            from_date: Set(as_of),
            to_date: Set(None),
            ..Default::default()
        })
        .await?;
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// IPアドレス（14.2）
// ---------------------------------------------------------------------------

pub async fn ipアドレスを取り込む(
    tx: &AuditedTx,
    project_id: i32,
    rows: &[IpRow],
    as_of: DateTime<Utc>,
) -> Result<Report, ImportError> {
    let mut report = Report::default();
    let 機器 = このプロジェクトの機器(tx.reader(), project_id).await?;
    // **ファイル内の重複を先に見る。**DBの部分一意インデックスに当たる前に、
    // どの行同士がぶつかっているかを示す（14.2）
    let mut 既出: HashMap<(i32, String), usize> = HashMap::new();

    for (n, row) in rows.iter().enumerate() {
        let address = row.ip_address.trim().to_owned();
        let target = format!(
            "{} {} {address}",
            row.hostname.trim(),
            row.os_interface_name.trim()
        );

        let d = match 機器を引く(tx, &機器, &row.hostname).await? {
            Ok(d) => d,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };
        let i = match 要るインタフェース(tx, d.id, &row.os_interface_name).await? {
            Ok(i) => i,
            Err(理由) => {
                report.push(Entry::new(Outcome::Error, target, 理由));
                continue;
            }
        };

        let prefix = match row.prefix_length.trim().parse::<i32>() {
            Ok(p) if (0..=128).contains(&p) => p,
            _ => {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!("prefix_length「{}」が正しくありません", row.prefix_length),
                ));
                continue;
            }
        };

        let subnet_id = match 空ならnone(&row.subnet_cidr) {
            None => None,
            Some(cidr) => {
                let found = subnet::Entity::find()
                    .filter(subnet::Column::ProjectId.eq(project_id))
                    .filter(subnet::Column::Cidr.eq(cidr))
                    .one(tx.reader())
                    .await?;
                match found {
                    Some(s) => Some(s.id),
                    None => {
                        report.push(Entry::new(
                            Outcome::Error,
                            target,
                            format!("サブネット「{cidr}」がこのプロジェクトにありません"),
                        ));
                        continue;
                    }
                }
            }
        };

        if let Some(sid) = subnet_id {
            if let Some(前) = 既出.insert((sid, address.clone()), n) {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!("同じサブネットの同じIPが{}行目にもあります", 前 + 2),
                ));
                continue;
            }
        }

        let 現在 = ip_address::Entity::find()
            .filter(ip_address::Column::OsInterfaceId.eq(i.id))
            .filter(ip_address::Column::IpAddress.eq(&address))
            .filter(ip_address::Column::ToDate.is_null())
            .one(tx.reader())
            .await?;

        // **同一サブネット内の重複はDBが禁じている**（14.2）。素通りさせると
        // DBのエラーで落ちるので、利用者が直せる言葉でここで止める
        if let Some(sid) = subnet_id {
            let 他所 = ip_address::Entity::find()
                .filter(ip_address::Column::SubnetId.eq(sid))
                .filter(ip_address::Column::IpAddress.eq(&address))
                .filter(ip_address::Column::ToDate.is_null())
                .all(tx.reader())
                .await?;
            if 他所.iter().any(|x| x.os_interface_id != i.id) {
                report.push(Entry::new(
                    Outcome::Error,
                    target,
                    format!("{address} は同じサブネットの別のインタフェースで使われています"),
                ));
                continue;
            }
        }

        match 現在 {
            Some(x) if x.prefix_length == prefix && x.subnet_id == subnet_id => {
                report.push(Entry::new(Outcome::Unchanged, target, ""));
                continue;
            }
            Some(x) => {
                let mut active: ip_address::ActiveModel = x.clone().into();
                active.to_date = Set(Some(as_of));
                tx.update(&x, active).await?;
                report.push(Entry::new(Outcome::Updated, target, ""));
            }
            None => report.push(Entry::new(Outcome::Created, target, "")),
        }

        tx.insert(ip_address::ActiveModel {
            os_interface_id: Set(i.id),
            ip_address: Set(address),
            prefix_length: Set(prefix),
            subnet_id: Set(subnet_id),
            work_order_id: Set(None),
            from_date: Set(as_of),
            to_date: Set(None),
            ..Default::default()
        })
        .await?;
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// 参照の解決
// ---------------------------------------------------------------------------

async fn 機器を引く(
    tx: &AuditedTx,
    候補: &[device::Model],
    hostname: &str,
) -> Result<Result<device::Model, String>, ImportError> {
    let key = 機器キー {
        hostname: hostname.to_owned(),
        ..Default::default()
    };
    Ok(解決する機器(tx.reader(), 候補, &key).await?)
}

async fn 現在のインタフェース(
    tx: &AuditedTx,
    device_id: i32,
    name: &str,
) -> Result<Option<os_interface::Model>, ImportError> {
    Ok(os_interface::Entity::find()
        .filter(os_interface::Column::DeviceId.eq(device_id))
        .filter(os_interface::Column::OsInterfaceName.eq(name.trim()))
        .filter(os_interface::Column::ToDate.is_null())
        .one(tx.reader())
        .await?)
}

/// 現在有効なインタフェースを引く。**無ければ理由を返す。**
async fn 要るインタフェース(
    tx: &AuditedTx,
    device_id: i32,
    name: &str,
) -> Result<Result<os_interface::Model, String>, ImportError> {
    Ok(match 現在のインタフェース(tx, device_id, name).await? {
        Some(i) => Ok(i),
        None => Err(format!("インタフェース「{}」がありません", name.trim())),
    })
}

/// VLANをタグ（と名前）で引く（8.5）。
///
/// **同じタグが複数ありうる**ため、タグだけで複数該当したらエラーにする。
/// 黙って1つ目を選ぶと、別の拠点のVLANに繋ぎ込む。
async fn 解決するvlan(
    tx: &AuditedTx,
    tag: &str,
    name: &str,
) -> Result<Result<Option<i32>, String>, ImportError> {
    let Some(tag) = 空ならnone(tag) else {
        return Ok(Ok(None));
    };
    let Ok(tag) = tag.parse::<i32>() else {
        return Ok(Err(format!("vlan_tag「{tag}」を数値として読めません")));
    };

    let mut q = vlan::Entity::find().filter(vlan::Column::VlanTag.eq(tag));
    if let Some(name) = 空ならnone(name) {
        q = q.filter(vlan::Column::Name.eq(name));
    }
    let 候補 = q.all(tx.reader()).await?;

    Ok(match 候補.len() {
        1 => Ok(Some(候補[0].id)),
        0 => Err(format!("VLAN {tag} が見つかりません")),
        n => Err(format!(
            "VLAN {tag} に{n}件が該当します。vlan_name で指定してください"
        )),
    })
}
