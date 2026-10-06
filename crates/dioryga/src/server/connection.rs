//! ポート接続（設計書16.1のB領域、8.4、8.7）。
//!
//! ケーブルの端を、機器に載っている部品のポートに挿す。**画面は機器の下に
//! 置く**（インターフェースと同じ）。
//!
//! # ケーブルの実物は、最初の接続と同時に作る
//!
//! 実物だけを先に登録する画面は無い。ケーブルはプロジェクトへの所属を
//! 持たず、**つながっている機器のプロジェクトから辿る。**すべての端を外した
//! ケーブルは画面から辿れなくなる（行は履歴として残る）。
//!
//! # 端ごとに挿す
//!
//! 通常のケーブルは端が2つ、ブレイクアウトは Trunk と Branch×N になるだけで、
//! 同じ仕組みで扱う。2本目以降の端は、すでに挿さっているケーブルの空いた端を
//! 選んで挿す。
//!
//! # プロジェクトをまたぐ接続
//!
//! 共用のスイッチに、別のプロジェクトの機器をつなぐ場合である。**挿す人が、
//! ケーブルの挿さっている先のすべてのプロジェクトで編集できること**を求める。
//! 相手の機器は、そのプロジェクトのメンバーにだけ見せる。メンバーでない人には
//! 「別のプロジェクトの機器」とだけ出す。外すのは、外す端の側の権限だけでよい
//! ——両方を求めると、移譲・廃棄の前にケーブルを外せない側が出る。
//!
//! # 増設チケットの予約（11.6）
//!
//! 接続するときに計画中の増設チケットを選ぶと、その接続は予約になる。**搭載の
//! 予約と同じく、Executeまで待たずに現行の行を作る**（`work_order_id` つき）。
//! 予約したポートと端は埋まるので、二重予約は使用中の検査がそのまま止める。
//! チケットが実行されるまでは「予約中」と出し、中断で閉じる。
//!
//! 予約は、機器のポート接続の画面で作る。チケットの画面に置くと、相手側
//! （スイッチ）のポートを選ぶために、プロジェクトの全機器のポートを並べることに
//! なる。
//!
//! # 検査
//!
//! ケーブルの種別（`cable_kind`）とポートの種別（`port_kind`）が違えば拒否
//! する。**コネクタ形状は拒否しない**——`IEC C13` と `IEC C14` のように対に
//! なる組は値が違う。同じ値でなければ、画面に警告を出すだけにする。

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::{
    cable_catalog, cable_connection, cable_end_slot, cable_instance, device, device_assignment,
    part_catalog, part_instance, part_instance_location, part_port_slot, project, work_order,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::catalog::Labeled;
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

const DEVICE: &str = "Device";
const PROJECT: &str = "Project";

/// `IN (...)` に渡す件数の上限。SQLite の変数の上限（32766）より十分小さくする。
const まとめて引く件数: usize = 500;

struct PortRow {
    part: String,
    port: String,
    kind: String,
    connector: String,
    /// つながっていれば、その接続
    link: Option<LinkRow>,
}

struct LinkRow {
    connection_id: i32,
    cable: String,
    end: String,
    /// 反対側の端。ブレイクアウトでは複数ある
    peers: Vec<String>,
    /// コネクタ形状が同じ値でないときの警告
    warning: Option<String>,
    /// 増設チケットの予約（11.6）。実行されるまでの間だけ入る
    reserved: Option<TicketLink>,
}

/// 予約しているチケットへの導線。
struct TicketLink {
    id: i32,
    title: String,
}

#[derive(askama::Template)]
#[template(path = "connections.html")]
struct ConnectionsPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    device_id: i32,
    hostname: String,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_part: String,
    t_port: String,
    t_kind: String,
    t_connector: String,
    t_cable: String,
    t_end: String,
    t_peer: String,
    t_none: String,
    t_no_ports: String,
    t_disconnect: String,
    t_connect: String,
    t_connect_hint: String,
    t_new_cable: String,
    t_open_end: String,
    t_serial_number: String,
    t_asset_number: String,
    t_new_only_hint: String,
    t_no_free_port: String,
    t_reserved: String,
    t_ticket: String,
    t_ticket_none: String,
    t_ticket_hint: String,
    /// 予約に使える増設チケット（計画中のもの）
    tickets: Vec<Labeled>,
    rows: Vec<PortRow>,
    free_ports: Vec<Labeled>,
    new_cables: Vec<Labeled>,
    open_ends: Vec<Labeled>,
    can_edit: bool,
    error: Option<String>,
}

pub async fn show(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    描く(&state, &current, project_id, device_id, None).await
}

