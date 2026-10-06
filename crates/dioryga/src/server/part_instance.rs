//! プロジェクトの部品の画面（設計書6.2、12.4、16.1）。
//!
//! 部品の実物（`PART_INSTANCE`）を一覧し、登録し、置き場所を移す。部品は
//! プロジェクトを列に持たないため、**このプロジェクトの部品かどうかは置き場所から
//! たどる**（[`crate::part_location`]）。
//!
//! # 置けるのはこのプロジェクトの中だけ
//!
//! 置き場所は、このプロジェクトに今ある機器・このプロジェクトの設備・什器
//! （撤去したものを除く）・このプロジェクトのいずれか。他のプロジェクトへ移すのは
//! 変更管理チケットの担当であり、廃棄もチケットで行う（11章）。部品の取込
//! （`import/parts.rs`）と同じ範囲である。
//!
//! # 置き場所の移動は閉じて開く
//!
//! `PART_INSTANCE_LOCATION` は履歴である（不変条件1）。現行の行を閉じ、新しい行を
//! 開く。**変更管理チケットは通さない**——誰がいつ移したかは監査ログで追う。
//!
//! # ケーブルが挿さっている部品は動かさない
//!
//! 現行のケーブルの接続がある部品を、機器から外す・別の場所へ移すことは拒否する。
//! 先にポート接続の画面で外させる。自動では外さない（移譲・廃棄と同じ）。

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    cable_connection, device, device_assignment, mount_container, part_catalog, part_instance,
    part_instance_location, project, vendor,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::device_state;
use crate::error::{AppError, AppResult};
use crate::part_location::{self, DEVICE, MOUNT_CONTAINER, PROJECT};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::Labeled;
use crate::server::view::{
    render, チケット番号, 保管中の表示, 容体の表示, 容体の選択肢, 状態の表示, 状態の選択肢, Choice,
    Chrome, Locale, STORED,
};
use crate::server::AppState;

const DISPOSED: &str = "Disposed";

/// 一度に `IN` へ渡す件数。
const まとめて引く件数: usize = 500;

/// 一覧の絞り込み（置き場所の種類）。`(クエリの値, location_type)`。
const 絞り込み: &[(&str, &str)] = &[
    ("device", DEVICE),
    ("container", MOUNT_CONTAINER),
    ("project", PROJECT),
];

fn 内部(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(anyhow::anyhow!(e))
}

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct PartRow {
    id: i32,
    category: String,
    model: String,
    serial_number: String,
    status: String,
    status_label: String,
    /// 故障していれば表示名。正常なら `None`。
    attention: Option<String>,
    place: String,
    /// 置き場所の画面。プロジェクトに置いてある部品には無い。
    place_href: Option<String>,
    has_cable: bool,
}

#[derive(askama::Template)]
#[template(path = "part_instances.html")]
struct PartsPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_category: String,
    t_model: String,
    t_serial_number: String,
    t_status: String,
    t_health: String,
    t_place: String,
    t_actions: String,
    t_open: String,
    t_empty: String,
    t_cable: String,
    t_at_all: String,
    t_at_device: String,
    t_at_container: String,
    t_at_project: String,
    t_apply: String,
    t_new: String,
    t_serial_hint: String,
    t_unset: String,
    t_submit: String,
    t_devices: String,
    t_containers: String,
    t_here: String,
    rows: Vec<PartRow>,
    at: String,
    catalogs: Vec<Labeled>,
    statuses: Vec<Choice>,
    healths: Vec<Choice>,
    places: 置き場所の候補,
    can_edit: bool,
    /// **誤りのときは入力を保ったまま戻す**（16.1）。
    form: PartForm,
    error: Option<String>,
}

struct HistoryRow {
    place: String,
    period: String,
    /// 変更管理チケットで動いた行だけが持つ（廃棄など）。
    ticket: Option<Labeled>,
    current: bool,
}

