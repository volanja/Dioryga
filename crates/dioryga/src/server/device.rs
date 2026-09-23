//! プロジェクト領域の機器一覧・詳細（設計書16.1のB領域、6章、23章）。
//!
//! # クロスプロジェクト可視性（A-6、設計書3章）
//!
//! **「現在このプロジェクトにある機器」だけを出すのでは足りない。**プロジェクトPの
//! メンバーは「現在Pに属する、**または過去にPに属したことがある**」機器の全履歴を
//! 閲覧できる。現在の割当だけで絞ると、移設された機器の履歴が誰からも見えなくなる。
//!
//! 判定は `DEVICE_ASSIGNMENT` に `(Project, P)` の行が**一度でも**現れるか
//! （`to_date` を見ない）で行う。
//!
//! # 現在の状態は計算で求める
//!
//! 所在・搭載位置・搭載部品・ファームウェアは、いずれも履歴テーブルの
//! `to_date IS NULL` から引く（不変条件2）。`DEVICE` 側に持たない。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::{NaiveDate, Utc};
use entity::{
    chassis_model, configuration, device, device_assignment, device_mount, device_stack,
    firmware_version, fixed_asset, maintenance_contract, maintenance_contract_item, part_catalog,
    part_instance, part_instance_location, project, purchase, vendor,
};
use sea_orm::sea_query::{Expr, Func, LikeExpr, Query as SeaQuery};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;

// 機器の種別は取込と同じ表を見る（8.6、#125）
use dioryga_catalog_format::DEVICE_CATEGORIES;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{
    render, 状態の表示, 状態の選択肢, 種別の表示, 種別の選択肢, Choice, Chrome, Locale,
};
use crate::server::AppState;

/// `DEVICE_ASSIGNMENT.location_type`。
const PROJECT: &str = "Project";

/// 予約中の機器（設計書11.6）。ラック図でも区別表示する。
/// 機器の状態（8.6）
const DEVICE_PLANNED: &str = "planned";

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct DeviceRow {
    id: i32,
    hostname: String,
    device_type: String,
    model: String,
    status: String,
    /// 日本語画面での表示名（#172）。保存する値は `status` のまま
    status_label: String,
    /// 予約中は区別して表示する（設計書11.6）
    planned: bool,
    location: String,
    /// このプロジェクトを離れた機器。A-6により履歴は見える
    departed: bool,
}

#[derive(askama::Template)]
#[template(path = "devices.html")]
struct DevicesPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_new: String,
    t_keyword: String,
    t_search: String,
    t_scope_current: String,
    t_scope_all: String,
    /// 並べ替えできる見出し（#171）
    h_hostname: SortHead,
    t_device_type: String,
    t_model: String,
    h_status: SortHead,
    t_location: String,
    t_empty: String,
    t_departed: String,
    t_planned: String,
    q: String,
    scope: String,
    rows: Vec<DeviceRow>,
    can_edit: bool,
}

struct Labeled {
    label: String,
    value: String,
}

#[derive(askama::Template)]
#[template(path = "device_detail.html")]
struct DeviceDetailPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    device_id: i32,
    t_back: String,
    t_edit: String,
    t_basic: String,
    t_location_history: String,
    t_mount: String,
    t_parts: String,
    t_firmware: String,
    t_stack: String,
    t_none: String,
    t_planned: String,
    t_not_implemented: String,
    t_sbom: String,
    t_interfaces: String,
    hostname: String,
    planned: bool,
    /// 統合先。**統合された機器は削除されず、ここへ誘導する**（設計書23.9）
    merged_into: Option<i32>,
    t_merged: String,
    basic: Vec<Labeled>,
    locations: Vec<Labeled>,
    mount: Vec<Labeled>,
    parts: Vec<Labeled>,
    firmware: Vec<Labeled>,
    stack: Vec<Labeled>,
    /// 購入の記録（10.2）。**機器1台につき1行。専用の一覧画面を持たず、ここに出す。**
    purchase: Option<PurchaseView>,
    purchase_form: PurchaseForm,
    can_edit: bool,
}

/// 購入の記録（設計書10.2）。表示用。
struct PurchaseView {
    order_number: String,
    acquired_on: String,
    amount: String,
    supplier: String,
}

/// 購入の記録の入力欄。**1台につき1行なので、既存の値を入れて出し、上書きする。**
struct PurchaseForm {
    t_title: String,
    t_hint: String,
    t_order_number: String,
    t_order_number_hint: String,
    t_acquired_on: String,
    t_acquired_on_hint: String,
    t_amount: String,
    t_amount_hint: String,
    t_supplier: String,
    t_supplier_hint: String,
    t_save: String,
    t_empty: String,
    currency: String,
    order_number: String,
    acquired_on: String,
    amount: String,
    supplier: String,
}

/// 登録・編集の画面（設計書16.1「機器の登録は、手を動かす順に4段へ分ける」）。
///
/// **開閉と分岐はCSSで行う**（`:has(input:checked)`）。状態がフォームの値そのもの
/// なので、検証エラーで描き直したときに、開いていた欄は開いたまま戻る。
#[derive(askama::Template)]
#[template(path = "device_form.html")]
struct DeviceFormPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    /// 登録のときだけ、お金の記録と「このあとの手順」を出す
    is_new: bool,
    t: FormText,
    action: String,
    hostname: String,
    device_type: String,
    device_types: Vec<&'static str>,
    configuration_id: String,
    configurations: Vec<Labeled>,
    device_category: String,
    categories: Vec<Choice>,
    serial_number: String,
    asset_number: String,
    power_watt: String,
    status: String,
    statuses: Vec<Choice>,
    /// 「もう1台」で戻ったとき、直前に登録した機器のホスト名
    registered: Option<String>,
    // --- お金の記録（登録のときだけ） ---
    currency: String,
    order_number: String,
    supplier: String,
    acquisition_cost: String,
    acquisition_date: String,
    manage_as_fixed_asset: bool,
    useful_life_years: String,
    depreciation_method: String,
    methods: Vec<Choice>,
    register_maintenance: bool,
    maintenance_new: bool,
    maintenance_contract_id: String,
    contracts: Vec<Labeled>,
    contract_number: String,
    contract_vendor_id: String,
    vendors: Vec<Labeled>,
    contract_start: String,
    contract_end: String,
    contract_amount: String,
    quote_contact: String,
    failure_contact: String,
    error: Option<String>,
}

/// 登録・編集画面の文言。
struct FormText {
    title: String,
    lead: String,
    import_hint: String,
    back: String,
    submit: String,
    submit_again: String,
    again_hint: String,
    required: String,
    optional: String,
    registered: String,
    sec_kind: String,
    sec_kind_hint: String,
    sec_identity: String,
    sec_identity_hint: String,
    sec_state: String,
    sec_money: String,
    sec_money_hint: String,
    device_type: String,
    tip_physical: String,
    tip_virtual: String,
    tip_container: String,
    tip_logical: String,
    configuration: String,
    configuration_hint: String,
    device_category: String,
    device_category_hint: String,
    hostname: String,
    hostname_hint: String,
    serial_number: String,
    serial_hint: String,
    asset_number: String,
    asset_hint: String,
    status: String,
    status_hint: String,
    power_watt: String,
    power_hint: String,
    order_number: String,
    order_number_hint: String,
    supplier: String,
    supplier_hint: String,
    acquisition_cost: String,
    acquisition_cost_hint: String,
    acquisition_date: String,
    acquisition_date_hint: String,
    manage_as_fixed_asset: String,
    fixed_asset_hint: String,
    useful_life_years: String,
    depreciation_method: String,
    depreciation_hint: String,
    register_maintenance: String,
    maintenance_existing: String,
    maintenance_new: String,
    maintenance_contract: String,
    maintenance_contract_hint: String,
    contract_number: String,
    contract_vendor: String,
    contract_start: String,
    contract_end: String,
    contract_end_hint: String,
    contract_amount: String,
    quote_contact: String,
    failure_contact: String,
    failure_contact_hint: String,
    next_steps: String,
    next_steps_lead: String,
    step_register: String,
    step_register_where: String,
    step_parts: String,
    step_parts_where: String,
    step_interfaces: String,
    step_interfaces_where: String,
    step_sbom: String,
    step_sbom_where: String,
}