async fn 描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    device_id: i32,
    error: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let d = 見える機器(state, project_id, device_id).await?;
    // 過去にあった機器は見せるだけで、書き換える導線を出さない
    let can_edit = can_edit && super::device::今ここにあるか(state, project_id, device_id).await?;
    let db = &state.db;

    let ポート = 載っているポート(db, device_id).await?;
    let 別のプロジェクト = rust_i18n::t!("connections.other_project", locale = l).to_string();
    let (見える, 編集できる) = 利用者のプロジェクト(db, &current.user).await?;
    let 視野 = 視野 {
        project_id,
        見える: &見える,
        別のプロジェクト: &別のプロジェクト,
    };
    let 未接続 = rust_i18n::t!("connections.unconnected", locale = l).to_string();

    let mut rows = Vec::new();
    let mut free_ports = Vec::new();
    for p in &ポート {
        let link = match 現行の接続(db, p.part_instance_id, p.slot.id).await? {
            Some(c) => {
                let ケーブル = ケーブルを引く(db, c.cable_instance_id).await?;
                let 端 = ケーブル.端(c.cable_end_slot_id);
                let mut peers = Vec::new();
                for e in &ケーブル.ends {
                    if e.id == c.cable_end_slot_id {
                        continue;
                    }
                    let 相手 = match ケーブル.接続.get(&e.id) {
                        Some(other) => 予約つきの接続先(db, &視野, other, l).await?,
                        None => 未接続.clone(),
                    };
                    peers.push(format!("{}: {相手}", e.end_label));
                }
                let warning = 端
                    .filter(|e| {
                        !e.connector_type
                            .eq_ignore_ascii_case(&p.slot.connector_type)
                    })
                    .map(|e| {
                        rust_i18n::t!(
                            "connections.connector_differs",
                            locale = l,
                            cable = e.connector_type,
                            port = p.slot.connector_type
                        )
                        .to_string()
                    });
                Some(LinkRow {
                    connection_id: c.id,
                    cable: ケーブル.名前(),
                    end: 端.map(|e| e.end_label.clone()).unwrap_or_default(),
                    peers,
                    warning,
                    reserved: 予約中のチケット(db, &c).await?.map(|w| TicketLink {
                        id: w.id,
                        title: w.title,
                    }),
                })
            }
            None => {
                free_ports.push(Labeled {
                    label: format!(
                        "{} {}（{} / {}）",
                        p.part_number, p.slot.port_label, p.slot.port_kind, p.slot.connector_type
                    ),
                    value: format!("{}:{}", p.part_instance_id, p.slot.id),
                });
                None
            }
        };
        rows.push(PortRow {
            part: p.part_number.clone(),
            port: p.slot.port_label.clone(),
            kind: p.slot.port_kind.clone(),
            connector: p.slot.connector_type.clone(),
            link,
        });
    }

    let (new_cables, open_ends) = if can_edit {
        (
            新しいケーブルの候補(db).await?,
            空いた端の候補(db, &視野, &編集できる).await?,
        )
    } else {
        (Vec::new(), Vec::new())
    };

    render(&ConnectionsPage {
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
        device_id,
        hostname: d.hostname,
        t_title: rust_i18n::t!("connections.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("connections.lead", locale = l).to_string(),
        t_back: rust_i18n::t!("devices.back", locale = l).to_string(),
        t_part: rust_i18n::t!("connections.part", locale = l).to_string(),
        t_port: rust_i18n::t!("connections.port", locale = l).to_string(),
        t_kind: rust_i18n::t!("connections.kind", locale = l).to_string(),
        t_connector: rust_i18n::t!("connections.connector", locale = l).to_string(),
        t_cable: rust_i18n::t!("connections.cable", locale = l).to_string(),
        t_end: rust_i18n::t!("connections.end", locale = l).to_string(),
        t_peer: rust_i18n::t!("connections.peer", locale = l).to_string(),
        t_none: rust_i18n::t!("connections.none", locale = l).to_string(),
        t_no_ports: rust_i18n::t!("connections.no_ports", locale = l).to_string(),
        t_disconnect: rust_i18n::t!("connections.disconnect", locale = l).to_string(),
        t_connect: rust_i18n::t!("connections.connect", locale = l).to_string(),
        t_connect_hint: rust_i18n::t!("connections.connect_hint", locale = l).to_string(),
        t_new_cable: rust_i18n::t!("connections.new_cable", locale = l).to_string(),
        t_open_end: rust_i18n::t!("connections.open_end", locale = l).to_string(),
        t_serial_number: rust_i18n::t!("connections.serial_number", locale = l).to_string(),
        t_asset_number: rust_i18n::t!("connections.asset_number", locale = l).to_string(),
        t_new_only_hint: rust_i18n::t!("connections.new_only_hint", locale = l).to_string(),
        t_no_free_port: rust_i18n::t!("connections.no_free_port", locale = l).to_string(),
        t_reserved: rust_i18n::t!("connections.reserved", locale = l).to_string(),
        t_ticket: rust_i18n::t!("connections.ticket", locale = l).to_string(),
        t_ticket_none: rust_i18n::t!("connections.ticket_none", locale = l).to_string(),
        t_ticket_hint: rust_i18n::t!("connections.ticket_hint", locale = l).to_string(),
        tickets: if can_edit {
            予約できるチケット(db, project_id).await?
        } else {
            Vec::new()
        },
        rows,
        free_ports,
        new_cables,
        open_ends,
        can_edit,
        error,
    })
}

// ---------------------------------------------------------------------------
// 接続する
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ConnectForm {
    /// `part_instance_id:port_slot_id`
    #[serde(default)]
    pub port: String,
    /// `new:{cable_end_slot_id}` または `open:{cable_instance_id}:{cable_end_slot_id}`
    #[serde(default)]
    pub cable: String,
    #[serde(default)]
    pub serial_number: String,
    #[serde(default)]
    pub asset_number: String,
    /// 予約として登録するときの、増設チケット（11.6）。空なら直接の登録
    #[serde(default)]
    pub work_order_id: String,
}

