//! 機器の詳細の「部品」と「ポートとIP」の節（設計書6.3、8.5、16.1、#255）。
//!
//! # 節ごとにその場で編集する（16.1）
//!
//! 機器の詳細を、見る画面と直す画面に分けない。節はそれぞれ1つのフォームで、
//! 節ごとに保存する。**開閉はチェックボックスと `:has()` で行う**（不変条件9）。
//! 検証エラーで描き直すときは、送られてきた値を入力欄へ戻し、その節を編集の
//! まま返す。
//!
//! # 部品は「載せる」操作で台帳に載る（6.3）
//!
//! 表の行は構成（`CONFIGURATION_PART`）から描くだけで、`PART_INSTANCE` を作る
//! のは人が「載せる」に印を付けて保存したときである。**構成から自動では
//! 作らない。**シリアル番号は空でよい。
//!
//! # 交換はシリアル番号の書き換えで記録する（6.3）
//!
//! 同じ型番への交換では行を作り直さない。`OS_INTERFACE` は同じ `PART_INSTANCE`
//! を指したままになる。書き換えた番号が倉庫プロジェクトの予備品と一致したら、
//! 予備品の所在を `Used` にする（`status` には持たない）。
//!
//! # ポートとIPは、送られてきた行だけを「あるべき状態」として合わせる（8.5）
//!
//! 履歴テーブルなので、変わった行だけを閉じて開く。**名前・種別・束ね方が
//! 変わったインターフェースは、閉じて開き直す**（属性を書き換える経路を
//! 持たない。取込と同じ）。フォームに無い行には触れない。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    configuration_part, device, interface_role, interface_stack, interface_vlan, ip_address,
    os_interface, part_catalog, part_instance, part_instance_location, part_port_slot, subnet,
    vlan,
};
use sea_orm::{ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::auth::middleware::CurrentUser;
use crate::device_state;
use crate::error::{AppError, AppResult};
use crate::part_location::{self, DEVICE, MOUNT_CONTAINER, PROJECT, USED};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::正規化;
use crate::server::view::{容体の表示, 容体の選択肢, Choice, Locale};
use crate::server::AppState;

/// 閉じた語彙（`vocabularies.md`、設計書8.5）。
const INTERFACE_TYPES: &[&str] = &["Physical", "Bond", "Vlan", "Svi", "Bridge", "Virtual"];
const AGGREGATION_MODES: &[&str] = &["LACP", "Static", "ActiveBackup"];
const PHYSICAL: &str = "Physical";
const BOND: &str = "Bond";
const UNTAGGED: &str = "Untagged";
const TAGGED: &str = "Tagged";
const NETWORK: &str = "Network";

/// `INTERFACE_ROLE.role` の代表例（8.5）。開いた語彙なので候補として出すだけ。
const ROLES: &[&str] = &[
    "Service",
    "Management",
    "Backup",
    "Storage",
    "ClusterInterconnect",
    "vMotion",
];

/// これより多い部品は、畳んで出す。
const 畳む数: usize = 8;
/// 論理インターフェースを足すための空の行の数。
const 空の行: usize = 2;

pub(super) const PARTS: &str = "parts";
pub(super) const PORTS: &str = "ports";

fn 内部(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(anyhow::anyhow!(e))
}

/// 検証エラーで描き直す節。送られてきた値を入力欄へ戻す。
pub(super) struct 描き直し {
    pub 節: &'static str,
    pub error: String,
    pub 入力: HashMap<String, String>,
}

impl 描き直し {
    fn 値(&self, 節: &str, key: &str) -> Option<&str> {
        (self.節 == 節)
            .then(|| self.入力.get(key).map(String::as_str))
            .flatten()
    }
    fn ある(&self, 節: &str, key: &str) -> bool {
        self.節 == 節 && self.入力.contains_key(key)
    }
}

fn 対へ(pairs: Vec<(String, String)>) -> HashMap<String, String> {
    pairs.into_iter().collect()
}

// ===========================================================================
// 部品
// ===========================================================================

pub(super) struct PartUnit {
    pub no: usize,
    /// 台帳に載っていれば、その部品。
    pub part_id: Option<i32>,
    /// 入力欄の名前の末尾。載っている行は `p{id}`、載っていない行は `n{型}_{k}`。
    pub key: String,
    pub serial: String,
    pub checked: bool,
    pub health: String,
    pub failed: bool,
}

pub(super) struct PartGroup {
    pub catalog_id: i32,
    pub category: String,
    pub model: String,
    /// `25G×2` のような、ネットワークポートの要約。無ければ空。
    pub ports: String,
    pub quantity: usize,
    pub registered: usize,
    pub folded: bool,
    pub all_checked: bool,
    pub units: Vec<PartUnit>,
}

pub(super) struct PartsView {
    pub groups: Vec<PartGroup>,
    pub quantity: usize,
    pub registered: usize,
    pub editing: bool,
    pub error: Option<String>,
    pub healths: Vec<Choice>,
    pub t: PartsText,
}

pub(super) struct PartsText {
    pub title: String,
    pub lead: String,
    pub edit: String,
    pub cancel: String,
    pub save: String,
    pub category: String,
    pub part: String,
    pub ports: String,
    pub quantity: String,
    pub ledger: String,
    pub serial: String,
    pub health: String,
    pub empty: String,
    pub no_serial: String,
    pub not_registered: String,
    pub registered: String,
    pub register: String,
    pub register_all: String,
    pub failed: String,
    pub count: String,
    pub hint_register: String,
    pub hint_replace: String,
}

/// 機器に今載っている部品と、その型。
struct 載っている部品 {
    part: part_instance::Model,
    catalog: part_catalog::Model,
}

async fn 載っている部品を引く<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Vec<載っている部品>> {
    let ids: Vec<i32> = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(DEVICE))
        .filter(part_instance_location::Column::LocationId.eq(device_id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .map(|l| l.part_instance_id)
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let parts = part_instance::Entity::find()
        .filter(part_instance::Column::Id.is_in(ids))
        .order_by_asc(part_instance::Column::Id)
        .all(db)
        .await
        .map_err(内部)?;
    let 型 = 型を引く(db, parts.iter().map(|p| p.part_catalog_id)).await?;
    Ok(parts
        .into_iter()
        .filter_map(|part| {
            let catalog = 型.get(&part.part_catalog_id)?.clone();
            Some(載っている部品 { part, catalog })
        })
        .collect())
}

async fn 型を引く<C: ConnectionTrait>(
    db: &C,
    ids: impl Iterator<Item = i32>,
) -> AppResult<HashMap<i32, part_catalog::Model>> {
    let mut ids: Vec<i32> = ids.collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(part_catalog::Entity::find()
        .filter(part_catalog::Column::Id.is_in(ids))
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .map(|c| (c.id, c))
        .collect())
}

/// 型ごとのネットワークポート。ラベルの順ではなく登録の順に並べる。
async fn ネットワークポート<C: ConnectionTrait>(
    db: &C,
    catalog_ids: impl Iterator<Item = i32>,
) -> AppResult<HashMap<i32, Vec<part_port_slot::Model>>> {
    let mut ids: Vec<i32> = catalog_ids.collect();
    ids.sort_unstable();
    ids.dedup();
    let mut out: HashMap<i32, Vec<part_port_slot::Model>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    for s in part_port_slot::Entity::find()
        .filter(part_port_slot::Column::PartCatalogId.is_in(ids))
        .filter(part_port_slot::Column::PortKind.eq(NETWORK))
        .order_by_asc(part_port_slot::Column::Id)
        .all(db)
        .await
        .map_err(内部)?
    {
        out.entry(s.part_catalog_id).or_default().push(s);
    }
    Ok(out)
}

/// 構成が含む部品（型と数量）。構成を持たない機器では空。
async fn 構成の部品<C: ConnectionTrait>(
    db: &C,
    d: &device::Model,
) -> AppResult<Vec<configuration_part::Model>> {
    let Some(configuration_id) = d.configuration_id else {
        return Ok(Vec::new());
    };
    configuration_part::Entity::find()
        .filter(configuration_part::Column::ConfigurationId.eq(configuration_id))
        .order_by_asc(configuration_part::Column::Id)
        .all(db)
        .await
        .map_err(内部)
}

fn ポートの要約(ports: &[part_port_slot::Model]) -> String {
    if ports.is_empty() {
        return String::new();
    }
    let 速度: HashSet<&str> = ports
        .iter()
        .filter_map(|p| p.port_speed.as_deref())
        .collect();
    match (速度.len(), ports[0].port_speed.as_deref()) {
        (1, Some(s)) => format!("{s}×{}", ports.len()),
        _ => format!("×{}", ports.len()),
    }
}

/// 表の行の並び。構成の順に並べ、構成に無い型（部品の画面や取込で載せたもの）を
/// 後ろに足す。`(型, 構成の数量)`。
fn 行の並び(
    構成: &[configuration_part::Model], 載: &[載っている部品]
) -> Vec<(i32, usize)> {
    let mut out: Vec<(i32, usize)> = Vec::new();
    for c in 構成 {
        let 数 = usize::try_from(c.quantity).unwrap_or(0);
        match out.iter_mut().find(|(id, _)| *id == c.part_catalog_id) {
            Some((_, q)) => *q += 数,
            None => out.push((c.part_catalog_id, 数)),
        }
    }
    for p in 載 {
        if !out.iter().any(|(id, _)| *id == p.catalog.id) {
            out.push((p.catalog.id, 0));
        }
    }
    out
}

pub(super) async fn 部品の節(
    state: &AppState,
    d: &device::Model,
    l: &str,
    再: Option<&描き直し>,
) -> AppResult<PartsView> {
    let db = &state.db;
    let 載 = 載っている部品を引く(db, d.id).await?;
    let 構成 = 構成の部品(db, d).await?;
    let 並び = 行の並び(&構成, &載);
    let 型 = 型を引く(db, 並び.iter().map(|(id, _)| *id)).await?;
    let ポート = ネットワークポート(db, 並び.iter().map(|(id, _)| *id)).await?;

    let 入力 = |key: &str| 再.and_then(|r| r.値(PARTS, key));
    let 印 = |key: &str| 再.is_some_and(|r| r.ある(PARTS, key));

    let mut groups = Vec::new();
    for (catalog_id, 構成数) in 並び {
        let Some(catalog) = 型.get(&catalog_id) else {
            continue;
        };
        let 現物: Vec<&載っている部品> = 載.iter().filter(|p| p.catalog.id == catalog_id).collect();
        let quantity = 構成数.max(現物.len());

        let mut units = Vec::new();
        for (i, p) in 現物.iter().enumerate() {
            let key = format!("p{}", p.part.id);
            units.push(PartUnit {
                no: i + 1,
                part_id: Some(p.part.id),
                serial: 入力(&format!("serial_{key}"))
                    .map(str::to_owned)
                    .unwrap_or_else(|| p.part.serial_number.clone().unwrap_or_default()),
                checked: true,
                health: 入力(&format!("health_{key}"))
                    .map(str::to_owned)
                    .unwrap_or_else(|| p.part.health.clone()),
                failed: p.part.health == device_state::FAILED,
                key,
            });
        }
        for k in 0..quantity - 現物.len() {
            let key = format!("n{catalog_id}_{k}");
            units.push(PartUnit {
                no: 現物.len() + k + 1,
                part_id: None,
                serial: 入力(&format!("serial_{key}"))
                    .unwrap_or_default()
                    .to_owned(),
                checked: 印(&format!("register_{key}")),
                health: String::new(),
                failed: false,
                key,
            });
        }

        groups.push(PartGroup {
            catalog_id,
            category: catalog.category.clone(),
            model: catalog.part_number.clone(),
            ports: ポートの要約(ポート.get(&catalog_id).map(Vec::as_slice).unwrap_or(&[])),
            quantity,
            registered: 現物.len(),
            folded: quantity > 畳む数,
            all_checked: 印(&format!("register_all_{catalog_id}")),
            units,
        });
    }

    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    Ok(PartsView {
        quantity: groups.iter().map(|g| g.quantity).sum(),
        registered: groups.iter().map(|g| g.registered).sum(),
        groups,
        editing: 再.is_some_and(|r| r.節 == PARTS),
        error: 再.filter(|r| r.節 == PARTS).map(|r| r.error.clone()),
        healths: 容体の選択肢(device_state::HEALTHS, l),
        t: PartsText {
            title: t("device_sections.parts_title"),
            lead: t("device_sections.parts_lead"),
            edit: t("device_sections.edit"),
            cancel: t("device_sections.cancel"),
            save: t("device_sections.save"),
            category: t("device_sections.category"),
            part: t("device_sections.part"),
            ports: t("device_sections.port_count"),
            quantity: t("device_sections.quantity"),
            ledger: t("device_sections.ledger"),
            serial: t("devices.serial_number"),
            health: t("devices.health"),
            empty: t("device_sections.parts_empty"),
            no_serial: t("device_sections.no_serial"),
            not_registered: t("device_sections.not_registered"),
            registered: t("device_sections.registered"),
            register: t("device_sections.register"),
            register_all: t("device_sections.register_all"),
            failed: 容体の表示(device_state::FAILED, l),
            count: t("device_sections.count"),
            hint_register: t("device_sections.hint_register"),
            hint_replace: t("device_sections.hint_replace"),
        },
    })
}

/// 部品の節を保存する。
///
/// - 載っている行：シリアル番号の書き換え（交換）と、故障の有無
/// - 載っていない行：「載せる」の印か、シリアル番号の入力があれば台帳に載せる
pub async fn save_parts(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let d = super::device::編集する機器(&state, &current, project_id, device_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let db = &state.db;
    let 入力 = 対へ(pairs);

    macro_rules! 戻る {
        ($($arg:tt)*) => {
            return super::device::詳細を描く(
                &state,
                &current,
                project_id,
                device_id,
                Some(描き直し {
                    節: PARTS,
                    error: rust_i18n::t!($($arg)*, locale = l).to_string(),
                    入力,
                }),
            )
            .await
        };
    }

    // **読み取りはトランザクションを開く前に済ませる**（SQLiteで自分の書き込みロックを待つ）
    let 載 = 載っている部品を引く(db, d.id).await?;
    let 構成 = 構成の部品(db, &d).await?;
    let 並び = 行の並び(&構成, &載);
    let 型 = 型を引く(db, 並び.iter().map(|(id, _)| *id)).await?;

    // 保存後にこの機器に載っている部品の `(型, シリアル番号)`。同じ型の中で
    // 同じ番号を2つ持たせない
    let mut 番号: Vec<(i32, String)> = Vec::new();
    let mut 書き換え: Vec<(&載っている部品, Option<String>, String)> = Vec::new();
    for p in &載 {
        let key = format!("p{}", p.part.id);
        let serial = match 入力.get(&format!("serial_{key}")) {
            Some(v) => 空ならnone(v),
            None => p.part.serial_number.clone(),
        };
        let health = match 入力.get(&format!("health_{key}")) {
            Some(v) => match device_state::HEALTHS.iter().find(|h| **h == v.trim()) {
                Some(h) => (*h).to_owned(),
                None => 戻る!("devices.part_health_invalid"),
            },
            None => p.part.health.clone(),
        };
        if let Some(s) = &serial {
            番号.push((p.catalog.id, s.clone()));
        }
        if serial != p.part.serial_number || health != p.part.health {
            書き換え.push((p, serial, health));
        }
    }

    let mut 載せる: Vec<(i32, Option<String>)> = Vec::new();
    for (catalog_id, 構成数) in &並び {
        let 現物 = 載.iter().filter(|p| p.catalog.id == *catalog_id).count();
        let 全部 = 入力.contains_key(&format!("register_all_{catalog_id}"));
        for k in 0..構成数.saturating_sub(現物) {
            let key = format!("n{catalog_id}_{k}");
            let serial = 入力
                .get(&format!("serial_{key}"))
                .and_then(|v| 空ならnone(v));
            // **シリアル番号を入れた行は、印が無くても載せる**
            if 全部 || serial.is_some() || 入力.contains_key(&format!("register_{key}")) {
                if let Some(s) = &serial {
                    番号.push((*catalog_id, s.clone()));
                }
                載せる.push((*catalog_id, serial));
            }
        }
    }

    let mut 見た = HashSet::new();
    if 番号.iter().any(|組| !見た.insert(組)) {
        戻る!("device_sections.error_serial_repeated");
    }

    // 新しく入った番号を、他の部品と突き合わせる。倉庫プロジェクトの同じ型の
    // 予備品なら使ったものとし、それ以外に同じ番号があれば拒否する
    let この機器の部品: HashSet<i32> = 載.iter().map(|p| p.part.id).collect();
    let mut 使う予備品: Vec<part_instance_location::Model> = Vec::new();
    let 新しい番号 = 書き換え
        .iter()
        .filter(|(p, serial, _)| *serial != p.part.serial_number)
        .filter_map(|(p, serial, _)| serial.as_ref().map(|s| (&p.catalog, s)))
        .chain(
            載せる
                .iter()
                .filter_map(|(c, serial)| Some((型.get(c)?, serial.as_ref()?))),
        );
    for (catalog, serial) in 新しい番号 {
        match 同じ番号を調べる(db, catalog, serial, &この機器の部品).await? {
            同じ番号::無い => {}
            同じ番号::予備品(所在) => {
                if !使う予備品.iter().any(|s| s.id == 所在.id) {
                    使う予備品.push(所在);
                }
            }
            同じ番号::他にある => 戻る!("part_instances.error_serial_taken"),
        }
    }

    if 書き換え.is_empty() && 載せる.is_empty() {
        return Ok(戻り先(project_id, device_id, PARTS));
    }

    let now = Utc::now();
    let tx = AuditedTx::begin(db, Actor::User(current.user.id))
        .await
        .map_err(内部)?;
    for (p, serial, health) in 書き換え {
        let mut active: part_instance::ActiveModel = p.part.clone().into();
        active.serial_number = Set(serial);
        active.health = Set(health);
        active.updated_at = Set(now);
        tx.update(&p.part, active).await.map_err(内部)?;
    }
    for (catalog_id, serial) in 載せる {
        let part = tx
            .insert(part_instance::ActiveModel {
                part_catalog_id: Set(catalog_id),
                serial_number: Set(serial),
                // 機器と同じ段階にあるものとして載せる。表には出さない（6.3）
                status: Set(d.status.clone()),
                health: Set(device_state::OK.to_owned()),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await
            .map_err(内部)?;
        tx.insert(part_instance_location::ActiveModel {
            part_instance_id: Set(part.id),
            location_type: Set(DEVICE.to_owned()),
            location_id: Set(Some(d.id)),
            chassis_slot_id: Set(None),
            work_order_id: Set(None),
            from_date: Set(now),
            to_date: Set(None),
            ..Default::default()
        })
        .await
        .map_err(内部)?;
    }
    // **予備品は消さず、所在を `Used` にする**（6.3）。`location_id` は使った先の機器
    for 所在 in 使う予備品 {
        tx.update(
            &所在,
            part_instance_location::ActiveModel {
                id: Set(所在.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
        tx.insert(part_instance_location::ActiveModel {
            part_instance_id: Set(所在.part_instance_id),
            location_type: Set(USED.to_owned()),
            location_id: Set(Some(d.id)),
            chassis_slot_id: Set(None),
            work_order_id: Set(None),
            from_date: Set(now),
            to_date: Set(None),
            ..Default::default()
        })
        .await
        .map_err(内部)?;
    }
    tx.commit().await.map_err(内部)?;

    Ok(戻り先(project_id, device_id, PARTS))
}

enum 同じ番号 {
    無い,
    /// 倉庫プロジェクトにある同じ型の予備品。現行の所在の行。
    予備品(part_instance_location::Model),
    他にある,
}

/// 同じベンダーで同じシリアル番号の部品が、他にあるか（23.5、6.3）。
///
/// - 使用済み・廃棄済みの部品は数えない
/// - この機器に載っている部品は数えない。同じ型の重複はフォームの中で見ており、
///   **型の違う部品が同じ番号を持つのは、オンボードのNICがマザーボードの番号を
///   共有する場合である**（6.3）
async fn 同じ番号を調べる<C: ConnectionTrait>(
    db: &C,
    catalog: &part_catalog::Model,
    serial: &str,
    この機器の部品: &HashSet<i32>,
) -> AppResult<同じ番号> {
    let 候補: Vec<part_instance::Model> = part_instance::Entity::find()
        .filter(part_instance::Column::SerialNumber.eq(serial))
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .filter(|p| !この機器の部品.contains(&p.id))
        .collect();
    if 候補.is_empty() {
        return Ok(同じ番号::無い);
    }
    let 型 = 型を引く(db, 候補.iter().map(|p| p.part_catalog_id)).await?;
    let 所在 = part_location::現行の所在(db, 候補.iter().map(|p| p.id))
        .await
        .map_err(内部)?;

    let mut 予備品 = None;
    for p in 候補 {
        if 型.get(&p.part_catalog_id).map(|c| c.vendor_id) != Some(catalog.vendor_id) {
            continue;
        }
        let Some(今) = 所在.get(&p.id) else {
            return Ok(同じ番号::他にある);
        };
        if part_location::使い終えた(&今.location_type) {
            continue;
        }
        let 棚にある = 今.location_type == PROJECT || 今.location_type == MOUNT_CONTAINER;
        let 倉庫 = match part_location::部品の現在のプロジェクト(db, p.id)
            .await
            .map_err(内部)?
        {
            Some(pid) => crate::setting::倉庫プロジェクトか(db, pid)
                .await
                .map_err(内部)?,
            None => false,
        };
        if p.part_catalog_id == catalog.id && 棚にある && 倉庫 && 予備品.is_none() {
            予備品 = Some(今.clone());
        } else {
            return Ok(同じ番号::他にある);
        }
    }
    Ok(match 予備品 {
        Some(s) => 同じ番号::予備品(s),
        None => 同じ番号::無い,
    })
}

fn 空ならnone(value: &str) -> Option<String> {
    let v = 正規化(value);
    (!v.is_empty()).then_some(v)
}

fn 戻り先(project_id: i32, device_id: i32, 節: &str) -> Response {
    Redirect::to(&format!("/projects/{project_id}/devices/{device_id}#{節}")).into_response()
}

// ===========================================================================
// ポートとIP
// ===========================================================================

pub(super) struct PortRow {
    pub i: usize,
    /// 物理ポートの行か。`false` は論理インターフェース（と、ポートの分からない Physical）。
    pub is_port: bool,
    /// 足すための空の行か。
    pub is_new: bool,
    pub port_label: String,
    pub serial: String,
    /// 現行のインターフェース。無ければ空。
    pub id: String,
    /// `part_instance_id:port_slot_id`。物理ポートの行だけ。
    pub port: String,
    pub name: String,
    /// `Bond:LACP` のように、種別と束ね方を1つにした値。
    pub kind: String,
    /// 物理ポートの行では束ね先（上位）、それ以外では下位。カンマ区切りの名前。
    pub stack: String,
    /// 束ねている物理ポート。物理ポートの行の束ね先から決まるので、表示だけ。
    pub members: String,
    pub vlan: String,
    pub ip: String,
    pub role: String,
    pub peer: String,
    pub remove: bool,
}

pub(super) struct PortsView {
    pub rows: Vec<PortRow>,
    pub physical: usize,
    pub interfaces: usize,
    pub ips: usize,
    pub editing: bool,
    pub error: Option<String>,
    pub kinds: Vec<String>,
    pub roles: Vec<&'static str>,
    pub names: Vec<String>,
    pub vlans: Vec<String>,
    pub t: PortsText,
}

pub(super) struct PortsText {
    pub title: String,
    pub lead: String,
    pub edit: String,
    pub cancel: String,
    pub save: String,
    pub port: String,
    pub serial: String,
    pub interface: String,
    pub kind: String,
    pub stack: String,
    pub vlan: String,
    pub ip: String,
    pub role: String,
    pub peer: String,
    pub remove: String,
    pub physical: String,
    pub logical: String,
    pub add: String,
    pub unused: String,
    pub no_serial: String,
    pub empty: String,
    pub count: String,
    pub hint_ports: String,
    pub hint_remove: String,
    pub hint_stack: String,
    pub hint_vlan_ip: String,
}

/// この機器の物理ポート（台帳に載せたNICのネットワークポート）。
struct 物理ポート {
    part_id: i32,
    slot_id: i32,
    label: String,
    serial: String,
}

async fn 物理ポートを並べる<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Vec<物理ポート>> {
    let 載 = 載っている部品を引く(db, device_id).await?;
    let ポート = ネットワークポート(db, 載.iter().map(|p| p.catalog.id)).await?;

    let mut 枚数: HashMap<i32, usize> = HashMap::new();
    for p in &載 {
        *枚数.entry(p.catalog.id).or_default() += 1;
    }
    let mut 何枚目: HashMap<i32, usize> = HashMap::new();
    let mut out = Vec::new();
    for p in &載 {
        let Some(slots) = ポート.get(&p.catalog.id) else {
            continue;
        };
        let n = 何枚目.entry(p.catalog.id).or_default();
        *n += 1;
        // **同じ型が2枚以上あるときだけ、台帳の行ごとに番号を付ける**（16.1）
        let 型番 = match 枚数.get(&p.catalog.id) {
            Some(数) if *数 > 1 => format!("{} #{}", p.catalog.part_number, n),
            _ => p.catalog.part_number.clone(),
        };
        for s in slots {
            out.push(物理ポート {
                part_id: p.part.id,
                slot_id: s.id,
                label: format!("{型番} {}", s.port_label),
                serial: p.part.serial_number.clone().unwrap_or_default(),
            });
        }
    }
    Ok(out)
}

/// 機器の現在のインターフェースと、その上に載っているもの。
struct 現状 {
    interfaces: Vec<os_interface::Model>,
    vlans: HashMap<i32, Vec<(vlan::Model, interface_vlan::Model)>>,
    roles: HashMap<i32, Vec<interface_role::Model>>,
    ips: HashMap<i32, Vec<ip_address::Model>>,
    /// この機器のインターフェースが上位・下位のどちらかにある、現行の束ね。
    stacks: Vec<interface_stack::Model>,
}

impl 現状 {
    fn 名前(&self, id: i32) -> Option<&str> {
        self.interfaces
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.os_interface_name.as_str())
    }
    fn ポート行か(&self, id: i32) -> bool {
        self.interfaces
            .iter()
            .any(|i| i.id == id && ポートを持つ(i))
    }
}

fn ポートを持つ(i: &os_interface::Model) -> bool {
    i.part_instance_id.is_some() && i.port_slot_id.is_some()
}

async fn 現状を引く<C: ConnectionTrait>(db: &C, device_id: i32) -> AppResult<現状> {
    let interfaces = os_interface::Entity::find()
        .filter(os_interface::Column::DeviceId.eq(device_id))
        .filter(os_interface::Column::ToDate.is_null())
        .order_by_asc(os_interface::Column::OsInterfaceName)
        .all(db)
        .await
        .map_err(内部)?;
    let ids: Vec<i32> = interfaces.iter().map(|i| i.id).collect();
    let mut out = 現状 {
        interfaces,
        vlans: HashMap::new(),
        roles: HashMap::new(),
        ips: HashMap::new(),
        stacks: Vec::new(),
    };
    if ids.is_empty() {
        return Ok(out);
    }

    let links = interface_vlan::Entity::find()
        .filter(interface_vlan::Column::OsInterfaceId.is_in(ids.clone()))
        .filter(interface_vlan::Column::ToDate.is_null())
        .order_by_asc(interface_vlan::Column::Id)
        .all(db)
        .await
        .map_err(内部)?;
    let vlan表: HashMap<i32, vlan::Model> = if links.is_empty() {
        HashMap::new()
    } else {
        vlan::Entity::find()
            .filter(vlan::Column::Id.is_in(links.iter().map(|l| l.vlan_id).collect::<Vec<_>>()))
            .all(db)
            .await
            .map_err(内部)?
            .into_iter()
            .map(|v| (v.id, v))
            .collect()
    };
    for link in links {
        if let Some(v) = vlan表.get(&link.vlan_id) {
            out.vlans
                .entry(link.os_interface_id)
                .or_default()
                .push((v.clone(), link));
        }
    }
    for r in interface_role::Entity::find()
        .filter(interface_role::Column::OsInterfaceId.is_in(ids.clone()))
        .filter(interface_role::Column::ToDate.is_null())
        .order_by_asc(interface_role::Column::Id)
        .all(db)
        .await
        .map_err(内部)?
    {
        out.roles.entry(r.os_interface_id).or_default().push(r);
    }
    for ip in ip_address::Entity::find()
        .filter(ip_address::Column::OsInterfaceId.is_in(ids.clone()))
        .filter(ip_address::Column::ToDate.is_null())
        .order_by_asc(ip_address::Column::Id)
        .all(db)
        .await
        .map_err(内部)?
    {
        out.ips.entry(ip.os_interface_id).or_default().push(ip);
    }
    out.stacks = interface_stack::Entity::find()
        .filter(
            Condition::any()
                .add(interface_stack::Column::UpperInterfaceId.is_in(ids.clone()))
                .add(interface_stack::Column::LowerInterfaceId.is_in(ids)),
        )
        .filter(interface_stack::Column::ToDate.is_null())
        .order_by_asc(interface_stack::Column::Id)
        .all(db)
        .await
        .map_err(内部)?;
    Ok(out)
}

fn 種別の値(i: &os_interface::Model) -> String {
    match (&i.interface_type, &i.aggregation_mode) {
        (t, Some(m)) if t == BOND => format!("{BOND}:{m}"),
        (t, _) => t.clone(),
    }
}

/// 選べる種別。`Bond` は束ね方ごとに並べる。
fn 種別の候補() -> Vec<String> {
    let mut out = Vec::new();
    for t in INTERFACE_TYPES {
        if *t == BOND {
            out.extend(AGGREGATION_MODES.iter().map(|m| format!("{BOND}:{m}")));
        }
        out.push((*t).to_owned());
    }
    out
}

fn vlanの表示(list: &[(vlan::Model, interface_vlan::Model)]) -> String {
    list.iter()
        .map(|(v, link)| match link.tagging_mode.as_str() {
            TAGGED => format!("{}T", v.vlan_tag),
            _ => v.vlan_tag.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) async fn ポートの節(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    d: &device::Model,
    l: &str,
    再: Option<&描き直し>,
) -> AppResult<PortsView> {
    let db = &state.db;
    let ポート = 物理ポートを並べる(db, d.id).await?;
    let 今 = 現状を引く(db, d.id).await?;

    let 上位 = |id: i32| -> String {
        今.stacks
            .iter()
            .filter(|s| s.lower_interface_id == id)
            .filter_map(|s| 今.名前(s.upper_interface_id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // 下位は、物理ポートの行とそれ以外に分ける。前者は表示だけ
    let 下位 = |id: i32, ポート行: bool| -> String {
        今.stacks
            .iter()
            .filter(|s| s.upper_interface_id == id)
            .filter(|s| 今.ポート行か(s.lower_interface_id) == ポート行)
            .filter_map(|s| 今.名前(s.lower_interface_id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let 既存の行 = |i: &os_interface::Model, 行: &mut PortRow| {
        行.id = i.id.to_string();
        行.name = i.os_interface_name.clone();
        行.kind = 種別の値(i);
        行.vlan = vlanの表示(今.vlans.get(&i.id).map(Vec::as_slice).unwrap_or(&[]));
        行.ip = 今
            .ips
            .get(&i.id)
            .map(|v| {
                v.iter()
                    .map(|ip| format!("{}/{}", ip.ip_address, ip.prefix_length))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        行.role = 今
            .roles
            .get(&i.id)
            .map(|v| {
                v.iter()
                    .map(|r| r.role.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
    };
    let 空の = |is_port: bool| PortRow {
        i: 0,
        is_port,
        is_new: false,
        port_label: String::new(),
        serial: String::new(),
        id: String::new(),
        port: String::new(),
        name: String::new(),
        kind: if is_port {
            PHYSICAL.to_owned()
        } else {
            String::new()
        },
        stack: String::new(),
        members: String::new(),
        vlan: String::new(),
        ip: String::new(),
        role: String::new(),
        peer: String::new(),
        remove: false,
    };

    let mut rows = Vec::new();
    let mut 出した = HashSet::new();
    for p in &ポート {
        let mut 行 = 空の(true);
        行.port_label = p.label.clone();
        行.serial = p.serial.clone();
        行.port = format!("{}:{}", p.part_id, p.slot_id);
        if let Some(i) = 今
            .interfaces
            .iter()
            .find(|i| i.part_instance_id == Some(p.part_id) && i.port_slot_id == Some(p.slot_id))
        {
            既存の行(i, &mut 行);
            行.stack = 上位(i.id);
            出した.insert(i.id);
        }
        行.peer = super::connection::ポートの接続先(
            db,
            &current.user,
            project_id,
            p.part_id,
            p.slot_id,
            l,
        )
        .await?;
        rows.push(行);
    }
    let physical = rows.len();
    for i in 今.interfaces.iter().filter(|i| !出した.contains(&i.id)) {
        let mut 行 = 空の(false);
        既存の行(i, &mut 行);
        // 部品が外されたなどで、ポートを持つのに上の並びに出なかった行。束ね先を見せる
        if ポートを持つ(i) {
            行.is_port = true;
            行.port = format!(
                "{}:{}",
                i.part_instance_id.unwrap_or_default(),
                i.port_slot_id.unwrap_or_default()
            );
            行.stack = 上位(i.id);
        } else {
            行.stack = 下位(i.id, false);
            行.members = 下位(i.id, true);
        }
        rows.push(行);
    }
    let interfaces = 今.interfaces.len();
    for _ in 0..空の行 {
        let mut 行 = 空の(false);
        行.is_new = true;
        rows.push(行);
    }

    // 送られてきた値を戻す。行の番号は、同じ状態から描く限り同じになる
    let 入力 = |key: String| 再.and_then(|r| r.値(PORTS, &key).map(str::to_owned));
    for (n, 行) in rows.iter_mut().enumerate() {
        行.i = n;
        if 再.is_none_or(|r| r.節 != PORTS) {
            continue;
        }
        if let Some(v) = 入力(format!("name_{n}")) {
            行.name = v;
        }
        if let (false, Some(v)) = (行.is_port, 入力(format!("kind_{n}"))) {
            行.kind = v;
        }
        if let Some(v) = 入力(format!("stack_{n}")) {
            行.stack = v;
        }
        if let Some(v) = 入力(format!("vlan_{n}")) {
            行.vlan = v;
        }
        if let Some(v) = 入力(format!("ip_{n}")) {
            行.ip = v;
        }
        if let Some(v) = 入力(format!("role_{n}")) {
            行.role = v;
        }
        行.remove = 再.is_some_and(|r| r.ある(PORTS, &format!("remove_{n}")));
    }

    let 候補のvlan = vlan::Entity::find()
        .filter(vlan::Column::RetiredAt.is_null())
        .order_by_asc(vlan::Column::VlanTag)
        .all(db)
        .await
        .map_err(内部)?;
    let mut vlans = Vec::new();
    for v in &候補のvlan {
        vlans.push(v.vlan_tag.to_string());
        vlans.push(format!("{}T", v.vlan_tag));
    }
    vlans.dedup();

    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    Ok(PortsView {
        physical,
        interfaces,
        ips: 今.ips.values().map(Vec::len).sum(),
        names: 今
            .interfaces
            .iter()
            .map(|i| i.os_interface_name.clone())
            .collect(),
        rows,
        editing: 再.is_some_and(|r| r.節 == PORTS),
        error: 再.filter(|r| r.節 == PORTS).map(|r| r.error.clone()),
        kinds: 種別の候補(),
        roles: ROLES.to_vec(),
        vlans,
        t: PortsText {
            title: t("device_sections.ports_title"),
            lead: t("network.interfaces_lead"),
            edit: t("device_sections.edit"),
            cancel: t("device_sections.cancel"),
            save: t("device_sections.save"),
            port: t("network.port"),
            serial: t("devices.serial_number"),
            interface: t("network.interface"),
            kind: t("network.interface_type"),
            stack: t("device_sections.stack"),
            vlan: t("vlans.title"),
            ip: t("network.ip_addresses"),
            role: t("network.roles"),
            peer: t("connections.peer"),
            remove: t("device_sections.remove"),
            physical: t("device_sections.physical"),
            logical: t("device_sections.logical"),
            add: t("device_sections.add"),
            unused: t("device_sections.unused"),
            no_serial: t("device_sections.no_serial"),
            empty: t("device_sections.ports_empty"),
            count: t("device_sections.ports_count"),
            hint_ports: t("device_sections.hint_ports"),
            hint_remove: t("device_sections.hint_remove"),
            hint_stack: t("device_sections.hint_stack"),
            hint_vlan_ip: t("device_sections.hint_vlan_ip"),
        },
    })
}

/// 以前の画面の場所。機器の詳細の節へ送る。
pub async fn interfaces(Path((project_id, device_id)): Path<(i32, i32)>) -> Response {
    戻り先(project_id, device_id, PORTS)
}

/// フォームの1行。
struct 入力行 {
    n: usize,
    id: Option<i32>,
    port: Option<(i32, i32)>,
    name: String,
    kind: String,
    stack: String,
    vlan: String,
    ip: String,
    role: String,
    remove: bool,
}

/// 保存後のインターフェース。既存のまま残すか、新しく開く。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum 節点 {
    既存(i32),
    新規(usize),
}

/// 検証を済ませた1行。
struct 計画行 {
    節点: 節点,
    /// 物理ポートの行か（束ねの持ち主を決める）。
    ポート行: bool,
    name: String,
    interface_type: String,
    aggregation_mode: Option<String>,
    port: Option<(i32, i32)>,
    stack: Vec<String>,
    vlans: Vec<(i32, &'static str)>,
    ips: Vec<(String, i32, Option<i32>)>,
    roles: Vec<String>,
}

fn 区切る(value: &str) -> Vec<String> {
    value
        .split([',', '、', '，'])
        .map(正規化)
        .filter(|v| !v.is_empty())
        .collect()
}

fn 二つ組(value: &str) -> Option<(i32, i32)> {
    let (a, b) = value.trim().split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `100` / `200T` / `200T:名前`。同じタグのVLANが複数あるときは名前で選ぶ
/// （VLANタグはL2ドメインごとに独立しており、一意ではない。8.3）。
fn vlanを読む(token: &str, 候補: &[vlan::Model]) -> Result<(i32, &'static str), &'static str> {
    let (頭, 名前) = match token.split_once(':') {
        Some((a, b)) => (a.trim(), Some(b.trim())),
        None => (token.trim(), None),
    };
    let (数字, mode) = match 頭.strip_suffix(['T', 't']) {
        Some(n) => (n, TAGGED),
        None => (頭, UNTAGGED),
    };
    let tag: i32 = 数字.parse().map_err(|_| "device_sections.error_vlan")?;
    let 一致: Vec<&vlan::Model> = 候補
        .iter()
        .filter(|v| v.vlan_tag == tag)
        .filter(|v| 名前.is_none_or(|n| v.name == n))
        .collect();
    match 一致.as_slice() {
        [v] => Ok((v.id, mode)),
        [] => Err("device_sections.error_vlan_unknown"),
        _ => Err("device_sections.error_vlan_ambiguous"),
    }
}

/// `192.0.2.10/24`。**書き間違いを弾くだけ**で、アドレスとしての正しさは見ない
/// （IPv6や特殊な表記で正当な入力を拒否しないため）。
fn ipを読む(token: &str) -> Result<(String, i32), &'static str> {
    let (addr, prefix) = token
        .split_once('/')
        .ok_or("device_sections.error_ip_form")?;
    let addr = addr.trim();
    if addr.is_empty()
        || !addr
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':')
    {
        return Err("network.error_ip");
    }
    match prefix.trim().parse::<i32>() {
        Ok(n) if (0..=128).contains(&n) => Ok((addr.to_owned(), n)),
        _ => Err("network.error_prefix"),
    }
}

/// アドレスを含むサブネット。**最も狭いものを選び、同じ広さなら自分のプロジェクトの
/// ものを優先する。**読めないアドレスやCIDRは、どこにも属さないものとして扱う。
fn サブネットを決める(
    addr: &str, 候補: &[subnet::Model], project_id: i32
) -> Option<i32> {
    let ip: IpAddr = addr.parse().ok()?;
    候補
        .iter()
        .filter_map(|s| {
            let (net, prefix) = s.cidr.split_once('/')?;
            let net: IpAddr = net.trim().parse().ok()?;
            let prefix: u32 = prefix.trim().parse().ok()?;
            含む(net, prefix, ip).then_some((prefix, s.project_id == Some(project_id), s.id))
        })
        .max_by_key(|(prefix, 自分, _)| (*prefix, *自分))
        .map(|(_, _, id)| id)
}

fn 含む(net: IpAddr, prefix: u32, ip: IpAddr) -> bool {
    fn 上位ビット(value: u128, 幅: u32, prefix: u32) -> Option<u128> {
        if prefix > 幅 {
            return None;
        }
        Some(if prefix == 0 {
            0
        } else {
            value >> (幅 - prefix)
        })
    }
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(a)) => 上位ビット(u128::from(u32::from(n)), 32, prefix)
            .zip(上位ビット(u128::from(u32::from(a)), 32, prefix))
            .is_some_and(|(x, y)| x == y),
        (IpAddr::V6(n), IpAddr::V6(a)) => 上位ビット(u128::from(n), 128, prefix)
            .zip(上位ビット(u128::from(a), 128, prefix))
            .is_some_and(|(x, y)| x == y),
        _ => false,
    }
}

fn 入力行を読む(入力: &HashMap<String, String>) -> Vec<入力行> {
    let mut 番号: Vec<usize> = 入力
        .keys()
        .filter_map(|k| k.strip_prefix("name_")?.parse().ok())
        .collect();
    番号.sort_unstable();
    let 値 = |key: String| 入力.get(&key).cloned().unwrap_or_default();
    番号
        .into_iter()
        .map(|n| 入力行 {
            n,
            id: 値(format!("id_{n}")).trim().parse().ok(),
            port: 二つ組(&値(format!("port_{n}"))),
            name: 正規化(&値(format!("name_{n}"))),
            kind: 値(format!("kind_{n}")).trim().to_owned(),
            stack: 値(format!("stack_{n}")),
            vlan: 値(format!("vlan_{n}")),
            ip: 値(format!("ip_{n}")),
            role: 値(format!("role_{n}")),
            remove: 入力.contains_key(&format!("remove_{n}")),
        })
        .collect()
}

/// ポートとIPの節を保存する。
///
/// 送られてきた行を「あるべき状態」として、現在との差だけを閉じて開く。
/// **行は消さない**（不変条件1）。
pub async fn save_ports(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let d = super::device::編集する機器(&state, &current, project_id, device_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let db = &state.db;
    let 入力 = 対へ(pairs);

    macro_rules! 戻る {
        ($key:expr) => {
            return super::device::詳細を描く(
                &state,
                &current,
                project_id,
                device_id,
                Some(描き直し {
                    節: PORTS,
                    error: rust_i18n::t!($key, locale = l).to_string(),
                    入力,
                }),
            )
            .await
        };
    }

    // **読み取りはトランザクションを開く前に済ませる**（SQLiteで自分の書き込みロックを待つ）
    let 今 = 現状を引く(db, d.id).await?;
    let ポート = 物理ポートを並べる(db, d.id).await?;
    let 候補のvlan = vlan::Entity::find()
        .filter(vlan::Column::RetiredAt.is_null())
        .all(db)
        .await
        .map_err(内部)?;
    let 候補のサブネット = subnet::Entity::find()
        .filter(
            Condition::any()
                .add(subnet::Column::ProjectId.eq(project_id))
                .add(subnet::Column::ProjectId.is_null()),
        )
        .all(db)
        .await
        .map_err(内部)?;

    // ---- 1行ずつ読む ------------------------------------------------------
    let mut 閉じる: Vec<i32> = Vec::new();
    let mut 計画: Vec<計画行> = Vec::new();
    let mut 触れた: HashSet<i32> = HashSet::new();
    for 行 in 入力行を読む(&入力) {
        let 既存 = match 行.id {
            // **この機器の現行のインターフェースであることを確かめる。**IDだけを
            // 信じると、別の機器のインターフェースを操作できてしまう
            Some(id) => match 今.interfaces.iter().find(|i| i.id == id) {
                Some(i) => Some(i),
                None => return Err(AppError::NotFound),
            },
            None => None,
        };
        if let Some(i) = 既存 {
            if !触れた.insert(i.id) {
                return Err(AppError::NotFound);
            }
            if 行.remove {
                閉じる.push(i.id);
                continue;
            }
        }
        let 中身がある = [&行.stack, &行.vlan, &行.ip, &行.role]
            .iter()
            .any(|v| !v.trim().is_empty());
        if 行.name.is_empty() {
            // 使っていないポートと、足さなかった空の行
            if 既存.is_none() && !中身がある {
                continue;
            }
            戻る!("network.error_name");
        }

        // 既存の行のポートは、送られてきた値ではなく今の値を使う
        let port = match 既存 {
            Some(i) => i.part_instance_id.zip(i.port_slot_id),
            None => 行.port,
        };
        if 既存.is_none() {
            if let Some((part, slot)) = port {
                if !ポート
                    .iter()
                    .any(|p| p.part_id == part && p.slot_id == slot)
                {
                    戻る!("network.error_port");
                }
            }
        }

        // **物理ポートを持てるのは Physical だけ、束ね方を持てるのは Bond だけ**（8.5）
        let (interface_type, aggregation_mode) = if port.is_some() {
            (PHYSICAL.to_owned(), None)
        } else {
            match 行.kind.split_once(':') {
                Some((BOND, m)) if AGGREGATION_MODES.contains(&m) => {
                    (BOND.to_owned(), Some(m.to_owned()))
                }
                Some(_) => 戻る!("network.error_aggregation"),
                // **閉じた語彙は既定へ寄せず拒否する**（8.6）
                None if INTERFACE_TYPES.contains(&行.kind.as_str()) => (行.kind.clone(), None),
                None => 戻る!("network.error_type"),
            }
        };

        let mut vlans = Vec::new();
        for token in 区切る(&行.vlan) {
            match vlanを読む(&token, &候補のvlan) {
                Ok(v) if vlans.iter().any(|(id, _)| *id == v.0) => {
                    let _ = v;
                    戻る!("network.error_vlan_duplicate")
                }
                Ok(v) => vlans.push(v),
                Err(key) => 戻る!(key),
            }
        }
        let mut ips = Vec::new();
        for token in 区切る(&行.ip) {
            match ipを読む(&token) {
                Ok((addr, prefix)) => {
                    if !ips.iter().any(|(a, p, _)| *a == addr && *p == prefix) {
                        let s = サブネットを決める(&addr, &候補のサブネット, project_id);
                        ips.push((addr, prefix, s));
                    }
                }
                Err(key) => 戻る!(key),
            }
        }
        let mut roles = Vec::new();
        for r in 区切る(&行.role) {
            if !roles.contains(&r) {
                roles.push(r);
            }
        }

        // **属性を書き換える経路は持たない。**名前・種別・束ね方が変わったら、
        // 閉じて開き直す（23.5）
        let 節点 = match 既存 {
            Some(i)
                if i.os_interface_name == 行.name
                    && i.interface_type == interface_type
                    && i.aggregation_mode == aggregation_mode =>
            {
                節点::既存(i.id)
            }
            Some(i) => {
                閉じる.push(i.id);
                節点::新規(行.n)
            }
            None => 節点::新規(行.n),
        };
        計画.push(計画行 {
            節点,
            ポート行: port.is_some(),
            name: 行.name,
            interface_type,
            aggregation_mode,
            port,
            stack: 区切る(&行.stack),
            vlans,
            ips,
            roles,
        });
    }

    // ---- 保存後の名前と、ポートの重複 -------------------------------------
    // フォームに無い行はそのまま残る
    let mut 名前: HashMap<String, (節点, bool)> = HashMap::new();
    for i in &今.interfaces {
        if !触れた.contains(&i.id) {
            名前.insert(
                i.os_interface_name.clone(),
                (節点::既存(i.id), ポートを持つ(i)),
            );
        }
    }
    for 行 in &計画 {
        if 名前
            .insert(行.name.clone(), (行.節点, 行.ポート行))
            .is_some()
        {
            戻る!("network.error_interface_duplicate");
        }
    }
    let mut 使うポート: HashSet<(i32, i32)> = 今
        .interfaces
        .iter()
        .filter(|i| !閉じる.contains(&i.id))
        .filter_map(|i| i.part_instance_id.zip(i.port_slot_id))
        .collect();
    for 行 in 計画.iter().filter(|r| matches!(r.節点, 節点::新規(_))) {
        if let Some(p) = 行.port {
            if !使うポート.insert(p) {
                戻る!("device_sections.error_port_in_use");
            }
        }
    }

    // ---- 束ね ------------------------------------------------------------
    // 束ねの持ち主は、下位が物理ポートの行ならその行、そうでなければ上位の行。
    // 持ち主がフォームにある束ねだけを合わせる
    let mut 望む束ね: HashSet<(節点, 節点)> = HashSet::new();
    for 行 in &計画 {
        for 相手 in &行.stack {
            let Some((先, 先はポート行)) = 名前.get(相手).copied() else {
                戻る!("device_sections.error_stack_unknown");
            };
            if 先 == 行.節点 {
                戻る!("network.error_member_self");
            }
            if 行.ポート行 {
                望む束ね.insert((先, 行.節点));
            } else if 先はポート行 {
                戻る!("device_sections.error_stack_port");
            } else {
                望む束ね.insert((行.節点, 先));
            }
        }
    }
    let 残る節点: HashMap<i32, &計画行> = 計画
        .iter()
        .filter_map(|r| match r.節点 {
            節点::既存(id) => Some((id, r)),
            節点::新規(_) => None,
        })
        .collect();
    let mut 閉じる束ね: Vec<&interface_stack::Model> = Vec::new();
    let mut 結果の束ね: HashSet<(節点, 節点)> = 望む束ね.clone();
    for s in &今.stacks {
        let 組 = (
            節点::既存(s.upper_interface_id),
            節点::既存(s.lower_interface_id),
        );
        let 端が閉じる =
            閉じる.contains(&s.upper_interface_id) || 閉じる.contains(&s.lower_interface_id);
        let 持ち主 = if 今.ポート行か(s.lower_interface_id) {
            s.lower_interface_id
        } else {
            s.upper_interface_id
        };
        if 端が閉じる || (残る節点.contains_key(&持ち主) && !望む束ね.contains(&組))
        {
            閉じる束ね.push(s);
        } else {
            結果の束ね.insert(組);
        }
    }
    // **循環を作らせない。**上下を辿る表示が終わらなくなる（8.5）
    if 循環する(&結果の束ね) {
        戻る!("network.error_member_cycle");
    }
    let 既にある束ね: HashSet<(節点, 節点)> = 今
        .stacks
        .iter()
        .filter(|s| !閉じる束ね.iter().any(|c| c.id == s.id))
        .map(|s| {
            (
                節点::既存(s.upper_interface_id),
                節点::既存(s.lower_interface_id),
            )
        })
        .collect();
    let mut 開く束ね: Vec<(節点, 節点)> = 望む束ね.difference(&既にある束ね).copied().collect();
    開く束ね.sort_by_key(|組| format!("{組:?}"));

    // ---- 子の差分 --------------------------------------------------------
    let mut 閉じるvlan: Vec<&interface_vlan::Model> = Vec::new();
    let mut 閉じる役割: Vec<&interface_role::Model> = Vec::new();
    let mut 閉じるip: Vec<&ip_address::Model> = Vec::new();
    for id in &閉じる {
        閉じるvlan.extend(今.vlans.get(id).into_iter().flatten().map(|(_, link)| link));
        閉じる役割.extend(今.roles.get(id).into_iter().flatten());
        閉じるip.extend(今.ips.get(id).into_iter().flatten());
    }
    let mut 開くvlan: Vec<(節点, i32, &'static str)> = Vec::new();
    let mut 開く役割: Vec<(節点, String)> = Vec::new();
    let mut 開くip: Vec<(節点, String, i32, Option<i32>)> = Vec::new();
    for 行 in &計画 {
        let id = match 行.節点 {
            節点::既存(id) => Some(id),
            節点::新規(_) => None,
        };
        let 今のvlan = id
            .and_then(|id| 今.vlans.get(&id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let 今の役割 = id
            .and_then(|id| 今.roles.get(&id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let 今のip = id
            .and_then(|id| 今.ips.get(&id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        for (_, link) in 今のvlan {
            if !行
                .vlans
                .iter()
                .any(|(v, m)| *v == link.vlan_id && *m == link.tagging_mode)
            {
                閉じるvlan.push(link);
            }
        }
        for (v, m) in &行.vlans {
            if !今のvlan
                .iter()
                .any(|(_, link)| link.vlan_id == *v && link.tagging_mode == *m)
            {
                開くvlan.push((行.節点, *v, m));
            }
        }
        for r in 今の役割 {
            if !行.roles.contains(&r.role) {
                閉じる役割.push(r);
            }
        }
        for r in &行.roles {
            if !今の役割.iter().any(|c| c.role == *r) {
                開く役割.push((行.節点, r.clone()));
            }
        }
        for ip in 今のip {
            if !行
                .ips
                .iter()
                .any(|(a, p, _)| *a == ip.ip_address && *p == ip.prefix_length)
            {
                閉じるip.push(ip);
            }
        }
        for (a, p, s) in &行.ips {
            if !今のip
                .iter()
                .any(|c| c.ip_address == *a && c.prefix_length == *p)
            {
                開くip.push((行.節点, a.clone(), *p, *s));
            }
        }
    }

    // **同じサブネットの中で同じIPを2つ持たない**（14.2）。DB制約もあるが、
    // 制約違反の500ではなく理由を返す。この保存で閉じる行は数えない
    let mut 開く組 = HashSet::new();
    for (_, addr, _, s) in &開くip {
        let Some(sid) = s else { continue };
        if !開く組.insert((*sid, addr.clone())) {
            戻る!("network.error_ip_duplicate");
        }
        let 既に = ip_address::Entity::find()
            .filter(ip_address::Column::SubnetId.eq(*sid))
            .filter(ip_address::Column::IpAddress.eq(addr))
            .filter(ip_address::Column::ToDate.is_null())
            .all(db)
            .await
            .map_err(内部)?;
        if 既に.iter().any(|x| !閉じるip.iter().any(|c| c.id == x.id)) {
            戻る!("network.error_ip_duplicate");
        }
    }

    let 開く行: Vec<&計画行> = 計画
        .iter()
        .filter(|r| matches!(r.節点, 節点::新規(_)))
        .collect();
    if 閉じる.is_empty()
        && 開く行.is_empty()
        && 閉じる束ね.is_empty()
        && 開く束ね.is_empty()
        && 閉じるvlan.is_empty()
        && 開くvlan.is_empty()
        && 閉じる役割.is_empty()
        && 開く役割.is_empty()
        && 閉じるip.is_empty()
        && 開くip.is_empty()
    {
        return Ok(戻り先(project_id, device_id, PORTS));
    }

    // ---- 書く ------------------------------------------------------------
    let now = Utc::now();
    let tx = AuditedTx::begin(db, Actor::User(current.user.id))
        .await
        .map_err(内部)?;

    // 先に閉じる。同じサブネットの同じIPを付け替えるとき、開くより先に閉じて
    // いないと部分インデックスに当たる
    for row in 閉じるvlan {
        tx.update(
            row,
            interface_vlan::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }
    for row in 閉じる役割 {
        tx.update(
            row,
            interface_role::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }
    for row in 閉じるip {
        tx.update(
            row,
            ip_address::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }
    for row in 閉じる束ね {
        tx.update(
            row,
            interface_stack::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }
    for id in &閉じる {
        let Some(row) = 今.interfaces.iter().find(|i| i.id == *id) else {
            continue;
        };
        tx.update(
            row,
            os_interface::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(now)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }

    let mut 新しいid: BTreeMap<usize, i32> = BTreeMap::new();
    for 行 in 開く行 {
        let 節点::新規(n) = 行.節点 else {
            continue;
        };
        let created = tx
            .insert(os_interface::ActiveModel {
                device_id: Set(d.id),
                interface_type: Set(行.interface_type.clone()),
                part_instance_id: Set(行.port.map(|p| p.0)),
                port_slot_id: Set(行.port.map(|p| p.1)),
                os_interface_name: Set(行.name.clone()),
                aggregation_mode: Set(行.aggregation_mode.clone()),
                work_order_id: Set(None),
                from_date: Set(now),
                to_date: Set(None),
                ..Default::default()
            })
            .await
            .map_err(内部)?;
        新しいid.insert(n, created.id);
    }
    let 実id = |節: 節点| -> Option<i32> {
        match 節 {
            節点::既存(id) => Some(id),
            節点::新規(n) => 新しいid.get(&n).copied(),
        }
    };

    for (upper, lower) in 開く束ね {
        let (Some(upper), Some(lower)) = (実id(upper), 実id(lower)) else {
            continue;
        };
        tx.insert(interface_stack::ActiveModel {
            upper_interface_id: Set(upper),
            lower_interface_id: Set(lower),
            from_date: Set(now),
            to_date: Set(None),
            ..Default::default()
        })
        .await
        .map_err(内部)?;
    }
    for (節, vlan_id, mode) in 開くvlan {
        let Some(id) = 実id(節) else { continue };
        tx.insert(interface_vlan::ActiveModel {
            os_interface_id: Set(id),
            vlan_id: Set(vlan_id),
            tagging_mode: Set(mode.to_owned()),
            work_order_id: Set(None),
            from_date: Set(now),
            to_date: Set(None),
            ..Default::default()
        })
        .await
        .map_err(内部)?;
    }
    for (節, role) in 開く役割 {
        let Some(id) = 実id(節) else { continue };
        tx.insert(interface_role::ActiveModel {
            os_interface_id: Set(id),
            role: Set(role),
            from_date: Set(now),
            to_date: Set(None),
            ..Default::default()
        })
        .await
        .map_err(内部)?;
    }
    for (節, addr, prefix, subnet_id) in 開くip {
        let Some(id) = 実id(節) else { continue };
        tx.insert(ip_address::ActiveModel {
            os_interface_id: Set(id),
            ip_address: Set(addr),
            prefix_length: Set(prefix),
            subnet_id: Set(subnet_id),
            work_order_id: Set(None),
            from_date: Set(now),
            to_date: Set(None),
            ..Default::default()
        })
        .await
        .map_err(内部)?;
    }
    tx.commit().await.map_err(内部)?;

    Ok(戻り先(project_id, device_id, PORTS))
}

/// `(上位, 下位)` の束ねが循環するか。
fn 循環する(束ね: &HashSet<(節点, 節点)>) -> bool {
    let mut 下位: HashMap<節点, Vec<節点>> = HashMap::new();
    for (u, l) in 束ね {
        下位.entry(*u).or_default().push(*l);
    }
    for 起点 in 下位.keys() {
        let mut 見る: Vec<節点> = 下位[起点].clone();
        let mut 見た = HashSet::new();
        while let Some(n) = 見る.pop() {
            if n == *起点 {
                return true;
            }
            if 見た.insert(n) {
                見る.extend(下位.get(&n).into_iter().flatten().copied());
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vlan(id: i32, tag: i32, name: &str) -> vlan::Model {
        vlan::Model {
            id,
            vlan_tag: tag,
            name: name.to_owned(),
            zone: None,
            description: String::new(),
            retired_at: None,
            created_by: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn vlanは番号で受け末尾のtでタグ付きにする() {
        let 候補 = vec![vlan(1, 100, "svc"), vlan(2, 200, "stg")];
        assert_eq!(vlanを読む("100", &候補), Ok((1, UNTAGGED)));
        assert_eq!(vlanを読む("200T", &候補), Ok((2, TAGGED)));
        assert_eq!(vlanを読む("200t", &候補), Ok((2, TAGGED)));
        assert_eq!(
            vlanを読む("300", &候補),
            Err("device_sections.error_vlan_unknown")
        );
        assert_eq!(vlanを読む("abc", &候補), Err("device_sections.error_vlan"));
    }

    /// VLANタグは一意ではない（8.3）。黙ってどちらかを選ばない。
    #[test]
    fn 同じタグのvlanが複数あれば名前で選ぶ() {
        let 候補 = vec![vlan(1, 100, "tokyo"), vlan(2, 100, "osaka")];
        assert_eq!(
            vlanを読む("100", &候補),
            Err("device_sections.error_vlan_ambiguous")
        );
        assert_eq!(vlanを読む("100:osaka", &候補), Ok((2, UNTAGGED)));
        assert_eq!(vlanを読む("100T:tokyo", &候補), Ok((1, TAGGED)));
    }

    #[test]
    fn ipはプレフィックスまで1つの欄で受ける() {
        assert_eq!(ipを読む("192.0.2.10/24"), Ok(("192.0.2.10".to_owned(), 24)));
        assert_eq!(
            ipを読む("2001:db8::1/64"),
            Ok(("2001:db8::1".to_owned(), 64))
        );
        assert_eq!(ipを読む("192.0.2.10"), Err("device_sections.error_ip_form"));
        assert_eq!(ipを読む("192.0.2.10/200"), Err("network.error_prefix"));
        assert_eq!(ipを読む("ホスト/24"), Err("network.error_ip"));
    }

    fn subnet(id: i32, project_id: Option<i32>, cidr: &str) -> subnet::Model {
        subnet::Model {
            id,
            project_id,
            vlan_id: None,
            cidr: cidr.to_owned(),
            zone: None,
            description: String::new(),
            created_by: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn サブネットは最も狭いものを選び同じ広さなら自分のものを選ぶ() {
        let 候補 = vec![
            subnet(1, None, "10.0.0.0/8"),
            subnet(2, Some(7), "10.20.100.0/24"),
            subnet(3, None, "10.20.100.0/24"),
            subnet(4, Some(7), "2001:db8::/32"),
        ];
        assert_eq!(サブネットを決める("10.20.100.17", &候補, 7), Some(2));
        assert_eq!(サブネットを決める("10.9.0.1", &候補, 7), Some(1));
        assert_eq!(サブネットを決める("192.0.2.1", &候補, 7), None);
        assert_eq!(サブネットを決める("2001:db8::1", &候補, 7), Some(4));
        // 読めないアドレスは、どこにも属さない
        assert_eq!(サブネットを決める("10.20.100", &候補, 7), None);
    }

    #[test]
    fn 束ねの循環を見つける() {
        let a = 節点::既存(1);
        let b = 節点::既存(2);
        let c = 節点::新規(0);
        assert!(!循環する(&HashSet::from([(a, b), (b, c)])));
        assert!(循環する(&HashSet::from([(a, b), (b, c), (c, a)])));
    }
}