/// 語彙（`vocabularies.md`）。DB制約にはせず、画面はリストから選ばせる。
const DEVICE_TYPES: &[&str] = &["Physical", "Virtual", "Container", "Logical"];
const STATUSES: &[&str] = &["running", "failed", "repairing", "planned", "provisioning"];
/// 登録で選べる状態。**`planned` を含まない**——予約は増設の変更管理チケットが
/// 作る（設計書11.6）。この画面からも作れると、同じ状態を作る経路が2つになる。
const STATUSES_ON_CREATE: &[&str] = &["provisioning", "running", "repairing", "failed"];
/// 筐体のある1台。構成とシリアル番号を持つのはこれだけ（設計書6.2、13.2）。
const PHYSICAL: &str = "Physical";
/// `PURCHASE` / `FIXED_ASSET` / `MAINTENANCE_CONTRACT_ITEM` の `item_type`。
const DEVICE_ITEM: &str = "Device";

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// 並べ替える列（#171）。`hostname` / `status`。
    #[serde(default)]
    pub sort: Option<String>,
    /// `asc` / `desc`。
    #[serde(default)]
    pub dir: Option<String>,
}

/// 並べ替え（#171）。**URLに持つ**——再読み込みと共有で並びが残る。
///
/// **DBで並べる。**10,000台規模（22.2）で、取得後に並べ直すのは重い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Hostname,
    Status,
}

impl Sort {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("status") => Self::Status,
            // 既定はホスト名。**識別子で並ぶのが、台帳では最も探しやすい**
            _ => Self::Hostname,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hostname => "hostname",
            Self::Status => "status",
        }
    }

    fn column(self) -> device::Column {
        match self {
            Self::Hostname => device::Column::Hostname,
            Self::Status => device::Column::Status,
        }
    }
}

/// 並べ替えの向き。見出しを押すたびに入れ替わる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Asc,
    Desc,
}

impl Dir {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("desc") => Self::Desc,
            _ => Self::Asc,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    fn 逆(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

/// 見出しの1つ。押すと並べ替える（#171）。
struct SortHead {
    label: String,
    /// この列を押したときのリンク。向きは、いまその列で並んでいれば反転する。
    href: String,
    /// いまこの列で並んでいるか。矢印を出す
    current: bool,
    /// `▲`（昇順）/ `▼`（降順）。
    arrow: &'static str,
}

/// 表示範囲。既定は「現在このプロジェクトにあるもの」。
///
/// **A-6により、過去に所属した機器も閲覧できる。**ただし既定で混ぜると、
/// 移管済みの機器が現役と並んでしまう。16.1の「完了済みを隠せるフィルタは必須」
/// と同じ考え方で、既定を現在に絞り、切り替えで過去を出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Current,
    All,
}

impl Scope {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("all") => Self::All,
            _ => Self::Current,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::All => "all",
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let scope = Scope::parse(query.scope.as_deref());
    let keyword = query.q.clone().unwrap_or_default();
    let sort = Sort::parse(query.sort.as_deref());
    let dir = Dir::parse(query.dir.as_deref());

    let devices = 検索(&state.db, project_id, &keyword, sort, dir)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // 表示に要る情報をまとめて引く。行ごとに問い合わせるとN+1になる
    let 所在 = 現在の所在(&state.db, &devices).await?;
    let 型名 = 構成名(&state.db, &devices).await?;

    let mut rows = Vec::new();
    for d in devices {
        let 現在地 = 所在.iter().find(|(id, _)| *id == d.id).map(|(_, a)| a);
        let このプロジェクトにいる =
            現在地.is_some_and(|a| a.location_type == PROJECT && a.location_id == Some(project_id));

        if scope == Scope::Current && !このプロジェクトにいる {
            continue;
        }

        rows.push(DeviceRow {
            model: 型名
                .iter()
                .find(|(id, _)| *id == d.id)
                .map(|(_, n)| n.clone())
                .or_else(|| d.device_category.as_deref().map(|c| 種別の表示(c, l)))
                .unwrap_or_default(),
            location: match 現在地 {
                Some(a) => 所在の表示(&state, a, l).await?,
                None => rust_i18n::t!("devices.location_unknown", locale = l).to_string(),
            },
            departed: !このプロジェクトにいる,
            planned: d.status == DEVICE_PLANNED,
            // クラス名には生の値を、文字には表示名を当てる（#172）
            status_label: 状態の表示(&d.status, l),
            status: d.status,
            id: d.id,
            hostname: d.hostname,
            device_type: d.device_type,
        });
    }

    render(&DevicesPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "devices",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: rust_i18n::t!("devices.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("devices.lead", locale = l).to_string(),
        t_new: rust_i18n::t!("devices.new", locale = l).to_string(),
        t_keyword: rust_i18n::t!("devices.keyword", locale = l).to_string(),
        t_search: rust_i18n::t!("common.search", locale = l).to_string(),
        t_scope_current: rust_i18n::t!("devices.scope_current", locale = l).to_string(),
        t_scope_all: rust_i18n::t!("devices.scope_all", locale = l).to_string(),
        h_hostname: 並べ替えの見出し(
            project_id,
            &keyword,
            scope,
            sort,
            dir,
            Sort::Hostname,
            rust_i18n::t!("devices.hostname", locale = l).to_string(),
        ),
        t_device_type: rust_i18n::t!("devices.device_type", locale = l).to_string(),
        t_model: rust_i18n::t!("devices.model", locale = l).to_string(),
        h_status: 並べ替えの見出し(
            project_id,
            &keyword,
            scope,
            sort,
            dir,
            Sort::Status,
            rust_i18n::t!("devices.status", locale = l).to_string(),
        ),
        t_location: rust_i18n::t!("devices.location", locale = l).to_string(),
        t_empty: rust_i18n::t!("devices.empty", locale = l).to_string(),
        t_departed: rust_i18n::t!("devices.departed", locale = l).to_string(),
        t_planned: rust_i18n::t!("devices.planned", locale = l).to_string(),
        q: keyword,
        scope: scope.as_str().to_owned(),
        rows,
        can_edit,
    })
}

/// **A-6の判定を含む検索。**
///
/// `DEVICE_ASSIGNMENT` に `(Project, project_id)` の行が一度でも現れる機器を返す。
/// **`to_date` で絞らない**のが要点で、絞ると移設済みの機器の履歴が見えなくなる。
///
/// 統合で吸収された機器（`merged_into_device_id` あり）は除く。重複として
/// 畳まれた側であり、一覧に出すと同じ機器が2行に見える（設計書23.9）。
async fn 検索<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
    keyword: &str,
    sort: Sort,
    dir: Dir,
) -> Result<Vec<device::Model>, sea_orm::DbErr> {
    let 所属したことがある = SeaQuery::select()
        .column(device_assignment::Column::DeviceId)
        .from(device_assignment::Entity)
        .and_where(Expr::col(device_assignment::Column::LocationType).eq(PROJECT))
        .and_where(Expr::col(device_assignment::Column::LocationId).eq(project_id))
        .to_owned();

    let mut query = device::Entity::find()
        .filter(device::Column::Id.in_subquery(所属したことがある))
        .filter(device::Column::MergedIntoDeviceId.is_null());

    let keyword = keyword.trim();
    if !keyword.is_empty() {
        let pattern = format!("%{}%", escape_like(&keyword.to_lowercase()));
        let like = |column| {
            Expr::expr(Func::lower(Expr::col(column))).like(LikeExpr::new(&pattern).escape('\\'))
        };
        query = query.filter(
            like(device::Column::Hostname)
                .or(like(device::Column::SerialNumber))
                .or(like(device::Column::AssetNumber)),
        );
    }

    // **並べ替えはDBで行う**（#171）。同じ値のときの並びが揺れないよう、
    // 最後に id を添える
    let query = match dir {
        Dir::Asc => query.order_by_asc(sort.column()),
        Dir::Desc => query.order_by_desc(sort.column()),
    };
    query.order_by_asc(device::Column::Id).all(db).await
}