/// どのケーブルのどの端を挿すか。
enum 挿す端 {
    /// 新しい実物を作って挿す。型は端から決まる
    新規(cable_end_slot::Model),
    /// すでに挿さっているケーブルの、空いた端を挿す
    既存(cable_instance::Model, cable_end_slot::Model),
}

pub async fn connect(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<ConnectForm>,
) -> AppResult<Response> {
    let l = 書き換える機器(&state, &current, project_id, device_id).await?;
    let db = &state.db;
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));

    let (_, 編集できる) = 利用者のプロジェクト(db, &current.user).await?;
    let 入力 = match 検証(db, &編集できる, device_id, &form).await? {
        Ok(v) => v,
        Err(key) => {
            let message = rust_i18n::t!(key, locale = l).to_string();
            return 描く(&state, &current, project_id, device_id, Some(message)).await;
        }
    };
    let (part_instance_id, port_slot_id, 端) = 入力;

    // **予約は計画中の増設にしか作れない**（11.6）。承認後に配線が変わっては、
    // 承認した内容と実施する内容がずれる。候補に無い値を送られても通さない
    let work_order_id = match form.work_order_id.trim() {
        "" => None,
        id => {
            let 候補 = 予約できるチケット(db, project_id).await?;
            if !候補.iter().any(|t| t.value == id) {
                let message = rust_i18n::t!("connections.error_ticket", locale = l).to_string();
                return 描く(&state, &current, project_id, device_id, Some(message)).await;
            }
            id.parse::<i32>().ok()
        }
    };

    let now = Utc::now();
    let tx = AuditedTx::begin(db, Actor::User(current.user.id))
        .await
        .map_err(内部)?;
    let (cable_instance_id, cable_end_slot_id) = match 端 {
        挿す端::新規(end) => {
            let 空なら無し = |s: &str| {
                let s = s.trim();
                (!s.is_empty()).then(|| s.to_owned())
            };
            let made = tx
                .insert(cable_instance::ActiveModel {
                    cable_catalog_id: Set(end.cable_catalog_id),
                    serial_number: Set(空なら無し(&form.serial_number)),
                    asset_number: Set(空なら無し(&form.asset_number)),
                    retired_at: Set(None),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                .await
                .map_err(内部)?;
            (made.id, end.id)
        }
        挿す端::既存(cable, end) => (cable.id, end.id),
    };
    // チケットを選ばなければ、変更管理チケットを持たない接続になる（搭載やIP
    // アドレスを直接登録するのと同じ扱い）。選べば予約になる（11.6）
    tx.insert(cable_connection::ActiveModel {
        cable_instance_id: Set(cable_instance_id),
        cable_end_slot_id: Set(cable_end_slot_id),
        part_instance_id: Set(part_instance_id),
        port_slot_id: Set(port_slot_id),
        work_order_id: Set(work_order_id),
        from_date: Set(now),
        to_date: Set(None),
        ..Default::default()
    })
    .await
    .map_err(内部)?;
    tx.commit().await.map_err(内部)?;

    Ok(Redirect::to(&戻り先(project_id, device_id)).into_response())
}

/// 入力を確かめる。誤りは i18n のキーで返す。
async fn 検証(
    db: &sea_orm::DatabaseConnection,
    編集できる: &HashSet<i32>,
    device_id: i32,
    form: &ConnectForm,
) -> AppResult<Result<(i32, i32, 挿す端), &'static str>> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));

    // --- ポート。この機器に今載っている部品の、空いたポートに限る
    let Some((part_instance_id, port_slot_id)) = 二つ組(&form.port) else {
        return Ok(Err("connections.error_port"));
    };
    let ポート = 載っているポート(db, device_id).await?;
    let Some(p) = ポート
        .iter()
        .find(|p| p.part_instance_id == part_instance_id && p.slot.id == port_slot_id)
    else {
        return Ok(Err("connections.error_port"));
    };
    if 現行の接続(db, part_instance_id, port_slot_id)
        .await?
        .is_some()
    {
        return Ok(Err("connections.error_port_in_use"));
    }

    // --- ケーブルの端
    let 部分: Vec<&str> = form.cable.split(':').collect();
    let 端 = match 部分.as_slice() {
        ["new", end_id] => {
            let Some(end) = 端を引く(db, end_id).await? else {
                return Ok(Err("connections.error_cable"));
            };
            挿す端::新規(end)
        }
        ["open", cable_id, end_id] => {
            let Ok(cable_id) = cable_id.parse::<i32>() else {
                return Ok(Err("connections.error_cable"));
            };
            let Some(end) = 端を引く(db, end_id).await? else {
                return Ok(Err("connections.error_cable"));
            };
            let Some(cable) = cable_instance::Entity::find_by_id(cable_id)
                .one(db)
                .await
                .map_err(内部)?
                .filter(|c| c.cable_catalog_id == end.cable_catalog_id && c.retired_at.is_none())
            else {
                return Ok(Err("connections.error_cable"));
            };
            // **挿さっている先のすべてのプロジェクトで、編集できることを求める。**
            // 共用のスイッチに別のプロジェクトの機器をつなぐには、両方のプロジェクトで
            // Operator 以上が要る。IDを直接送られても、権限の無いプロジェクトの
            // ケーブルは挿せない。どこにも挿さっていないケーブルも挿せない
            let 現行 = ケーブルの現行の接続(db, cable.id).await?;
            if 現行.iter().any(|c| c.cable_end_slot_id == end.id) {
                return Ok(Err("connections.error_end_in_use"));
            }
            if 現行.is_empty() || !すべて編集できる(db, 現行.iter(), 編集できる).await?
            {
                return Ok(Err("connections.error_cable"));
            }
            挿す端::既存(cable, end)
        }
        _ => return Ok(Err("connections.error_cable")),
    };

    // --- 種別。ケーブルとポートで違えば拒否する（コネクタ形状は見ない）
    let end = match &端 {
        挿す端::新規(end) | 挿す端::既存(_, end) => end,
    };
    let Some(catalog) = cable_catalog::Entity::find_by_id(end.cable_catalog_id)
        .one(db)
        .await
        .map_err(内部)?
    else {
        return Ok(Err("connections.error_cable"));
    };
    // 廃番の型で、新しい実物は作れない。すでにある実物の端は挿せる
    if matches!(端, 挿す端::新規(_)) && catalog.retired_at.is_some() {
        return Ok(Err("connections.error_cable"));
    }
    if catalog.cable_kind.as_deref() != Some(p.slot.port_kind.as_str()) {
        return Ok(Err("connections.error_kind"));
    }

    Ok(Ok((part_instance_id, port_slot_id, 端)))
}