#[derive(askama::Template)]
#[template(path = "part_instance_detail.html")]
struct PartDetailPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    part_id: i32,
    t_parts: String,
    t_basic: String,
    t_category: String,
    t_model: String,
    t_serial_number: String,
    t_status: String,
    t_health: String,
    t_place: String,
    t_history: String,
    t_period: String,
    t_ticket: String,
    t_current: String,
    t_move: String,
    t_move_hint: String,
    t_move_to: String,
    t_devices: String,
    t_containers: String,
    t_here: String,
    t_has_cable: String,
    t_connections: String,
    title: String,
    category: String,
    model: String,
    serial_number: String,
    status: String,
    status_label: String,
    health: String,
    health_label: String,
    place: String,
    place_href: Option<String>,
    /// ケーブルが挿さっている機器のポート接続の画面。挿さっていなければ `None`。
    connections_href: Option<String>,
    history: Vec<HistoryRow>,
    places: 置き場所の候補,
    /// 今の置き場所（`<select>` の値）。移動の候補から外す。
    current_place: String,
    can_edit: bool,
    error: Option<String>,
}

/// 置き場所の候補。機器・設備・什器を `<optgroup>` に分けて出す。
struct 置き場所の候補 {
    devices: Vec<Labeled>,
    containers: Vec<Labeled>,
}

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// `device` / `container` / `project`。空ならすべて。
    #[serde(default)]
    pub at: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let at = query.at.unwrap_or_default();
    一覧を描く(&state, &current, project_id, &at, PartForm::default(), None).await
}

async fn 一覧を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    at: &str,
    form: PartForm,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let db = &state.db;

    // 知らない値は「すべて」として扱う
    let 種類 = 絞り込み.iter().find(|(k, _)| *k == at).map(|(_, v)| *v);
    let 所在: Vec<part_instance_location::Model> =
        part_location::プロジェクトの部品の所在(db, project_id)
            .await
            .map_err(内部)?
            .into_iter()
            .filter(|s| 種類.is_none_or(|t| s.location_type == t))
            .collect();

    // 表示に要る情報をまとめて引く。行ごとに問い合わせるとN+1になる
    let ids: Vec<i32> = 所在.iter().map(|s| s.part_instance_id).collect();
    let mut 部品: HashMap<i32, part_instance::Model> = HashMap::new();
    let mut ケーブルあり: HashSet<i32> = HashSet::new();
    for 組 in ids.chunks(まとめて引く件数) {
        for p in part_instance::Entity::find()
            .filter(part_instance::Column::Id.is_in(組.to_vec()))
            .all(db)
            .await
            .map_err(内部)?
        {
            部品.insert(p.id, p);
        }
        for c in cable_connection::Entity::find()
            .filter(cable_connection::Column::PartInstanceId.is_in(組.to_vec()))
            .filter(cable_connection::Column::ToDate.is_null())
            .all(db)
            .await
            .map_err(内部)?
        {
            ケーブルあり.insert(c.part_instance_id);
        }
    }
    let 型 = 型を引く(db, 部品.values().map(|p| p.part_catalog_id)).await?;
    let 場所 = 場所の名前(db, &所在).await?;
    // **倉庫にある部品の `status` には意味が無い**（設計書6.3）。保管中と出す
    let 倉庫 = crate::setting::倉庫プロジェクトか(db, project_id)
        .await
        .map_err(内部)?;

    let mut rows = Vec::new();
    for s in &所在 {
        let Some(p) = 部品.get(&s.part_instance_id) else {
            continue;
        };
        let (category, model) = 型.get(&p.part_catalog_id).cloned().unwrap_or_default();
        let (place, place_href) = 場所.表示(project_id, s, l);
        rows.push(PartRow {
            id: p.id,
            category,
            model,
            serial_number: p.serial_number.clone().unwrap_or_default(),
            // クラス名には生の値を、文字には表示名を当てる
            status: if 倉庫 {
                STORED.to_owned()
            } else {
                p.status.clone()
            },
            status_label: if 倉庫 {
                保管中の表示(l)
            } else {
                状態の表示(&p.status, l)
            },
            attention: (p.health != device_state::OK).then(|| 容体の表示(&p.health, l)),
            place,
            place_href,
            has_cable: ケーブルあり.contains(&p.id),
        });
    }
    rows.sort_by(|a, b| {
        (&a.category, &a.model, &a.serial_number, a.id).cmp(&(
            &b.category,
            &b.model,
            &b.serial_number,
            b.id,
        ))
    });

    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    render(&PartsPage {
        chrome: Chrome::project(
            db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "parts",
        )
        .await,
        project_id,
        project_name: project.name,
        t_title: t("part_instances.title"),
        t_lead: t("part_instances.lead"),
        t_category: t("part_instances.category"),
        t_model: t("part_instances.model"),
        t_serial_number: t("part_instances.serial_number"),
        t_status: t("part_instances.status"),
        t_health: t("part_instances.health"),
        t_place: t("part_instances.place"),
        t_actions: t("projects.actions"),
        t_open: t("part_instances.open"),
        t_empty: t("part_instances.empty"),
        t_cable: t("part_instances.cable"),
        t_at_all: t("part_instances.at_all"),
        t_at_device: t("part_instances.at_device"),
        t_at_container: t("part_instances.at_container"),
        t_at_project: t("part_instances.at_project"),
        t_apply: t("catalog.apply"),
        t_new: t("part_instances.new"),
        t_serial_hint: t("part_instances.serial_hint"),
        t_unset: t("catalog.unset"),
        t_submit: t("part_instances.submit"),
        t_devices: t("part_instances.devices"),
        t_containers: t("part_instances.containers"),
        t_here: t("part_instances.here"),
        rows,
        at: if 種類.is_some() {
            at.to_owned()
        } else {
            String::new()
        },
        catalogs: if can_edit {
            選べる型(db).await?
        } else {
            Vec::new()
        },
        statuses: 状態の選択肢(device_state::STATUSES, l),
        healths: 容体の選択肢(device_state::HEALTHS, l),
        places: 置き場所を並べる(db, project_id).await?,
        can_edit,
        form,
        error,
    })
}