/// 見出しを組む（#171）。**いまの検索・表示範囲を保ったままリンクにする。**
fn 並べ替えの見出し(
    project_id: i32,
    keyword: &str,
    scope: Scope,
    sort: Sort,
    dir: Dir,
    列: Sort,
    label: String,
) -> SortHead {
    let current = sort == 列;
    // 同じ列を押したら向きを反転、別の列なら昇順から
    let 次 = if current { dir.逆() } else { Dir::Asc };
    let href = format!(
        "/projects/{project_id}/devices?q={}&scope={}&sort={}&dir={}",
        百分率符号化(keyword),
        scope.as_str(),
        列.as_str(),
        次.as_str()
    );
    SortHead {
        label,
        href,
        current,
        arrow: if dir == Dir::Asc { "▲" } else { "▼" },
    }
}

/// クエリ文字列に載せるための符号化。
///
/// **依存を増やさないために自前で持つ。**英数字と `-._~` 以外を `%XX` にする
/// （RFC 3986 の unreserved）。空白を `+` にはしない——`%20` でどちらの解釈でも通る。
fn 百分率符号化(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 各機器の現在の所在（`to_date IS NULL` の1行）。
async fn 現在の所在(
    db: &sea_orm::DatabaseConnection,
    devices: &[device::Model],
) -> AppResult<Vec<(i32, device_assignment::Model)>> {
    if devices.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i32> = devices.iter().map(|d| d.id).collect();

    let rows = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.is_in(ids))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(rows.into_iter().map(|a| (a.device_id, a)).collect())
}

/// 所在の表示名。プロジェクト名・倉庫名まで解決する。
async fn 所在の表示(
    state: &AppState,
    assignment: &device_assignment::Model,
    locale: &str,
) -> AppResult<String> {
    let 名前 = match (assignment.location_type.as_str(), assignment.location_id) {
        (PROJECT, Some(id)) => project::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|p| p.name),
        ("Warehouse", Some(id)) => entity::warehouse::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|w| w.name),
        _ => None,
    };

    if let Some(name) = 名前 {
        return Ok(format!("{}：{name}", assignment.location_type));
    }

    // **キーを実行時に組み立てない。**組み立てると、語彙が増えたときに
    // 翻訳の欠落がコンパイルでも起動時でも表面化しない
    Ok(match assignment.location_type.as_str() {
        // Disposed は location_id を持たない（設計書6.2）
        "Disposed" => rust_i18n::t!("devices.location_disposed", locale = locale).to_string(),
        other => other.to_owned(),
    })
}

/// 各機器の型名（`CONFIGURATION` → `CHASSIS_MODEL` → `VENDOR`）。
async fn 構成名(
    db: &sea_orm::DatabaseConnection,
    devices: &[device::Model],
) -> AppResult<Vec<(i32, String)>> {
    let config_ids: Vec<i32> = devices.iter().filter_map(|d| d.configuration_id).collect();
    if config_ids.is_empty() {
        return Ok(Vec::new());
    }

    let configs = configuration::Entity::find()
        .filter(configuration::Column::Id.is_in(config_ids))
        .find_also_related(chassis_model::Entity)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let vendor_ids: Vec<i32> = configs
        .iter()
        .filter_map(|(_, m)| m.as_ref().map(|m| m.vendor_id))
        .collect();
    let vendors = if vendor_ids.is_empty() {
        Vec::new()
    } else {
        vendor::Entity::find()
            .filter(vendor::Column::Id.is_in(vendor_ids))
            .all(db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    };

    Ok(devices
        .iter()
        .filter_map(|d| {
            let cid = d.configuration_id?;
            let (config, model) = configs.iter().find(|(c, _)| c.id == cid)?;
            let model = model.as_ref()?;
            let vendor = vendors.iter().find(|v| v.id == model.vendor_id);
            Some((
                d.id,
                match vendor {
                    Some(v) => format!("{} {} / {}", v.name, model.model_name, config.name),
                    None => format!("{} / {}", model.model_name, config.name),
                },
            ))
        })
        .collect())
}

// ---------------------------------------------------------------------------
// 詳細
// ---------------------------------------------------------------------------

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let d = 対象(&state, project_id, device_id).await?;

    let 未設定 = rust_i18n::t!("devices.none", locale = l).to_string();
    let 空欄 = |value: Option<String>| value.unwrap_or_else(|| 未設定.clone());

    let basic = vec![
        Labeled {
            label: rust_i18n::t!("devices.device_type", locale = l).to_string(),
            value: d.device_type.clone(),
        },
        Labeled {
            label: rust_i18n::t!("devices.model", locale = l).to_string(),
            value: 構成名(&state.db, std::slice::from_ref(&d))
                .await?
                .first()
                .map(|(_, n)| n.clone())
                .or_else(|| d.device_category.as_deref().map(|c| 種別の表示(c, l)))
                .unwrap_or_else(|| 未設定.clone()),
        },
        Labeled {
            label: rust_i18n::t!("devices.serial_number", locale = l).to_string(),
            value: 空欄(d.serial_number.clone()),
        },
        Labeled {
            label: rust_i18n::t!("devices.asset_number", locale = l).to_string(),
            value: 空欄(d.asset_number.clone()),
        },
        Labeled {
            label: rust_i18n::t!("devices.power_watt", locale = l).to_string(),
            value: format!("{} W", d.power_watt),
        },
        Labeled {
            label: rust_i18n::t!("devices.status", locale = l).to_string(),
            value: 状態の表示(&d.status, l),
        },
        Labeled {
            label: "UID".to_owned(),
            value: d.uid.clone(),
        },
    ];

    // 所在は履歴をすべて出す。**A-6が保証するのはこの履歴の閲覧**であり、
    // 現在地だけを見せるのでは移設の経緯が追えない
    let mut locations = Vec::new();
    for a in device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(d.id))
        .order_by_desc(device_assignment::Column::FromDate)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        let 期間 = match a.to_date {
            Some(to) => format!(
                "{} 〜 {}",
                a.from_date.format("%Y-%m-%d"),
                to.format("%Y-%m-%d")
            ),
            None => format!("{} 〜", a.from_date.format("%Y-%m-%d")),
        };
        locations.push(Labeled {
            label: 期間,
            value: 所在の表示(&state, &a, l).await?,
        });
    }

    let 購入 = 購入の記録(&state.db, d.id).await?;

    render(&DeviceDetailPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "devices",
        )
        .await,
        project_id,
        project_name: project.name.clone(),
        device_id: d.id,
        t_back: rust_i18n::t!("devices.back", locale = l).to_string(),
        t_edit: rust_i18n::t!("projects.edit", locale = l).to_string(),
        t_basic: rust_i18n::t!("devices.basic", locale = l).to_string(),
        t_location_history: rust_i18n::t!("devices.location_history", locale = l).to_string(),
        t_mount: rust_i18n::t!("devices.mount", locale = l).to_string(),
        t_parts: rust_i18n::t!("devices.parts", locale = l).to_string(),
        t_firmware: rust_i18n::t!("devices.firmware", locale = l).to_string(),
        t_stack: rust_i18n::t!("devices.stack", locale = l).to_string(),
        t_none: 未設定,
        t_planned: rust_i18n::t!("devices.planned", locale = l).to_string(),
        t_not_implemented: rust_i18n::t!("devices.section_pending", locale = l).to_string(),
        t_sbom: rust_i18n::t!("sbom.title", locale = l).to_string(),
        t_interfaces: rust_i18n::t!("network.interfaces", locale = l).to_string(),
        t_merged: rust_i18n::t!("devices.merged", locale = l).to_string(),
        hostname: d.hostname.clone(),
        planned: d.status == DEVICE_PLANNED,
        merged_into: d.merged_into_device_id,
        basic,
        locations,
        mount: 搭載位置(&state, &d, l).await?,
        parts: 搭載部品(&state, &d).await?,
        firmware: ファームウェア(&state, &d).await?,
        stack: スタック構成(&state, &d).await?,
        purchase: 購入の表示(&購入, &project.currency),
        purchase_form: 購入の入力欄(&購入, &project.currency, l),
        can_edit,
    })
}