// ---------------------------------------------------------------------------
// 外す
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DisconnectForm {
    pub connection_id: i32,
}

/// 端を外す。**現行行を閉じるだけで、消さない**（不変条件1）。
pub async fn disconnect(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    Form(form): Form<DisconnectForm>,
) -> AppResult<Response> {
    書き換える機器(&state, &current, project_id, device_id).await?;
    let db = &state.db;
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));

    // **この機器のポートに挿さっている端に限る。**IDだけを信じると、別の機器の
    // 接続を外せてしまう
    let ポート = 載っているポート(db, device_id).await?;
    let row = cable_connection::Entity::find_by_id(form.connection_id)
        .one(db)
        .await
        .map_err(内部)?
        .filter(|c| c.to_date.is_none())
        .filter(|c| {
            ポート
                .iter()
                .any(|p| p.part_instance_id == c.part_instance_id && p.slot.id == c.port_slot_id)
        })
        .ok_or(AppError::NotFound)?;

    let tx = AuditedTx::begin(db, Actor::User(current.user.id))
        .await
        .map_err(内部)?;
    tx.update(
        &row,
        cable_connection::ActiveModel {
            id: Set(row.id),
            to_date: Set(Some(Utc::now())),
            ..Default::default()
        },
    )
    .await
    .map_err(内部)?;
    tx.commit().await.map_err(内部)?;

    Ok(Redirect::to(&戻り先(project_id, device_id)).into_response())
}

// ---------------------------------------------------------------------------
// 他の画面から使う判定
// ---------------------------------------------------------------------------