// ---------------------------------------------------------------------------
// 登録
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct PartForm {
    #[serde(default)]
    pub part_catalog_id: String,
    /// 任意。シリアルの無い部品はこの画面から登録する（取込はシリアル必須、23.5）。
    #[serde(default)]
    pub serial_number: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub health: String,
    /// `Device:{id}` / `MountContainer:{id}` / `Project`。
    #[serde(default)]
    pub place: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    Form(form): Form<PartForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let db = &state.db;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **廃番・統合済みの型は選べない**（18.5）。画面は候補を絞るが、POSTは直接叩ける
    let catalog = match form.part_catalog_id.trim().parse::<i32>() {
        Ok(id) => part_catalog::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(内部)?
            .filter(|c| c.retired_at.is_none() && c.merged_into_part_catalog_id.is_none()),
        Err(_) => None,
    };
    let Some(catalog) = catalog else {
        let e = 誤り("part_instances.error_catalog");
        return 一覧を描く(&state, &current, project_id, "", form, e).await;
    };
    let Some(status) = 語彙(&form.status, device_state::STATUSES) else {
        let e = 誤り("part_instances.error_status");
        return 一覧を描く(&state, &current, project_id, "", form, e).await;
    };
    let Some(health) = 語彙(&form.health, device_state::HEALTHS) else {
        let e = 誤り("part_instances.error_health");
        return 一覧を描く(&state, &current, project_id, "", form, e).await;
    };
    let Some((location_type, location_id)) =
        置き場所を読む(&state, project_id, &form.place).await?
    else {
        let e = 誤り("part_instances.error_place");
        return 一覧を描く(&state, &current, project_id, "", form, e).await;
    };

    // **シリアルはベンダーの中で一意とみなす**（23.10）。取込はベンダー＋シリアルで
    // 突合するため、2つ目を作ると取込がどちらを指すのか決められなくなる
    let serial = crate::server::catalog::正規化(&form.serial_number);
    let serial = (!serial.is_empty()).then_some(serial);
    if let Some(serial) = &serial {
        if 同じシリアルがある(db, catalog.vendor_id, serial).await? {
            let e = 誤り("part_instances.error_serial_taken");
            return 一覧を描く(&state, &current, project_id, "", form, e).await;
        }
    }

    let now = Utc::now();
    let tx = AuditedTx::begin(db, Actor::User(current.user.id))
        .await
        .map_err(内部)?;
    let part = tx
        .insert(part_instance::ActiveModel {
            part_catalog_id: Set(catalog.id),
            serial_number: Set(serial),
            status: Set(status),
            health: Set(health),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .await
        .map_err(内部)?;
    tx.insert(part_instance_location::ActiveModel {
        part_instance_id: Set(part.id),
        location_type: Set(location_type),
        location_id: Set(Some(location_id)),
        // どのスロットかまでは求めない（6.2）
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(now),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(内部)?;
    tx.commit().await.map_err(内部)?;

    Ok(Redirect::to(&format!("/projects/{project_id}/parts")).into_response())
}

// ---------------------------------------------------------------------------
// 詳細・置き場所の移動
// ---------------------------------------------------------------------------

pub async fn detail(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, part_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    詳細を描く(&state, &current, project_id, part_id, None).await
}

async fn 詳細を描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    part_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let tz = state.タイムゾーン(&current.user);
    let db = &state.db;
    let (part, 現在) = 対象(state, project_id, part_id).await?;

    let (category, model) = 型を引く(db, [part.part_catalog_id])
        .await?
        .remove(&part.part_catalog_id)
        .unwrap_or_default();

    let 履歴 = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(part.id))
        .order_by_desc(part_instance_location::Column::FromDate)
        .order_by_desc(part_instance_location::Column::Id)
        .all(db)
        .await
        .map_err(内部)?;
    let 場所 = 場所の名前(db, &履歴).await?;
    let history = 履歴
        .iter()
        .map(|s| HistoryRow {
            place: 場所.表示(project_id, s, l).0,
            period: match s.to_date {
                Some(to) => format!(
                    "{} 〜 {}",
                    crate::tz::日時(s.from_date, tz),
                    crate::tz::日時(to, tz)
                ),
                None => format!("{} 〜", crate::tz::日時(s.from_date, tz)),
            },
            ticket: s.work_order_id.map(|id| Labeled {
                label: チケット番号(id),
                value: format!("/projects/{project_id}/work-orders/{id}"),
            }),
            current: s.to_date.is_none(),
        })
        .collect();

    let (place, place_href) = 場所.表示(project_id, &現在, l);
    // ケーブルは機器に載っている部品にしか挿せない。外す先はその機器の画面
    let connections_href = match (&現在.location_id, 現在.location_type.as_str()) {
        (Some(device_id), DEVICE)
            if part_location::ケーブルが挿さっている(db, part.id)
                .await
                .map_err(内部)? =>
        {
            Some(format!(
                "/projects/{project_id}/devices/{device_id}/connections"
            ))
        }
        _ => None,
    };
    let 倉庫 = crate::setting::倉庫プロジェクトか(db, project_id)
        .await
        .map_err(内部)?;

    let serial_number = part.serial_number.clone().unwrap_or_default();
    let t = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    render(&PartDetailPage {
        chrome: Chrome::project(
            db,
            &current.user,
            current.csrf_token.clone(),
            &project,
            "parts",
        )
        .await,
        project_id,
        project_name: project.name,
        part_id: part.id,
        t_parts: t("part_instances.title"),
        t_basic: t("part_instances.basic"),
        t_category: t("part_instances.category"),
        t_model: t("part_instances.model"),
        t_serial_number: t("part_instances.serial_number"),
        t_status: t("part_instances.status"),
        t_health: t("part_instances.health"),
        t_place: t("part_instances.place"),
        t_history: t("part_instances.history"),
        t_period: t("part_instances.period"),
        t_ticket: t("part_instances.ticket"),
        t_current: t("part_instances.current"),
        t_move: t("part_instances.move"),
        t_move_hint: t("part_instances.move_hint"),
        t_move_to: t("part_instances.move_to"),
        t_devices: t("part_instances.devices"),
        t_containers: t("part_instances.containers"),
        t_here: t("part_instances.here"),
        t_has_cable: t("part_instances.has_cable"),
        t_connections: t("connections.title"),
        title: if serial_number.is_empty() {
            model.clone()
        } else {
            format!("{model} [{serial_number}]")
        },
        category,
        model,
        serial_number,
        status: if 倉庫 {
            STORED.to_owned()
        } else {
            part.status.clone()
        },
        status_label: if 倉庫 {
            保管中の表示(l)
        } else {
            状態の表示(&part.status, l)
        },
        health_label: 容体の表示(&part.health, l),
        health: part.health,
        place,
        place_href,
        connections_href,
        history,
        places: if can_edit {
            置き場所を並べる(db, project_id).await?
        } else {
            置き場所の候補 {
                devices: Vec::new(),
                containers: Vec::new(),
            }
        },
        current_place: 置き場所の値(&現在),
        can_edit,
        error,
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct MoveForm {
    /// `Device:{id}` / `MountContainer:{id}` / `Project`。
    #[serde(default)]
    pub place: String,
}

/// 置き場所を移す。**閉じて開く**（不変条件1）。
pub async fn relocate(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, part_id)): Path<(i32, i32)>,
    Form(form): Form<MoveForm>,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    編集権(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let db = &state.db;
    let 誤り = |key: &str| Some(rust_i18n::t!(key, locale = l).to_string());

    // **読み取りはトランザクションを開く前に済ませる**（SQLiteで自分の書き込みロックを待つ）
    let (part, 現在) = 対象(&state, project_id, part_id).await?;
    let Some((location_type, location_id)) =
        置き場所を読む(&state, project_id, &form.place).await?
    else {
        let e = 誤り("part_instances.error_place");
        return 詳細を描く(&state, &current, project_id, part_id, e).await;
    };
    if 現在.location_type == location_type && 現在.location_id == Some(location_id) {
        let e = 誤り("part_instances.error_same_place");
        return 詳細を描く(&state, &current, project_id, part_id, e).await;
    }
    // **ケーブルが挿さったままの部品は動かさない。**自動では外さない
    if part_location::ケーブルが挿さっている(db, part.id)
        .await
        .map_err(内部)?
    {
        let e = 誤り("part_instances.error_has_cable");
        return 詳細を描く(&state, &current, project_id, part_id, e).await;
    }

    let now = Utc::now();
    let tx = AuditedTx::begin(db, Actor::User(current.user.id))
        .await
        .map_err(内部)?;
    tx.update(
        &現在,
        part_instance_location::ActiveModel {
            id: Set(現在.id),
            to_date: Set(Some(now)),
            ..Default::default()
        },
    )
    .await
    .map_err(内部)?;
    tx.insert(part_instance_location::ActiveModel {
        part_instance_id: Set(part.id),
        location_type: Set(location_type),
        location_id: Set(Some(location_id)),
        chassis_slot_id: Set(None),
        work_order_id: Set(None),
        from_date: Set(now),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(内部)?;
    tx.commit().await.map_err(内部)?;

    Ok(Redirect::to(&format!("/projects/{project_id}/parts/{part_id}")).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// このプロジェクトに今ある部品と、その現行の所在。無ければ `NotFound`。
///
/// 廃棄した部品・他のプロジェクトへ移った部品は、この画面では扱わない。
async fn 対象(
    state: &AppState,
    project_id: i32,
    part_id: i32,
) -> AppResult<(part_instance::Model, part_instance_location::Model)> {
    let db = &state.db;
    let part = part_instance::Entity::find_by_id(part_id)
        .one(db)
        .await
        .map_err(内部)?
        .ok_or(AppError::NotFound)?;
    if part_location::部品の現在のプロジェクト(db, part.id)
        .await
        .map_err(内部)?
        != Some(project_id)
    {
        return Err(AppError::NotFound);
    }
    let 現在 = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(part.id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .one(db)
        .await
        .map_err(内部)?
        .ok_or(AppError::NotFound)?;
    Ok((part, 現在))
}

/// フォームの値から置き場所を読む。**このプロジェクトの中に限る。**
///
/// 置けない値（他のプロジェクトの機器・過去にあった機器・撤去した設備・什器・
/// 語彙に無い値）は `None`。
async fn 置き場所を読む(
    state: &AppState,
    project_id: i32,
    value: &str,
) -> AppResult<Option<(String, i32)>> {
    let value = value.trim();
    if value == PROJECT {
        return Ok(Some((PROJECT.to_owned(), project_id)));
    }
    let Some((種類, id)) = value.split_once(':') else {
        return Ok(None);
    };
    let Ok(id) = id.parse::<i32>() else {
        return Ok(None);
    };
    match 種類 {
        DEVICE => Ok(super::device::今ここにあるか(state, project_id, id)
            .await?
            .then(|| (DEVICE.to_owned(), id))),
        MOUNT_CONTAINER => Ok(mount_container::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .map_err(内部)?
            .filter(|c| {
                c.location_type == PROJECT && c.location_id == project_id && c.retired_at.is_none()
            })
            .map(|c| (MOUNT_CONTAINER.to_owned(), c.id))),
        _ => Ok(None),
    }
}

/// 所在の行を、フォームの値に直す（[`置き場所を読む`] の逆）。
fn 置き場所の値(s: &part_instance_location::Model) -> String {
    match (s.location_type.as_str(), s.location_id) {
        (PROJECT, _) => PROJECT.to_owned(),
        (種類, Some(id)) => format!("{種類}:{id}"),
        (種類, None) => 種類.to_owned(),
    }
}

/// 置ける機器と設備・什器。名前の順。
async fn 置き場所を並べる<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> AppResult<置き場所の候補> {
    let 機器id: Vec<i32> = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .map(|a| a.device_id)
        .collect();
    let mut devices = Vec::new();
    for 組 in 機器id.chunks(まとめて引く件数) {
        for d in device::Entity::find()
            .filter(device::Column::Id.is_in(組.to_vec()))
            .all(db)
            .await
            .map_err(内部)?
        {
            devices.push(Labeled {
                label: d.hostname,
                value: format!("{DEVICE}:{}", d.id),
            });
        }
    }
    devices.sort_by(|a, b| a.label.cmp(&b.label));

    let containers = mount_container::Entity::find()
        .filter(mount_container::Column::LocationType.eq(PROJECT))
        .filter(mount_container::Column::LocationId.eq(project_id))
        .filter(mount_container::Column::RetiredAt.is_null())
        .order_by_asc(mount_container::Column::Name)
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .map(|c| Labeled {
            label: c.name,
            value: format!("{MOUNT_CONTAINER}:{}", c.id),
        })
        .collect();

    Ok(置き場所の候補 {
        devices,
        containers,
    })
}

/// 所在の行に出てくる機器・設備・什器の名前。
struct 場所 {
    機器: HashMap<i32, String>,
    /// 機器が所属したことのあるプロジェクト。**そこに無いプロジェクトには名前を
    /// 出さない**（機器を見せる範囲と同じ）。
    機器の所属先: HashMap<i32, HashSet<i32>>,
    設備: HashMap<i32, (String, i32)>,
}

impl 場所 {
    /// 置き場所の表示と、その画面へのリンク。
    ///
    /// **他のプロジェクトのものは名前を出さない。**部品は機器について移譲される
    /// ため、履歴には他のプロジェクトにいた頃の行が混ざる。
    fn 表示(
        &self,
        project_id: i32,
        s: &part_instance_location::Model,
        l: &str,
    ) -> (String, Option<String>) {
        let 別 = || {
            (
                rust_i18n::t!("part_instances.other_project", locale = l).to_string(),
                None,
            )
        };
        match (s.location_type.as_str(), s.location_id) {
            (DEVICE, Some(id)) => match self.機器.get(&id) {
                Some(hostname)
                    if self
                        .機器の所属先
                        .get(&id)
                        .is_some_and(|先| 先.contains(&project_id)) =>
                {
                    (
                        hostname.clone(),
                        Some(format!("/projects/{project_id}/devices/{id}")),
                    )
                }
                _ => 別(),
            },
            (MOUNT_CONTAINER, Some(id)) => match self.設備.get(&id) {
                Some((name, 持ち主)) if *持ち主 == project_id => (
                    name.clone(),
                    Some(format!("/projects/{project_id}/containers/{id}")),
                ),
                _ => 別(),
            },
            (PROJECT, Some(id)) if id == project_id => (
                rust_i18n::t!("part_instances.here", locale = l).to_string(),
                None,
            ),
            (DISPOSED, _) => (
                rust_i18n::t!("part_instances.disposed", locale = l).to_string(),
                None,
            ),
            _ => 別(),
        }
    }
}

async fn 場所の名前<C: ConnectionTrait>(
    db: &C,
    所在: &[part_instance_location::Model],
) -> AppResult<場所> {
    let ids = |種類: &str| -> Vec<i32> {
        let mut v: Vec<i32> = 所在
            .iter()
            .filter(|s| s.location_type == 種類)
            .filter_map(|s| s.location_id)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };

    let mut 機器 = HashMap::new();
    let mut 機器の所属先: HashMap<i32, HashSet<i32>> = HashMap::new();
    for 組 in ids(DEVICE).chunks(まとめて引く件数) {
        for d in device::Entity::find()
            .filter(device::Column::Id.is_in(組.to_vec()))
            .all(db)
            .await
            .map_err(内部)?
        {
            機器.insert(d.id, d.hostname);
        }
        for a in device_assignment::Entity::find()
            .filter(device_assignment::Column::DeviceId.is_in(組.to_vec()))
            .filter(device_assignment::Column::LocationType.eq(PROJECT))
            .all(db)
            .await
            .map_err(内部)?
        {
            if let Some(先) = a.location_id {
                機器の所属先.entry(a.device_id).or_default().insert(先);
            }
        }
    }

    let mut 設備 = HashMap::new();
    for 組 in ids(MOUNT_CONTAINER).chunks(まとめて引く件数) {
        for c in mount_container::Entity::find()
            .filter(mount_container::Column::Id.is_in(組.to_vec()))
            .all(db)
            .await
            .map_err(内部)?
        {
            設備.insert(c.id, (c.name, c.location_id));
        }
    }

    Ok(場所 {
        機器,
        機器の所属先,
        設備,
    })
}

/// 型の表示（分類と、ベンダー＋型番）。
async fn 型を引く<C: ConnectionTrait>(
    db: &C,
    ids: impl IntoIterator<Item = i32>,
) -> AppResult<HashMap<i32, (String, String)>> {
    let mut ids: Vec<i32> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();

    let mut 型 = Vec::new();
    for 組 in ids.chunks(まとめて引く件数) {
        型.extend(
            part_catalog::Entity::find()
                .filter(part_catalog::Column::Id.is_in(組.to_vec()))
                .all(db)
                .await
                .map_err(内部)?,
        );
    }
    let ベンダー = ベンダー名(db, 型.iter().map(|c| c.vendor_id)).await?;
    Ok(型
        .into_iter()
        .map(|c| {
            let v = ベンダー.get(&c.vendor_id).cloned().unwrap_or_default();
            (c.id, (c.category, format!("{v} {}", c.part_number)))
        })
        .collect())
}

async fn ベンダー名<C: ConnectionTrait>(
    db: &C,
    ids: impl IntoIterator<Item = i32>,
) -> AppResult<HashMap<i32, String>> {
    let mut ids: Vec<i32> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut out = HashMap::new();
    for 組 in ids.chunks(まとめて引く件数) {
        for v in vendor::Entity::find()
            .filter(vendor::Column::Id.is_in(組.to_vec()))
            .all(db)
            .await
            .map_err(内部)?
        {
            out.insert(v.id, v.name);
        }
    }
    Ok(out)
}

/// 登録で選べる型（廃番・統合済みを除く、18.5）。
async fn 選べる型<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    let 型 = part_catalog::Entity::find()
        .filter(part_catalog::Column::RetiredAt.is_null())
        .filter(part_catalog::Column::MergedIntoPartCatalogId.is_null())
        .all(db)
        .await
        .map_err(内部)?;
    let ベンダー = ベンダー名(db, 型.iter().map(|c| c.vendor_id)).await?;
    let mut out: Vec<Labeled> = 型
        .into_iter()
        .map(|c| Labeled {
            label: format!(
                "{} / {} {}",
                c.category,
                ベンダー.get(&c.vendor_id).cloned().unwrap_or_default(),
                c.part_number
            ),
            value: c.id.to_string(),
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(out)
}

/// 同じベンダーで同じシリアルの部品が、既にあるか（23.10）。
async fn 同じシリアルがある<C: ConnectionTrait>(
    db: &C,
    vendor_id: i32,
    serial: &str,
) -> AppResult<bool> {
    let 同シリアル = part_instance::Entity::find()
        .filter(part_instance::Column::SerialNumber.eq(serial))
        .all(db)
        .await
        .map_err(内部)?;
    let 型: Vec<i32> = 同シリアル.iter().map(|p| p.part_catalog_id).collect();
    if 型.is_empty() {
        return Ok(false);
    }
    Ok(part_catalog::Entity::find()
        .filter(part_catalog::Column::Id.is_in(型))
        .filter(part_catalog::Column::VendorId.eq(vendor_id))
        .one(db)
        .await
        .map_err(内部)?
        .is_some())
}

fn 語彙(value: &str, allowed: &[&'static str]) -> Option<String> {
    let value = value.trim();
    allowed
        .iter()
        .find(|v| **v == value)
        .map(|v| (*v).to_owned())
}

async fn 編集権(state: &AppState, current: &CurrentUser, project_id: i32) -> AppResult<()> {
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)
}

/// 閲覧の可否を確かめ、プロジェクトと「編集できるか」を返す。
async fn 入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<(project::Model, bool)> {
    let project = project::Entity::find_by_id(project_id)
        .one(&state.db)
        .await
        .map_err(内部)?
        .ok_or(AppError::NotFound)?;

    authorization::require_project_member(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let can_edit = authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .is_ok();

    Ok((project, can_edit))
}