/// この機器の購入の記録（設計書10.2）。**機器1台につき1行。**
async fn 購入の記録<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Option<purchase::Model>> {
    purchase::Entity::find()
        .filter(purchase::Column::ItemType.eq(DEVICE_ITEM))
        .filter(purchase::Column::ItemId.eq(device_id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

fn 購入の表示(p: &Option<purchase::Model>, 通貨: &str) -> Option<PurchaseView> {
    p.as_ref().map(|p| PurchaseView {
        order_number: p.order_number.clone().unwrap_or_default(),
        acquired_on: p.acquired_on.map(|d| d.to_string()).unwrap_or_default(),
        amount: crate::currency::表示(p.amount, 通貨),
        supplier: p.supplier.clone().unwrap_or_default(),
    })
}

fn 購入の入力欄(p: &Option<purchase::Model>, 通貨: &str, l: &'static str) -> PurchaseForm {
    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    PurchaseForm {
        t_title: t("devices.purchase"),
        t_hint: t("devices.purchase_hint"),
        t_order_number: t("devices.order_number"),
        t_order_number_hint: t("devices.order_number_hint"),
        t_acquired_on: t("devices.acquisition_date"),
        t_acquired_on_hint: t("devices.acquisition_date_hint"),
        t_amount: t("devices.acquisition_cost"),
        t_amount_hint: t("costs.amount_hint"),
        t_supplier: t("devices.supplier"),
        t_supplier_hint: t("devices.supplier_hint"),
        t_save: t("common.save"),
        t_empty: t("devices.no_purchase"),
        currency: 通貨.to_owned(),
        order_number: p
            .as_ref()
            .and_then(|p| p.order_number.clone())
            .unwrap_or_default(),
        acquired_on: p
            .as_ref()
            .and_then(|p| p.acquired_on)
            .map(|d| d.to_string())
            .unwrap_or_default(),
        amount: p
            .as_ref()
            .map(|p| crate::currency::表示(p.amount, 通貨))
            .unwrap_or_default(),
        supplier: p
            .as_ref()
            .and_then(|p| p.supplier.clone())
            .unwrap_or_default(),
    }
}

/// 購入の記録の入力（設計書10.2）。
#[derive(Debug, Default, Deserialize)]
pub struct PurchaseInput {
    /// 自由入力。**同じ番号を別の機器に入れてよい。**
    #[serde(default)]
    pub order_number: String,
    #[serde(default)]
    pub acquisition_date: String,
    #[serde(default)]
    pub acquisition_cost: String,
    /// 買った相手。**`VENDOR` から選ばせない**——代理店・商社から買うのが普通
    #[serde(default)]
    pub supplier: String,
}

/// 検証を通った購入の記録。
struct 購入の値 {
    order_number: Option<String>,
    acquired_on: Option<NaiveDate>,
    amount: i64,
    supplier: Option<String>,
}

impl PurchaseInput {
    /// **どの欄も空なら `Ok(None)`**（購入の記録を作らない）。
    fn 検証(&self, 通貨: &str) -> Result<Option<購入の値>, &'static str> {
        let order_number = 空ならnone(&self.order_number);
        let supplier = 空ならnone(&self.supplier);
        let acquired_on = match self.acquisition_date.trim() {
            "" => None,
            v => Some(日付(v).ok_or("devices.error_acquisition_date")?),
        };
        let amount = match self.acquisition_cost.trim() {
            "" => None,
            v => Some(crate::currency::最小単位へ(v, 通貨).ok_or("costs.error_amount")?),
        };
        if order_number.is_none() && supplier.is_none() && acquired_on.is_none() && amount.is_none()
        {
            return Ok(None);
        }
        Ok(Some(購入の値 {
            order_number,
            acquired_on,
            amount: amount.unwrap_or(0),
            supplier,
        }))
    }
}

/// 購入の記録を保存する（設計書10.2）。**1台につき1行なので上書きする。**
pub async fn save_purchase(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<PurchaseInput>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let d = 対象(&state, project_id, device_id).await?;
    let 誤り = |key: &str| AppError::Validation(rust_i18n::t!(key, locale = "ja").to_string());

    let Some(値) = form.検証(&project.currency).map_err(誤り)? else {
        return Ok(
            Redirect::to(&format!("/projects/{project_id}/devices/{device_id}")).into_response(),
        );
    };
    // **読み取りはトランザクションを開く前に済ませる**（SQLiteで自分の書き込みロックを待つ）
    let 既存 = 購入の記録(&state.db, d.id).await?;

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    購入を書く(&tx, d.id, 既存, 値, Utc::now()).await?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/devices/{device_id}")).into_response())
}

/// 購入の記録を書く。あれば上書き、無ければ作る。
async fn 購入を書く(
    tx: &AuditedTx,
    device_id: i32,
    既存: Option<purchase::Model>,
    値: 購入の値,
    now: chrono::DateTime<Utc>,
) -> AppResult<()> {
    match 既存 {
        Some(既存) => {
            let mut active: purchase::ActiveModel = 既存.clone().into();
            active.order_number = Set(値.order_number);
            active.acquired_on = Set(値.acquired_on);
            active.amount = Set(値.amount);
            active.supplier = Set(値.supplier);
            active.updated_at = Set(now);
            tx.update(&既存, active)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
        None => {
            tx.insert(purchase::ActiveModel {
                item_type: Set(DEVICE_ITEM.to_owned()),
                item_id: Set(device_id),
                order_number: Set(値.order_number),
                acquired_on: Set(値.acquired_on),
                amount: Set(値.amount),
                supplier: Set(値.supplier),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
    }
    Ok(())
}

fn 空ならnone(value: &str) -> Option<String> {
    match value.trim() {
        "" => None,
        v => Some(v.to_owned()),
    }
}

/// 画面の日付。`2026-09-23` も `2026/09/23` も読む（[`crate::date::読む`]）。
fn 日付(value: &str) -> Option<NaiveDate> {
    crate::date::読む(value)
}

/// 現在の搭載位置（`DEVICE_MOUNT` の `to_date IS NULL`）。
async fn 搭載位置(
    state: &AppState,
    d: &device::Model,
    locale: &str,
) -> AppResult<Vec<Labeled>> {
    let Some(m) = device_mount::Entity::find()
        .filter(device_mount::Column::DeviceId.eq(d.id))
        .filter(device_mount::Column::ToDate.is_null())
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    else {
        return Ok(Vec::new());
    };

    let mut rows = Vec::new();

    // container_id と host_device_id は排他（設計書12.2）
    if let Some(container_id) = m.container_id {
        let name = entity::mount_container::Entity::find_by_id(container_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|c| c.name)
            .unwrap_or_default();
        rows.push(Labeled {
            label: rust_i18n::t!("devices.container", locale = locale).to_string(),
            value: name,
        });
    }
    if let Some(host_id) = m.host_device_id {
        let name = device::Entity::find_by_id(host_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|h| h.hostname)
            .unwrap_or_default();
        rows.push(Labeled {
            label: rust_i18n::t!("devices.host_device", locale = locale).to_string(),
            value: name,
        });
    }
    if let Some(position) = m.position {
        rows.push(Labeled {
            label: rust_i18n::t!("devices.position", locale = locale).to_string(),
            value: position.to_string(),
        });
    }

    Ok(rows)
}

/// 現在搭載されている部品。
async fn 搭載部品(state: &AppState, d: &device::Model) -> AppResult<Vec<Labeled>> {
    let locations = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq("Device"))
        .filter(part_instance_location::Column::LocationId.eq(d.id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for l in locations {
        let Some(instance) = part_instance::Entity::find_by_id(l.part_instance_id)
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

        rows.push(Labeled {
            label: catalog
                .as_ref()
                .map(|c| format!("{} {}", c.category, c.part_number))
                .unwrap_or_default(),
            value: instance.serial_number.unwrap_or_default(),
        });
    }
    Ok(rows)
}

/// 現在有効なファームウェアの版。
async fn ファームウェア(state: &AppState, d: &device::Model) -> AppResult<Vec<Labeled>> {
    Ok(firmware_version::Entity::find()
        .filter(firmware_version::Column::ItemType.eq("Device"))
        .filter(firmware_version::Column::ItemId.eq(d.id))
        .filter(firmware_version::Column::ToDate.is_null())
        .order_by_asc(firmware_version::Column::Component)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|f| Labeled {
            label: f.component,
            value: f.version,
        })
        .collect())
}

/// スタックの構成筐体（`device_type="Logical"` の場合）。
async fn スタック構成(state: &AppState, d: &device::Model) -> AppResult<Vec<Labeled>> {
    let members = device_stack::Entity::find()
        .filter(device_stack::Column::LogicalDeviceId.eq(d.id))
        .filter(device_stack::Column::ToDate.is_null())
        .order_by_asc(device_stack::Column::MemberNumber)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for m in members {
        let name = device::Entity::find_by_id(m.member_device_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|x| x.hostname)
            .unwrap_or_default();
        rows.push(Labeled {
            label: format!("#{}", m.member_number),
            value: name,
        });
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// 登録・編集
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct DeviceForm {
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub device_type: String,
    #[serde(default)]
    pub configuration_id: String,
    #[serde(default)]
    pub device_category: String,
    #[serde(default)]
    pub serial_number: String,
    #[serde(default)]
    pub asset_number: String,
    #[serde(default)]
    pub power_watt: String,
    #[serde(default)]
    pub status: String,
}

/// 登録の入力（設計書16.1）。機器の欄に、お金の記録と送信後の行き先が加わる。
///
/// **隠れている欄の値も送信される**（CSSで隠しても `<input>` は残る）。形態に
/// 合わない項目・開いていない段の項目は、[`検証`] と [`費用を検証`] が捨てる。
#[derive(Debug, Default, Deserialize)]
pub struct NewDeviceForm {
    #[serde(flatten)]
    pub device: DeviceForm,
    #[serde(flatten)]
    pub purchase: PurchaseInput,
    /// チェックボックス。入っていれば `"1"`
    #[serde(default)]
    pub manage_as_fixed_asset: String,
    #[serde(default)]
    pub useful_life_years: String,
    #[serde(default)]
    pub depreciation_method: String,
    #[serde(default)]
    pub register_maintenance: String,
    /// `existing`（既定）/ `new`
    #[serde(default)]
    pub maintenance_mode: String,
    #[serde(default)]
    pub maintenance_contract_id: String,
    #[serde(default)]
    pub contract_number: String,
    #[serde(default)]
    pub contract_vendor_id: String,
    #[serde(default)]
    pub contract_start: String,
    #[serde(default)]
    pub contract_end: String,
    #[serde(default)]
    pub contract_amount: String,
    #[serde(default)]
    pub quote_contact: String,
    #[serde(default)]
    pub failure_contact: String,
    /// 送信ボタンの別。`again` なら同じ画面へ戻る（設計書16.1）
    #[serde(default)]
    pub then: String,
}

impl NewDeviceForm {
    fn 資産として管理する(&self) -> bool {
        !self.manage_as_fixed_asset.trim().is_empty()
    }
    fn 保守契約に含める(&self) -> bool {
        !self.register_maintenance.trim().is_empty()
    }
    fn 新しい契約(&self) -> bool {
        self.maintenance_mode.trim() == "new"
    }
}

/// 検証を通った入力。**保存してよいのはここに入った値だけ。**
struct 検証済み {
    hostname: String,
    device_type: String,
    configuration_id: Option<i32>,
    device_category: Option<String>,
    serial_number: Option<String>,
    asset_number: Option<String>,
    power_watt: i32,
    status: String,
}

impl DeviceForm {
    fn 空ならnone(value: &str) -> Option<String> {
        空ならnone(value)
    }

    /// 語彙に含まれていれば返す。**含まれていなければ `None`。**
    ///
    /// **既定値へ倒さない。**選択肢はサーバが描画しているため、語彙外の値が
    /// 届くのは改竄かクライアントの不具合しかありえない。黙って別の値を保存
    /// すると、どちらの場合も気付けない（設計書8.6、Q-21）。
    fn 語彙(value: &str, allowed: &[&'static str]) -> Option<String> {
        allowed
            .iter()
            .find(|v| **v == value.trim())
            .map(|v| (*v).to_owned())
    }
}

/// 入力を検証する。誤りがあれば表示用のi18nキーを返す。
///
/// **型・語彙の整合性は緩めない。**不変条件6の「検証は原則ハードな禁止ではなく
/// 警告」は「実機が仕様の想定外でありうる」ことへの配慮（スロット本数の超過等）
/// であって、送られてきた値が解釈できない場合の話ではない。
///
/// # 形態で変わる必須は、ここで確かめる（設計書16.1）
///
/// **画面の `required` は常に見えている欄（ホスト名）だけに付けている。**CSSで
/// 隠れた欄に付けると、空のまま生き残ってフォーム全体が送信できなくなるため
/// である。`Physical` の構成、それ以外の種別分類は、ここで必須にする。
async fn 検証(
    state: &AppState,
    form: &DeviceForm,
    新規: bool,
) -> AppResult<Result<検証済み, &'static str>> {
    let hostname = form.hostname.trim().to_owned();
    if hostname.is_empty() {
        return Ok(Err("devices.hostname_required"));
    }

    let Some(device_type) = DeviceForm::語彙(&form.device_type, DEVICE_TYPES) else {
        return Ok(Err("devices.device_type_invalid"));
    };
    // **予約は登録から作らない**（設計書11.6）。編集では予約中の機器もありうる
    let 選べる状態 = if 新規 { STATUSES_ON_CREATE } else { STATUSES };
    let Some(status) = DeviceForm::語彙(&form.status, 選べる状態) else {
        return Ok(Err("devices.status_invalid"));
    };
    let 物理 = device_type == PHYSICAL;

    // **形態に合わない項目は捨てる。**隠れた欄の値も送信されるため
    let device_category = if 物理 {
        None
    } else {
        match DeviceForm::空ならnone(&form.device_category) {
            None => return Ok(Err("devices.device_category_required")),
            Some(value) => match DeviceForm::語彙(&value, DEVICE_CATEGORIES) {
                Some(v) => Some(v),
                None => return Ok(Err("devices.device_category_invalid")),
            },
        }
    };

    // Virtual / Container / Logical は構成を持たない（設計書6.2）
    let configuration_id = if !物理 {
        None
    } else {
        match DeviceForm::空ならnone(&form.configuration_id) {
            None => return Ok(Err("devices.configuration_required")),
            Some(value) => {
                let Ok(id) = value.parse::<i32>() else {
                    return Ok(Err("devices.configuration_invalid"));
                };
                // **存在も確かめる。**確かめないと外部キー違反で500になり、
                // 利用者には何が悪いのか分からない
                let 実在 = configuration::Entity::find_by_id(id)
                    .one(&state.db)
                    .await
                    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                    .is_some();
                if !実在 {
                    return Ok(Err("devices.configuration_invalid"));
                }
                Some(id)
            }
        }
    };

    let power_watt = match form.power_watt.trim() {
        "" => 0,
        value => match value.parse::<i32>() {
            Ok(v) if v >= 0 => v,
            _ => return Ok(Err("devices.power_watt_invalid")),
        },
    };

    Ok(Ok(検証済み {
        hostname,
        device_type,
        configuration_id,
        device_category,
        // 仮想機器・コンテナ・論理機器にシリアル番号は無い（設計書6.2）
        serial_number: if 物理 {
            DeviceForm::空ならnone(&form.serial_number)
        } else {
            None
        },
        // 採番待ちでも登録できる（設計書23.2）
        asset_number: DeviceForm::空ならnone(&form.asset_number),
        power_watt,
        status,
    }))
}

/// 検証を通ったお金の記録（設計書16.1、10.2）。
struct 費用 {
    購入: Option<購入の値>,
    /// （耐用年数, 償却方法）。取得原価と取得日は購入の値を使う
    資産: Option<(i32, String)>,
    保守: Option<保守契約>,
}

enum 保守契約 {
    既存(i32),
    /// 大きいので箱に入れる（clippy::large_enum_variant）
    新規(Box<maintenance_contract::ActiveModel>),
}

/// お金の記録を検証する。**読み取りはここで済ませる**（トランザクションの前）。
///
/// - 取得原価・取得日・発注番号・購入元は、固定資産として管理するかに関わらず受ける
/// - **固定資産として管理するなら、取得原価と取得日は必須**（`FIXED_ASSET` の列が
///   NOT NULL）。そのうえで耐用年数と償却方法だけを足して聞く
async fn 費用を検証(
    state: &AppState,
    project: &project::Model,
    form: &NewDeviceForm,
) -> AppResult<Result<費用, &'static str>> {
    let 通貨 = project.currency.as_str();
    let mut 購入 = match form.purchase.検証(通貨) {
        Ok(v) => v,
        Err(key) => return Ok(Err(key)),
    };

    let 資産 = if form.資産として管理する() {
        // **欠けているのか読めないのかを分けて伝える。**「形が違う」と言われても、
        // 空欄のまま送った利用者には何を直せばよいか分からない
        if form.purchase.acquisition_cost.trim().is_empty() {
            return Ok(Err("devices.error_asset_cost_required"));
        }
        if !購入.as_ref().is_some_and(|p| p.acquired_on.is_some()) {
            return Ok(Err("devices.error_asset_date_required"));
        }
        // **閉じた語彙は既定へ寄せず拒否する**（8.6、Q-21）
        if !crate::cost::DEPRECIATION_METHODS.contains(&form.depreciation_method.trim()) {
            return Ok(Err("costs.error_method"));
        }
        // **耐用年数が0以下は按分できない**（24.2.2）
        let life = match form.useful_life_years.trim().parse::<i32>() {
            Ok(n) if n > 0 => n,
            _ => return Ok(Err("costs.error_life")),
        };
        Some((life, form.depreciation_method.trim().to_owned()))
    } else {
        None
    };
    // 固定資産として管理しないなら、購入は入力があったときだけ作る
    if 資産.is_none()
        && 購入.as_ref().is_some_and(|p| p.amount == 0)
        && form.purchase.acquisition_cost.trim().is_empty()
        && 購入.as_ref().is_some_and(|p| {
            p.order_number.is_none() && p.supplier.is_none() && p.acquired_on.is_none()
        })
    {
        購入 = None;
    }

    let 保守 = if !form.保守契約に含める() {
        None
    } else if form.新しい契約() {
        let number = crate::server::catalog::正規化(&form.contract_number);
        if number.is_empty() {
            return Ok(Err("costs.error_contract_number"));
        }
        if form.contract_start.trim().is_empty() || form.contract_end.trim().is_empty() {
            return Ok(Err("devices.error_contract_dates_required"));
        }
        let (Some(start), Some(end)) = (日付(&form.contract_start), 日付(&form.contract_end))
        else {
            return Ok(Err("devices.error_contract_dates"));
        };
        // **期間が0以下は按分できない**（24.2.2）
        if end < start {
            return Ok(Err("costs.error_period"));
        }
        let Some(amount) = crate::currency::最小単位へ(&form.contract_amount, 通貨) else {
            return Ok(Err("costs.error_amount"));
        };
        let Ok(vendor_id) = form.contract_vendor_id.trim().parse::<i32>() else {
            return Ok(Err("cables.error_vendor"));
        };
        let ベンダー = vendor::Entity::find_by_id(vendor_id)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if ベンダー.is_none() {
            return Ok(Err("cables.error_vendor"));
        }
        Some(保守契約::新規(Box::new(
            maintenance_contract::ActiveModel {
                contract_number: Set(number),
                vendor_id: Set(vendor_id),
                start_date: Set(start),
                end_date: Set(end),
                amount: Set(amount),
                quote_contact: Set(crate::server::catalog::正規化(&form.quote_contact)),
                failure_contact: Set(crate::server::catalog::正規化(&form.failure_contact)),
                order_number: Set(None),
                ..Default::default()
            },
        )))
    } else {
        let Ok(id) = form.maintenance_contract_id.trim().parse::<i32>() else {
            return Ok(Err("costs.error_contract"));
        };
        // **このプロジェクトから見える契約に限る**（コスト画面の一覧と同じ範囲）
        let 見える = super::cost::このプロジェクトの契約(state, project.id)
            .await?
            .iter()
            .any(|c| c.id == id);
        if !見える {
            return Ok(Err("costs.error_contract"));
        }
        Some(保守契約::既存(id))
    };

    Ok(Ok(費用 {
        購入, 資産, 保守
    }))
}

/// 「もう1台」で戻るときの引き継ぎ（設計書16.1）。
#[derive(Debug, Default, Deserialize)]
pub struct NewQuery {
    /// 直前に登録した機器。形態・構成・状態・保守契約をここから引き継ぐ
    #[serde(default)]
    pub again: Option<i32>,
}

pub async fn new_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<NewQuery>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let mut form = NewDeviceForm {
        device: DeviceForm {
            device_type: PHYSICAL.to_owned(),
            status: "provisioning".to_owned(),
            ..Default::default()
        },
        maintenance_mode: "existing".to_owned(),
        depreciation_method: crate::cost::STRAIGHT_LINE.to_owned(),
        ..Default::default()
    };

    // **ホスト名・シリアル番号・資産番号は引き継がない。**1台ごとに違う値である
    let mut registered = None;
    if let Some(id) = query.again {
        let 前 = 対象(&state, project_id, id).await?;
        form.device.device_type = 前.device_type.clone();
        form.device.configuration_id = 前
            .configuration_id
            .map(|v| v.to_string())
            .unwrap_or_default();
        form.device.device_category = 前.device_category.clone().unwrap_or_default();
        form.device.status = 前.status.clone();
        if let Some(item) = maintenance_contract_item::Entity::find()
            .filter(maintenance_contract_item::Column::ItemType.eq(DEVICE_ITEM))
            .filter(maintenance_contract_item::Column::ItemId.eq(前.id))
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        {
            form.register_maintenance = "1".to_owned();
            form.maintenance_contract_id = item.maintenance_contract_id.to_string();
        }
        registered = Some(前.hostname);
    }

    render(&フォーム(&state, &current, &project, None, &form, registered, None).await?)
}

async fn フォーム(
    state: &AppState,
    current: &CurrentUser,
    project: &project::Model,
    device_id: Option<i32>,
    form: &NewDeviceForm,
    registered: Option<String>,
    error: Option<String>,
) -> AppResult<DeviceFormPage> {
    let l = Locale::parse(&current.user.locale).as_str();
    let 新規 = device_id.is_none();
    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    let d = &form.device;

    let (contracts, vendors) = if 新規 {
        let contracts = super::cost::このプロジェクトの契約(state, project.id)
            .await?
            .into_iter()
            .map(|c| Labeled {
                label: format!("{} / {} 〜 {}", c.contract_number, c.start_date, c.end_date),
                value: c.id.to_string(),
            })
            .collect();
        let vendors = vendor::Entity::find()
            .filter(vendor::Column::RetiredAt.is_null())
            .filter(vendor::Column::MergedIntoVendorId.is_null())
            .order_by_asc(vendor::Column::Name)
            .all(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .into_iter()
            .map(|v| Labeled {
                label: v.name,
                value: v.id.to_string(),
            })
            .collect();
        (contracts, vendors)
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(DeviceFormPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            project,
            "devices",
        )
        .await,
        project_id: project.id,
        project_name: project.name.clone(),
        is_new: 新規,
        t: FormText {
            title: t(if 新規 {
                "devices.new_title"
            } else {
                "devices.edit_title"
            }),
            lead: t(if 新規 {
                "devices.new_lead"
            } else {
                "devices.edit_lead"
            }),
            import_hint: t("devices.import_hint"),
            back: t("devices.back"),
            submit: t(if 新規 {
                "devices.register"
            } else {
                "common.save"
            }),
            submit_again: t("devices.register_again"),
            again_hint: t("devices.register_again_hint"),
            required: t("devices.required"),
            optional: t("devices.optional"),
            registered: t("devices.registered"),
            sec_kind: t("devices.sec_kind"),
            sec_kind_hint: t("devices.sec_kind_hint"),
            sec_identity: t("devices.sec_identity"),
            sec_identity_hint: t("devices.sec_identity_hint"),
            sec_state: t("devices.sec_state"),
            sec_money: t("devices.sec_money"),
            sec_money_hint: t("devices.sec_money_hint"),
            device_type: t("devices.form_type"),
            tip_physical: t("devices.tip_physical"),
            tip_virtual: t("devices.tip_virtual"),
            tip_container: t("devices.tip_container"),
            tip_logical: t("devices.tip_logical"),
            configuration: t("devices.configuration"),
            configuration_hint: t("devices.configuration_hint"),
            device_category: t("devices.device_category"),
            device_category_hint: t("devices.device_category_hint"),
            hostname: t("devices.hostname"),
            hostname_hint: t("devices.hostname_hint"),
            serial_number: t("devices.serial_number"),
            serial_hint: t("devices.serial_hint"),
            asset_number: t("devices.asset_number"),
            asset_hint: t("devices.asset_hint"),
            status: t("devices.status"),
            status_hint: t(if 新規 {
                "devices.status_hint_new"
            } else {
                "devices.status_hint_edit"
            }),
            power_watt: t("devices.power_watt"),
            power_hint: t("devices.power_hint"),
            order_number: t("devices.order_number"),
            order_number_hint: t("devices.order_number_hint"),
            supplier: t("devices.supplier"),
            supplier_hint: t("devices.supplier_hint"),
            acquisition_cost: t("devices.acquisition_cost"),
            acquisition_cost_hint: t("costs.amount_hint"),
            acquisition_date: t("devices.acquisition_date"),
            acquisition_date_hint: t("devices.acquisition_date_hint"),
            manage_as_fixed_asset: t("devices.manage_as_fixed_asset"),
            fixed_asset_hint: t("devices.fixed_asset_hint"),
            useful_life_years: t("costs.useful_life"),
            depreciation_method: t("costs.method"),
            depreciation_hint: t("costs.method_hint"),
            register_maintenance: t("devices.register_maintenance"),
            maintenance_existing: t("devices.maintenance_existing"),
            maintenance_new: t("devices.maintenance_new"),
            maintenance_contract: t("devices.maintenance_contract"),
            maintenance_contract_hint: t("devices.maintenance_contract_hint"),
            contract_number: t("costs.contract_number"),
            contract_vendor: t("catalog.vendor"),
            contract_start: t("costs.start_date"),
            contract_end: t("costs.end_date"),
            contract_end_hint: t("devices.contract_end_hint"),
            contract_amount: t("costs.amount"),
            quote_contact: t("costs.quote_contact"),
            failure_contact: t("costs.failure_contact"),
            failure_contact_hint: t("costs.contact_hint"),
            next_steps: t("devices.next_steps"),
            next_steps_lead: t("devices.next_steps_lead"),
            step_register: t("devices.step_register"),
            step_register_where: t("devices.step_register_where"),
            step_parts: t("devices.step_parts"),
            step_parts_where: t("devices.step_parts_where"),
            step_interfaces: t("devices.step_interfaces"),
            step_interfaces_where: t("devices.step_interfaces_where"),
            step_sbom: t("devices.step_sbom"),
            step_sbom_where: t("devices.step_sbom_where"),
        },
        action: match device_id {
            Some(id) => format!("/projects/{}/devices/{id}", project.id),
            None => format!("/projects/{}/devices", project.id),
        },
        hostname: d.hostname.clone(),
        device_type: d.device_type.clone(),
        device_types: DEVICE_TYPES.to_vec(),
        configuration_id: d.configuration_id.clone(),
        configurations: 構成の候補(state).await?,
        device_category: d.device_category.clone(),
        categories: 種別の選択肢(l),
        serial_number: d.serial_number.clone(),
        asset_number: d.asset_number.clone(),
        power_watt: d.power_watt.clone(),
        status: d.status.clone(),
        statuses: 状態の選択肢(if 新規 { STATUSES_ON_CREATE } else { STATUSES }, l),
        registered,
        currency: project.currency.clone(),
        order_number: form.purchase.order_number.clone(),
        supplier: form.purchase.supplier.clone(),
        acquisition_cost: form.purchase.acquisition_cost.clone(),
        acquisition_date: form.purchase.acquisition_date.clone(),
        manage_as_fixed_asset: form.資産として管理する(),
        useful_life_years: form.useful_life_years.clone(),
        depreciation_method: form.depreciation_method.clone(),
        methods: crate::cost::DEPRECIATION_METHODS
            .iter()
            .map(|v| Choice {
                value: v,
                label: t(&format!("costs.method_{v}")),
            })
            .collect(),
        register_maintenance: form.保守契約に含める(),
        maintenance_new: form.新しい契約(),
        maintenance_contract_id: form.maintenance_contract_id.clone(),
        contracts,
        contract_number: form.contract_number.clone(),
        contract_vendor_id: form.contract_vendor_id.clone(),
        vendors,
        contract_start: form.contract_start.clone(),
        contract_end: form.contract_end.clone(),
        contract_amount: form.contract_amount.clone(),
        quote_contact: form.quote_contact.clone(),
        failure_contact: form.failure_contact.clone(),
        error,
    })
}

/// 構成の選択肢。カタログはプロジェクト横断の共有マスタ（設計書18.1）。
async fn 構成の候補(state: &AppState) -> AppResult<Vec<Labeled>> {
    let configs = configuration::Entity::find()
        .find_also_related(chassis_model::Entity)
        .order_by_asc(configuration::Column::Name)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(configs
        .into_iter()
        .map(|(c, model)| Labeled {
            label: match model {
                Some(m) => format!("{} / {}", m.model_name, c.name),
                None => c.name.clone(),
            },
            value: c.id.to_string(),
        })
        .collect())
}

/// 機器を登録する（設計書16.1）。
///
/// **1回の送信で複数のテーブルに書く**（機器・所在・購入・固定資産・保守契約）。
/// 途中で失敗したら機器も作らない。
pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<NewDeviceForm>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    // **読み取りはすべてトランザクションの前に済ませる**（SQLiteで自分の
    // 書き込みロックを待って止まる）。検証が読むのもここまで
    let 入力 = match 検証(&state, &form.device, true).await? {
        Ok(値) => 値,
        Err(key) => {
            let page = フォーム(
                &state,
                &current,
                &project,
                None,
                &form,
                None,
                Some(rust_i18n::t!(key, locale = l).to_string()),
            )
            .await?;
            return render(&page);
        }
    };
    let 費用 = match 費用を検証(&state, &project, &form).await? {
        Ok(値) => 値,
        Err(key) => {
            let page = フォーム(
                &state,
                &current,
                &project,
                None,
                &form,
                None,
                Some(rust_i18n::t!(key, locale = l).to_string()),
            )
            .await?;
            return render(&page);
        }
    };

    let now = Utc::now();
    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let created = tx
        .insert(device::ActiveModel {
            // 名前を変えても参照が切れない不変の識別子（設計書23.2）
            uid: Set(uuid::Uuid::new_v4().to_string()),
            external_id: Set(None),
            merged_into_device_id: Set(None),
            merged_at: Set(None),
            configuration_id: Set(入力.configuration_id),
            device_type: Set(入力.device_type),
            device_category: Set(入力.device_category),
            hostname: Set(入力.hostname),
            serial_number: Set(入力.serial_number),
            asset_number: Set(入力.asset_number),
            power_watt: Set(入力.power_watt),
            status: Set(入力.status),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **所在は DEVICE から導出するため、割当の行が要る**（旧B-1）。
    // これを書かないと、登録した機器がどの一覧にも出てこない
    tx.insert(device_assignment::ActiveModel {
        device_id: Set(created.id),
        location_type: Set(PROJECT.to_owned()),
        location_id: Set(Some(project_id)),
        work_order_id: Set(None),
        from_date: Set(now),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // --- お金の記録（設計書16.1、10.2） ---
    if let Some(購入) = 費用.購入 {
        // **固定資産には購入の値を複製する。**同じ値を2度入力させない
        let 資産の元 = (購入.amount, 購入.acquired_on);
        購入を書く(&tx, created.id, None, 購入, now).await?;
        if let (Some((life, method)), (cost, Some(date))) = (費用.資産, 資産の元) {
            tx.insert(fixed_asset::ActiveModel {
                item_type: Set(DEVICE_ITEM.to_owned()),
                item_id: Set(created.id),
                acquisition_cost: Set(cost),
                depreciation_method: Set(method),
                useful_life_years: Set(life),
                acquisition_date: Set(date),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
    }

    if let Some(保守) = 費用.保守 {
        let contract_id = match 保守 {
            保守契約::既存(id) => id,
            保守契約::新規(active) => {
                let mut active = *active;
                active.created_at = Set(now);
                active.updated_at = Set(now);
                tx.insert(active)
                    .await
                    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
                    .id
            }
        };
        tx.insert(maintenance_contract_item::ActiveModel {
            maintenance_contract_id: Set(contract_id),
            item_type: Set(DEVICE_ITEM.to_owned()),
            item_id: Set(created.id),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // 「もう1台」は同じ画面へ、そうでなければ機器の詳細へ——**このあとの手順
    // （部品・インターフェース・SBOM）は詳細から始まる**（設計書16.1）
    let 行き先 = if form.then.trim() == "again" {
        format!("/projects/{project_id}/devices/new?again={}", created.id)
    } else {
        format!("/projects/{project_id}/devices/{}", created.id)
    };
    Ok(Redirect::to(&行き先).into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let d = 対象(&state, project_id, device_id).await?;
    let form = NewDeviceForm {
        device: 既存の値(&d),
        ..Default::default()
    };
    let page = フォーム(&state, &current, &project, Some(d.id), &form, None, None).await?;
    render(&page)
}

fn 既存の値(d: &device::Model) -> DeviceForm {
    DeviceForm {
        hostname: d.hostname.clone(),
        device_type: d.device_type.clone(),
        configuration_id: d
            .configuration_id
            .map(|v| v.to_string())
            .unwrap_or_default(),
        device_category: d.device_category.clone().unwrap_or_default(),
        serial_number: d.serial_number.clone().unwrap_or_default(),
        asset_number: d.asset_number.clone().unwrap_or_default(),
        power_watt: d.power_watt.to_string(),
        status: d.status.clone(),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<DeviceForm>,
) -> AppResult<Response> {
    let project = 編集入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let target = 対象(&state, project_id, device_id).await?;

    let 入力 = match 検証(&state, &form, false).await? {
        Ok(値) => 値,
        Err(key) => {
            let form = NewDeviceForm {
                device: form,
                ..Default::default()
            };
            let page = フォーム(
                &state,
                &current,
                &project,
                Some(target.id),
                &form,
                None,
                Some(rust_i18n::t!(key, locale = l).to_string()),
            )
            .await?;
            return render(&page);
        }
    };

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mut active: device::ActiveModel = target.clone().into();
    active.hostname = Set(入力.hostname);
    active.device_type = Set(入力.device_type);
    active.configuration_id = Set(入力.configuration_id);
    active.device_category = Set(入力.device_category);
    active.serial_number = Set(入力.serial_number);
    active.asset_number = Set(入力.asset_number);
    active.power_watt = Set(入力.power_watt);
    active.status = Set(入力.status);
    active.updated_at = Set(Utc::now());
    tx.update(&target, active)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/devices/{device_id}")).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// 閲覧の可否を確かめ、プロジェクトと「編集できるか」を返す。
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

/// 編集の可否を確かめる。**ApproverとViewerはここで止まる。**
async fn 編集入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<project::Model> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    if !can_edit {
        return Err(AppError::Forbidden);
    }
    Ok(project)
}

/// 対象の機器。**このプロジェクトから見える機器に限る**（A-6）。
///
/// IDを直接叩かれても、所属したことのないプロジェクトからは見えない。
async fn 対象(state: &AppState, project_id: i32, device_id: i32) -> AppResult<device::Model> {
    let 見える = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .limit(1)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 表示範囲の既定は現在所属しているもの() {
        assert_eq!(Scope::parse(None), Scope::Current);
        assert_eq!(Scope::parse(Some("all")), Scope::All);
    }

    /// **語彙外は `None`。既定値へ倒さない。**倒すと改竄も不具合も気付けない。
    #[test]
    fn 語彙外の値は受け付けない() {
        assert_eq!(
            DeviceForm::語彙("Virtual", DEVICE_TYPES),
            Some("Virtual".to_owned())
        );
        assert_eq!(DeviceForm::語彙("なりすまし", DEVICE_TYPES), None);
        assert_eq!(DeviceForm::語彙("", STATUSES), None);
    }

    #[test]
    fn 空欄はnullとして扱う() {
        assert_eq!(DeviceForm::空ならnone("  "), None);
        assert_eq!(DeviceForm::空ならnone(" S1 "), Some("S1".to_owned()));
    }
}