/// 機器に載っている部品に、ケーブルが挿さっているか（移譲・廃棄の判定、#225）。
pub async fn 機器にケーブルがある<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<bool> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let 部品: Vec<i32> = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(DEVICE))
        .filter(part_instance_location::Column::LocationId.eq(device_id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .map(|l| l.part_instance_id)
        .collect();
    for 組 in 部品.chunks(まとめて引く件数) {
        let ある = cable_connection::Entity::find()
            .filter(cable_connection::Column::PartInstanceId.is_in(組.to_vec()))
            .filter(cable_connection::Column::ToDate.is_null())
            .one(db)
            .await
            .map_err(内部)?
            .is_some();
        if ある {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 部品に、ケーブルが挿さっているか（部品の廃棄・置き場所の移動の判定、#225、#229）。
pub async fn 部品にケーブルがある<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
) -> AppResult<bool> {
    crate::part_location::ケーブルが挿さっている(db, part_instance_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// ポートにつながっている相手（インターフェース一覧の「接続先」）。
///
/// つながっていなければ空。相手の端をまだ挿していなければ、ケーブルの名前だけを
/// 出す。見せる範囲は、ポート接続の画面と同じである。
pub async fn ポートの接続先<C: ConnectionTrait>(
    db: &C,
    user: &entity::app_user::Model,
    project_id: i32,
    part_instance_id: i32,
    port_slot_id: i32,
    l: &str,
) -> AppResult<String> {
    let Some(c) = 現行の接続(db, part_instance_id, port_slot_id).await? else {
        return Ok(String::new());
    };
    let 別のプロジェクト = rust_i18n::t!("connections.other_project", locale = l).to_string();
    let (見える, _) = 利用者のプロジェクト(db, user).await?;
    let 視野 = 視野 {
        project_id,
        見える: &見える,
        別のプロジェクト: &別のプロジェクト,
    };
    let k = ケーブルを引く(db, c.cable_instance_id).await?;
    let mut 相手 = Vec::new();
    for e in &k.ends {
        if e.id == c.cable_end_slot_id {
            continue;
        }
        if let Some(other) = k.接続.get(&e.id) {
            相手.push(予約つきの接続先(db, &視野, other, l).await?);
        }
    }
    if 相手.is_empty() {
        let 未接続 = rust_i18n::t!("connections.unconnected", locale = l);
        return Ok(format!("{}（{未接続}）", k.名前()));
    }
    Ok(相手.join(" / "))
}

/// 給電経路の本数（設計書12.6）。**機器に載っている部品の電源ポートのうち、
/// ケーブルが挿さっている数**と、電源ポートの総数を返す。電源ポートが無ければ
/// `None`。
pub async fn 給電経路<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Option<(usize, usize)>> {
    let mut 総数 = 0;
    let mut 接続 = 0;
    for p in 載っているポート(db, device_id).await? {
        if p.slot.port_kind != "Power" {
            continue;
        }
        総数 += 1;
        // 予約はまだ挿していないので数えない（11.6）
        if let Some(c) = 現行の接続(db, p.part_instance_id, p.slot.id).await? {
            if 予約中のチケット(db, &c).await?.is_none() {
                接続 += 1;
            }
        }
    }
    Ok((総数 > 0).then_some((接続, 総数)))
}

// ---------------------------------------------------------------------------
// 増設チケットの予約（設計書11.6）
// ---------------------------------------------------------------------------

/// 接続が予約なら、そのチケットを返す。
///
/// **予約なのは、チケットが実行されるまで**（`planned` / `approved`）。実行で
/// 予約は実際の接続になる。行は書き換えない——搭載の予約が、機器の `status` で
/// 予約中かどうかを見分けるのと同じく、ここではチケットの状態で見分ける。
async fn 予約中のチケット<C: ConnectionTrait>(
    db: &C,
    c: &cable_connection::Model,
) -> AppResult<Option<work_order::Model>> {
    let Some(id) = c.work_order_id else {
        return Ok(None);
    };
    Ok(work_order::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .filter(|w| ["planned", "approved"].contains(&w.status.as_str())))
}

/// 接続先の表示に、予約なら「予約中」を添える。
async fn 予約つきの接続先<C: ConnectionTrait>(
    db: &C,
    視野: &視野<'_>,
    c: &cable_connection::Model,
    l: &str,
) -> AppResult<String> {
    let mut s = 接続先の表示(db, 視野, c).await?;
    if 予約中のチケット(db, c).await?.is_some() {
        s.push_str(&format!(
            "（{}）",
            rust_i18n::t!("connections.reserved", locale = l)
        ));
    }
    Ok(s)
}

/// 予約に使えるチケット。**このプロジェクトの、計画中の増設だけ**（11.6）。
async fn 予約できるチケット<C: ConnectionTrait>(
    db: &C,
    project_id: i32,
) -> AppResult<Vec<Labeled>> {
    Ok(work_order::Entity::find()
        .filter(work_order::Column::ProjectId.eq(project_id))
        .filter(work_order::Column::WorkType.eq("Addition"))
        .filter(work_order::Column::Status.eq("planned"))
        .order_by_asc(work_order::Column::Id)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .into_iter()
        .map(|w| Labeled {
            label: w.title,
            value: w.id.to_string(),
        })
        .collect())
}

/// チケットが予約した接続の1行（チケットの詳細に出す）。
pub struct 予約した接続 {
    /// 機器・部品・ポート
    pub port: String,
    pub cable: String,
    pub end: String,
    /// 中断などで閉じた。**行は消さず閉じる**ため、閉じたものも見える
    pub released: bool,
}

/// チケットが予約した接続。**閉じたものも出す**（搭載の予約と同じ）。
pub async fn チケットの接続<C: ConnectionTrait>(
    db: &C,
    user: &entity::app_user::Model,
    project_id: i32,
    work_order_id: i32,
    l: &str,
) -> AppResult<Vec<予約した接続>> {
    let rows = cable_connection::Entity::find()
        .filter(cable_connection::Column::WorkOrderId.eq(work_order_id))
        .order_by_asc(cable_connection::Column::Id)
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let 別のプロジェクト = rust_i18n::t!("connections.other_project", locale = l).to_string();
    let (見える, _) = 利用者のプロジェクト(db, user).await?;
    let 視野 = 視野 {
        project_id,
        見える: &見える,
        別のプロジェクト: &別のプロジェクト,
    };
    let mut out = Vec::new();
    for c in rows {
        let k = ケーブルを引く(db, c.cable_instance_id).await?;
        out.push(予約した接続 {
            port: 接続先の表示(db, &視野, &c).await?,
            cable: k.名前(),
            end: k
                .端(c.cable_end_slot_id)
                .map(|e| e.end_label.clone())
                .unwrap_or_default(),
            released: c.to_date.is_some(),
        });
    }
    Ok(out)
}

/// 中断時に、チケットが予約した接続を解放する（11.6）。**行は消さず閉じる。**
pub async fn 予約を解放する(
    tx: &AuditedTx,
    work_order_id: i32,
    at: chrono::DateTime<Utc>,
) -> AppResult<()> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let 予約 = cable_connection::Entity::find()
        .filter(cable_connection::Column::WorkOrderId.eq(work_order_id))
        .filter(cable_connection::Column::ToDate.is_null())
        .all(tx.reader())
        .await
        .map_err(内部)?;
    for row in 予約 {
        tx.update(
            &row,
            cable_connection::ActiveModel {
                id: Set(row.id),
                to_date: Set(Some(at)),
                ..Default::default()
            },
        )
        .await
        .map_err(内部)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn 戻り先(project_id: i32, device_id: i32) -> String {
    format!("/projects/{project_id}/devices/{device_id}/connections")
}

/// `a:b` を2つの整数に分ける。
fn 二つ組(value: &str) -> Option<(i32, i32)> {
    let (a, b) = value.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// 機器に今載っている部品の、ポート1つ。
struct 載っているポート1 {
    part_instance_id: i32,
    /// 型番。同じ型番が複数載っていれば、シリアル番号（無ければ番号）つき
    part_number: String,
    slot: part_port_slot::Model,
}

/// 機器に今載っている部品のポート。種別（Network / Power / Stack）は問わない。
async fn 載っているポート<C: ConnectionTrait>(
    db: &C,
    device_id: i32,
) -> AppResult<Vec<載っているポート1>> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let locations = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::LocationType.eq(DEVICE))
        .filter(part_instance_location::Column::LocationId.eq(device_id))
        .filter(part_instance_location::Column::ToDate.is_null())
        .order_by_asc(part_instance_location::Column::Id)
        .all(db)
        .await
        .map_err(内部)?;

    // 先に部品と型番を集める。**同じ型番が複数載っていれば、シリアル番号を添えて
    // 見分ける**（無ければ載せた順の番号）
    let mut 部品 = Vec::new();
    for loc in locations {
        let Some(instance) = part_instance::Entity::find_by_id(loc.part_instance_id)
            .one(db)
            .await
            .map_err(内部)?
        else {
            continue;
        };
        let part_number = part_catalog::Entity::find_by_id(instance.part_catalog_id)
            .one(db)
            .await
            .map_err(内部)?
            .map(|c| c.part_number)
            .unwrap_or_default();
        部品.push((instance, part_number));
    }
    let mut 数: HashMap<String, usize> = HashMap::new();
    for (_, n) in &部品 {
        *数.entry(n.clone()).or_default() += 1;
    }
    let mut 順: HashMap<String, usize> = HashMap::new();

    let mut out = Vec::new();
    for (instance, 型番) in 部品 {
        let 何番目 = 順.entry(型番.clone()).or_default();
        *何番目 += 1;
        let part_number = if 数[&型番] > 1 {
            match instance.serial_number.as_deref().filter(|s| !s.is_empty()) {
                Some(serial) => format!("{型番} [{serial}]"),
                None => format!("{型番} #{何番目}"),
            }
        } else {
            型番
        };
        let slots = part_port_slot::Entity::find()
            .filter(part_port_slot::Column::PartCatalogId.eq(instance.part_catalog_id))
            .order_by_asc(part_port_slot::Column::Id)
            .all(db)
            .await
            .map_err(内部)?;
        for slot in slots {
            out.push(載っているポート1 {
                part_instance_id: instance.id,
                part_number: part_number.clone(),
                slot,
            });
        }
    }
    Ok(out)
}

async fn 現行の接続<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
    port_slot_id: i32,
) -> AppResult<Option<cable_connection::Model>> {
    cable_connection::Entity::find()
        .filter(cable_connection::Column::PartInstanceId.eq(part_instance_id))
        .filter(cable_connection::Column::PortSlotId.eq(port_slot_id))
        .filter(cable_connection::Column::ToDate.is_null())
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn ケーブルの現行の接続<C: ConnectionTrait>(
    db: &C,
    cable_instance_id: i32,
) -> AppResult<Vec<cable_connection::Model>> {
    cable_connection::Entity::find()
        .filter(cable_connection::Column::CableInstanceId.eq(cable_instance_id))
        .filter(cable_connection::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn 端を引く<C: ConnectionTrait>(
    db: &C,
    id: &str,
) -> AppResult<Option<cable_end_slot::Model>> {
    let Ok(id) = id.parse::<i32>() else {
        return Ok(None);
    };
    cable_end_slot::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// ケーブルの実物と、その型・端・現行の接続。
struct ケーブル {
    instance: cable_instance::Model,
    catalog: Option<cable_catalog::Model>,
    ends: Vec<cable_end_slot::Model>,
    /// 端（`cable_end_slot_id`）ごとの現行の接続
    接続: HashMap<i32, cable_connection::Model>,
}

impl ケーブル {
    fn 端(&self, end_slot_id: i32) -> Option<&cable_end_slot::Model> {
        self.ends.iter().find(|e| e.id == end_slot_id)
    }

    /// 一覧に出す名前。型（種類・長さ）と、あればシリアル番号。
    fn 名前(&self) -> String {
        let mut s = self.catalog.as_ref().map(型の名前).unwrap_or_default();
        if let Some(serial) = self.instance.serial_number.as_deref() {
            s.push_str(&format!(" [{serial}]"));
        }
        s
    }
}

fn 型の名前(c: &cable_catalog::Model) -> String {
    let mut s = c.cable_type.clone();
    if let Some(mm) = c.length_mm {
        s.push_str(&format!(" {}m", f64::from(mm) / 1000.0));
    }
    if let Some(n) = c.part_number.as_deref().filter(|n| !n.is_empty()) {
        s.push_str(&format!(" {n}"));
    }
    s
}

async fn ケーブルを引く<C: ConnectionTrait>(
    db: &C,
    cable_instance_id: i32,
) -> AppResult<ケーブル> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let instance = cable_instance::Entity::find_by_id(cable_instance_id)
        .one(db)
        .await
        .map_err(内部)?
        .ok_or(AppError::NotFound)?;
    let catalog = cable_catalog::Entity::find_by_id(instance.cable_catalog_id)
        .one(db)
        .await
        .map_err(内部)?;
    let ends = cable_end_slot::Entity::find()
        .filter(cable_end_slot::Column::CableCatalogId.eq(instance.cable_catalog_id))
        .order_by_asc(cable_end_slot::Column::Id)
        .all(db)
        .await
        .map_err(内部)?;
    let 接続 = ケーブルの現行の接続(db, cable_instance_id)
        .await?
        .into_iter()
        .map(|c| (c.cable_end_slot_id, c))
        .collect();
    Ok(ケーブル {
        instance,
        catalog,
        ends,
        接続,
    })
}

/// 部品が今載っている機器。機器の上になければ `None`。
async fn 部品の載せ先<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
) -> AppResult<Option<device::Model>> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let Some(device_id) = part_instance_location::Entity::find()
        .filter(part_instance_location::Column::PartInstanceId.eq(part_instance_id))
        .filter(part_instance_location::Column::LocationType.eq(DEVICE))
        .filter(part_instance_location::Column::ToDate.is_null())
        .one(db)
        .await
        .map_err(内部)?
        .and_then(|l| l.location_id)
    else {
        return Ok(None);
    };
    device::Entity::find_by_id(device_id)
        .one(db)
        .await
        .map_err(内部)
}

/// 部品が今あるプロジェクト（置き場所からたどる）。
async fn 部品のある場所<C: ConnectionTrait>(
    db: &C,
    part_instance_id: i32,
) -> AppResult<Option<i32>> {
    crate::part_location::部品の現在のプロジェクト(db, part_instance_id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

/// 利用者から見える範囲。接続先をどこまで出すかを決める。
struct 視野<'a> {
    /// 今開いているプロジェクト
    project_id: i32,
    /// 利用者がメンバーであるプロジェクト。**ここに無いプロジェクトの機器は、
    /// 機器名もポートも出さない**（A-6の但し書き）
    見える: &'a HashSet<i32>,
    /// 見えない機器の代わりに出す文字
    別のプロジェクト: &'a str,
}

/// 利用者がメンバーであるプロジェクトと、そのうち編集できるプロジェクト。
async fn 利用者のプロジェクト<C: ConnectionTrait>(
    db: &C,
    user: &entity::app_user::Model,
) -> AppResult<(HashSet<i32>, HashSet<i32>)> {
    let rows = entity::project_member::Entity::find()
        .filter(entity::project_member::Column::UserId.eq(user.id))
        .all(db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mut 見える = HashSet::new();
    let mut 編集できる = HashSet::new();
    for m in rows {
        見える.insert(m.project_id);
        if [authorization::ADMINISTRATOR, authorization::OPERATOR].contains(&m.role.as_str()) {
            編集できる.insert(m.project_id);
        }
    }
    Ok((見える, 編集できる))
}

/// 接続の反対側を、画面に出す文字にする。
///
/// 別のプロジェクトの機器は、プロジェクト名を添える。**利用者がメンバーでない
/// プロジェクトの機器は、機器名もポートも出さない**（A-6の但し書き：無関係な
/// 他プロジェクトのデータは見えない）。
async fn 接続先の表示<C: ConnectionTrait>(
    db: &C,
    視野: &視野<'_>,
    c: &cable_connection::Model,
) -> AppResult<String> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let 場所 = 部品のある場所(db, c.part_instance_id).await?;
    let Some(場所) = 場所.filter(|p| 視野.見える.contains(p)) else {
        return Ok(視野.別のプロジェクト.to_owned());
    };
    let 機器 = 部品の載せ先(db, c.part_instance_id).await?;
    // 型番は、相手の機器の画面と同じ見分け（シリアル番号）で出す
    let (型番, ポート) = match &機器 {
        Some(d) => 載っているポート(db, d.id)
            .await?
            .into_iter()
            .find(|p| p.part_instance_id == c.part_instance_id && p.slot.id == c.port_slot_id)
            .map(|p| (p.part_number, p.slot.port_label))
            .unwrap_or_default(),
        None => Default::default(),
    };
    let 機器名 = 機器.map(|d| d.hostname).unwrap_or_default();
    if 場所 == 視野.project_id {
        return Ok(format!("{機器名} {型番} {ポート}"));
    }
    let プロジェクト名 = project::Entity::find_by_id(場所)
        .one(db)
        .await
        .map_err(内部)?
        .map(|p| p.name)
        .unwrap_or_default();
    Ok(format!("{機器名} {型番} {ポート}（{プロジェクト名}）"))
}

/// 新しく作るケーブルの候補。**現役の型の、端ごとに1つ。**
///
/// 値は `new:{cable_end_slot_id}`。型は端から決まるので、型と端を1つの
/// 選択肢で選べる（素のHTMLフォームだけで動かすため）。
async fn 新しいケーブルの候補<C: ConnectionTrait>(db: &C) -> AppResult<Vec<Labeled>> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let catalogs = cable_catalog::Entity::find()
        .filter(cable_catalog::Column::RetiredAt.is_null())
        .order_by_asc(cable_catalog::Column::CableKind)
        .order_by_asc(cable_catalog::Column::CableType)
        .order_by_asc(cable_catalog::Column::Id)
        .all(db)
        .await
        .map_err(内部)?;
    let mut out = Vec::new();
    for c in catalogs {
        let ends = cable_end_slot::Entity::find()
            .filter(cable_end_slot::Column::CableCatalogId.eq(c.id))
            .order_by_asc(cable_end_slot::Column::Id)
            .all(db)
            .await
            .map_err(内部)?;
        for e in ends {
            out.push(Labeled {
                label: format!(
                    "{} {} — {}（{}）",
                    c.cable_kind.as_deref().unwrap_or_default(),
                    型の名前(&c),
                    e.end_label,
                    e.connector_type
                ),
                value: format!("new:{}", e.id),
            });
        }
    }
    Ok(out)
}

/// 利用者が編集できるプロジェクトの機器に挿さっているケーブルの、空いた端。
///
/// **別のプロジェクトの機器に挿さっているケーブルも候補に出す**——共用の
/// スイッチに、別のプロジェクトの機器をつなぐ場合である。ただし、そのケーブルが
/// 挿さっているすべてのプロジェクトで、利用者が編集できることを求める。
///
/// 値は `open:{cable_instance_id}:{cable_end_slot_id}`。
async fn 空いた端の候補<C: ConnectionTrait>(
    db: &C,
    視野: &視野<'_>,
    編集できる: &HashSet<i32>,
) -> AppResult<Vec<Labeled>> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));

    // 編集できるプロジェクトに今ある機器 → 載っている部品 → 挿さっているケーブル
    let mut プロジェクト: Vec<i32> = 編集できる.iter().copied().collect();
    プロジェクト.sort_unstable();
    let 機器: Vec<i32> = device_assignment::Entity::find()
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.is_in(プロジェクト))
        .filter(device_assignment::Column::ToDate.is_null())
        .all(db)
        .await
        .map_err(内部)?
        .into_iter()
        .map(|a| a.device_id)
        .collect();
    let mut 部品 = Vec::new();
    for 組 in 機器.chunks(まとめて引く件数) {
        部品.extend(
            part_instance_location::Entity::find()
                .filter(part_instance_location::Column::LocationType.eq(DEVICE))
                .filter(part_instance_location::Column::LocationId.is_in(組.to_vec()))
                .filter(part_instance_location::Column::ToDate.is_null())
                .all(db)
                .await
                .map_err(内部)?
                .into_iter()
                .map(|l| l.part_instance_id),
        );
    }
    let mut ケーブルid = Vec::new();
    let mut 見た = HashSet::new();
    for 組 in 部品.chunks(まとめて引く件数) {
        for c in cable_connection::Entity::find()
            .filter(cable_connection::Column::PartInstanceId.is_in(組.to_vec()))
            .filter(cable_connection::Column::ToDate.is_null())
            .order_by_asc(cable_connection::Column::Id)
            .all(db)
            .await
            .map_err(内部)?
        {
            if 見た.insert(c.cable_instance_id) {
                ケーブルid.push(c.cable_instance_id);
            }
        }
    }

    let mut out = Vec::new();
    for id in ケーブルid {
        let k = ケーブルを引く(db, id).await?;
        if k.instance.retired_at.is_some() || k.接続.len() >= k.ends.len() {
            continue;
        }
        if !すべて編集できる(db, k.接続.values(), 編集できる).await? {
            continue;
        }
        // どのケーブルかを、挿さっている側の機器とポートで示す
        let mut 挿さっている = Vec::new();
        for c in k.接続.values() {
            挿さっている.push(接続先の表示(db, 視野, c).await?);
        }
        挿さっている.sort();
        for e in &k.ends {
            if k.接続.contains_key(&e.id) {
                continue;
            }
            out.push(Labeled {
                label: format!(
                    "{} — {}（{}）← {}",
                    k.名前(),
                    e.end_label,
                    e.connector_type,
                    挿さっている.join(" / ")
                ),
                value: format!("open:{}:{}", k.instance.id, e.id),
            });
        }
    }
    Ok(out)
}

/// ケーブルが挿さっている先が、すべて利用者の編集できるプロジェクトにあるか。
async fn すべて編集できる<'a, C: ConnectionTrait>(
    db: &C,
    接続: impl Iterator<Item = &'a cable_connection::Model>,
    編集できる: &HashSet<i32>,
) -> AppResult<bool> {
    for c in 接続 {
        match 部品のある場所(db, c.part_instance_id).await? {
            Some(p) if 編集できる.contains(&p) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// この機器がこのプロジェクトから見えるか。**過去に所属した機器も見える**（A-6）。
async fn 見える機器(
    state: &AppState,
    project_id: i32,
    device_id: i32,
) -> AppResult<device::Model> {
    let 内部 = |e: sea_orm::DbErr| AppError::Internal(anyhow::anyhow!(e));
    let 所属したことがある = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::LocationType.eq(PROJECT))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .one(&state.db)
        .await
        .map_err(内部)?
        .is_some();
    if !所属したことがある {
        return Err(AppError::NotFound);
    }
    device::Entity::find_by_id(device_id)
        .one(&state.db)
        .await
        .map_err(内部)?
        .ok_or(AppError::NotFound)
}

/// 書き換えの入口。編集できるロールで、**今このプロジェクトにある機器**に限る。
/// 利用者の言語を返す。
async fn 書き換える機器(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    device_id: i32,
) -> AppResult<&'static str> {
    let (_, can_edit) = 入場(state, current, project_id).await?;
    if !can_edit {
        return Err(AppError::Forbidden);
    }
    見える機器(state, project_id, device_id).await?;
    if !super::device::今ここにあるか(state, project_id, device_id).await? {
        return Err(AppError::Forbidden);
    }
    Ok(Locale::parse(&current.user.locale).as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 二つ組は整数2つだけを通す() {
        assert_eq!(二つ組("3:14"), Some((3, 14)));
        assert_eq!(二つ組("3"), None);
        assert_eq!(二つ組("a:1"), None);
        assert_eq!(二つ組(""), None);
    }
}
