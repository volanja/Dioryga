//! SBOM取込・差分表示（設計書16.1のB領域、9章）。
//!
//! # 取込は同期で行う
//!
//! 一括取込UI（23章）が「数万行を超える初期投入はCLI、日常の追加は画面」と
//! 切り分けたのと同じで、SBOMは**1台につき1ファイル**であり、数千件でも
//! 秒で終わる。非同期にする理由が無い（16.8でhtmxを見送った判断と同根）。
//!
//! # 行ごとの監査ログを書かない
//!
//! `SBOM_COMPONENT_INDEX` は1回の取込で数千行入りうる。24.4が一括取込について
//! 定めたのと同じ理由で、**[`Actor::Import`] のトランザクションで書く。**
//! 追跡は `SBOM_IMPORT` と `IMPORT_RUN` が担う。
//!
//! # 取込は何も自動生成しない
//!
//! `SOFTWARE_INSTANCE` も `VENDOR` も作らない（9.6、18.3）。**観測記録であって
//! 資産の登録ではない。**画面にもその旨を出す——「取り込んだのに資産一覧に
//! 出てこない」という誤解を防ぐ。
//!
//! # クロスプロジェクト可視性
//!
//! スナップショットは複数プロジェクトの機器間で共有されうるが、**閲覧できるのは
//! 自分が権限を持つDeviceの取込行を通じてのみ**である（9.8）。URLが
//! `/projects/{id}/devices/{device_id}/sbom` を通ることでこれを担保する。

use axum::extract::{Multipart, Path, State};
use axum::response::Response;
use axum::Extension;
use chrono::Utc;
use entity::{device, import_run, project, sbom_component_change, sbom_import};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::sbom::{apply, parse};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 受け付ける上限。一括取込UI（23章）と揃える。
///
/// **10,000台規模の初期投入をこの画面で行う想定ではない。**1台ぶんのSBOMは
/// 数千件でも数百KBに収まる。
const 上限: usize = 8 * 1024 * 1024;

/// `IMPORT_RUN.import_type`。
const SBOM: &str = "sbom";

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct HistoryRow {
    imported_at: String,
    source_format: String,
    imported_by: String,
    component_count: i32,
    changes: usize,
    /// 現在の観測結果（`superseded_at IS NULL`）。
    current: bool,
}

struct ChangeRow {
    change_type: String,
    name: String,
    version_from: String,
    version_to: String,
}

#[derive(askama::Template)]
#[template(path = "sbom.html")]
struct SbomPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    device_id: i32,
    hostname: String,
    t_title: String,
    t_lead: String,
    t_no_auto_hint: String,
    t_back: String,
    t_upload: String,
    t_file: String,
    t_file_hint: String,
    t_submit: String,
    t_history: String,
    t_imported_at: String,
    t_source_format: String,
    t_imported_by: String,
    t_components: String,
    t_changes: String,
    t_current: String,
    t_no_history: String,
    t_latest_changes: String,
    t_no_change: String,
    t_change_type: String,
    t_name: String,
    t_from: String,
    t_to: String,
    history: Vec<HistoryRow>,
    /// 最新の取込の差分。**変化が無ければ0件**（9.6の手順3）。
    changes: Vec<ChangeRow>,
    can_edit: bool,
    error: Option<String>,
    notice: Option<String>,
}

pub async fn show(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
) -> AppResult<Response> {
    描く(&state, &current, project_id, device_id, None, None).await
}

async fn 描く(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
    device_id: i32,
    error: Option<String>,
    notice: Option<String>,
) -> AppResult<Response> {
    let (project, can_edit) = 入場(state, current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();
    let d = 機器(state, project_id, device_id).await?;

    let imports = sbom_import::Entity::find()
        .filter(sbom_import::Column::DeviceId.eq(device_id))
        .order_by_desc(sbom_import::Column::Id)
        .all(&state.db)
        .await
        .map_err(db)?;

    let mut history = Vec::new();
    for i in &imports {
        history.push(HistoryRow {
            imported_by: 氏名(&state.db, i.imported_by).await?,
            component_count: 件数(&state.db, &i.content_hash).await?,
            changes: 差分の件数(&state.db, i.id).await?,
            current: i.superseded_at.is_none(),
            imported_at: i.imported_at.format("%Y-%m-%d %H:%M").to_string(),
            source_format: i.source_format.clone(),
        });
    }

    // 直近の取込の差分だけを出す。**過去のぶんは履歴から辿る**
    let changes = match imports.first() {
        Some(i) => 差分行(&state.db, i.id).await?,
        None => Vec::new(),
    };

    render(&SbomPage {
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
        hostname: d.hostname.clone(),
        t_title: rust_i18n::t!("sbom.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("sbom.lead", locale = l).to_string(),
        t_no_auto_hint: rust_i18n::t!("sbom.no_auto_hint", locale = l).to_string(),
        t_back: rust_i18n::t!("devices.back", locale = l).to_string(),
        t_upload: rust_i18n::t!("sbom.upload", locale = l).to_string(),
        t_file: rust_i18n::t!("import.file", locale = l).to_string(),
        t_file_hint: rust_i18n::t!("sbom.file_hint", locale = l).to_string(),
        t_submit: rust_i18n::t!("sbom.submit", locale = l).to_string(),
        t_history: rust_i18n::t!("sbom.history", locale = l).to_string(),
        t_imported_at: rust_i18n::t!("sbom.imported_at", locale = l).to_string(),
        t_source_format: rust_i18n::t!("sbom.source_format", locale = l).to_string(),
        t_imported_by: rust_i18n::t!("sbom.imported_by", locale = l).to_string(),
        t_components: rust_i18n::t!("sbom.components", locale = l).to_string(),
        t_changes: rust_i18n::t!("sbom.changes", locale = l).to_string(),
        t_current: rust_i18n::t!("sbom.current", locale = l).to_string(),
        t_no_history: rust_i18n::t!("sbom.no_history", locale = l).to_string(),
        t_latest_changes: rust_i18n::t!("sbom.latest_changes", locale = l).to_string(),
        t_no_change: rust_i18n::t!("sbom.no_change", locale = l).to_string(),
        t_change_type: rust_i18n::t!("sbom.change_type", locale = l).to_string(),
        t_name: rust_i18n::t!("sbom.name", locale = l).to_string(),
        t_from: rust_i18n::t!("sbom.version_from", locale = l).to_string(),
        t_to: rust_i18n::t!("sbom.version_to", locale = l).to_string(),
        history,
        changes,
        can_edit,
        error,
        notice,
    })
}

// ---------------------------------------------------------------------------
// 取込
// ---------------------------------------------------------------------------

pub async fn upload(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path((project_id, device_id)): Path<(i32, i32)>,
    multipart: Multipart,
) -> AppResult<Response> {
    入場(&state, &current, project_id).await?;
    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let l = Locale::parse(&current.user.locale).as_str();
    let 訳 = |key: &str| rust_i18n::t!(key, locale = l).to_string();
    機器(&state, project_id, device_id).await?;

    let Some((filename, bytes)) = 受け取る(multipart).await? else {
        return 描く(
            &state,
            &current,
            project_id,
            device_id,
            Some(訳("import.error_no_file")),
            None,
        )
        .await;
    };

    if bytes.len() > 上限 {
        return 描く(
            &state,
            &current,
            project_id,
            device_id,
            Some(訳("import.error_too_large")),
            None,
        )
        .await;
    }

    let normalized = match parse::parse(&bytes) {
        Ok(n) => n,
        // **読めなかった理由をそのまま出す。**「失敗しました」だけでは、
        // 形式が違うのか壊れているのか分からない
        Err(e) => {
            return 描く(
                &state,
                &current,
                project_id,
                device_id,
                Some(訳(e.key())),
                None,
            )
            .await;
        }
    };

    let now = Utc::now();
    let _ = filename;

    // **書き込む前に件数を確定させる**（23.6のドライランと同じ形）。
    // `IMPORT_RUN` は件数の列を持ち、行を作る時点で値が要る
    let plan = apply::計画(&state.db, device_id, &normalized).await?;

    // **取込の実行記録を作る**（23.7）。SBOMでは `SBOM_IMPORT` が誰が何を
    // いつ取り込んだかを持つため、この行の主な役目は**`AUDIT_LOG` に
    // `import_run_id` を与えること**である（24.4）。
    //
    // 件数の列は行単位の取込向けに作られており、SBOMの差分は3種類ある。
    // `removed` に対応する列が無いので、そこは `SBOM_COMPONENT_CHANGE` が持つ。
    // `IMPORT_RUN` 自体は監査ログの対象にしない（`import` モジュールと同じ）。
    // **この行が監査ログの参照先**であり、自分を指す監査ログを作ると入れ子になる
    let run = import_run::ActiveModel {
        project_id: Set(Some(project_id)),
        kind: Set(SBOM.to_owned()),
        file_hash: Set(ファイルのハッシュ(&bytes)),
        as_of: Set(now),
        created_count: Set(plan.added() as i32),
        updated_count: Set(plan.version_changed() as i32),
        warning_count: Set(0),
        imported_by: Set(current.user.id),
        imported_at: Set(now),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .map_err(db)?;

    let tx = AuditedTx::begin(
        &state.db,
        Actor::Import {
            import_run_id: run.id,
        },
    )
    .await
    .map_err(db)?;
    apply::反映(
        &tx,
        device_id,
        current.user.id,
        None,
        &normalized,
        &plan,
        now,
    )
    .await?;
    tx.commit().await.map_err(db)?;

    let notice = if plan.unchanged {
        // **構成に変化なし**（9.6の手順3）。差分は0件になる
        rust_i18n::t!(
            "sbom.done_unchanged",
            locale = l,
            count = plan.component_count
        )
        .to_string()
    } else {
        rust_i18n::t!(
            "sbom.done",
            locale = l,
            count = plan.component_count,
            changes = plan.changes.len(),
            reused = if plan.snapshot_reused {
                訳("sbom.reused")
            } else {
                訳("sbom.stored")
            }
        )
        .to_string()
    };

    描く(&state, &current, project_id, device_id, None, Some(notice)).await
}

/// multipartから1つのファイルを取り出す。
async fn 受け取る(mut multipart: Multipart) -> AppResult<Option<(String, Vec<u8>)>> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("sbom.json").to_owned();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if bytes.is_empty() {
            return Ok(None);
        }
        return Ok(Some((filename, bytes.to_vec())));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

fn ファイルのハッシュ(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

async fn 差分行<C: ConnectionTrait>(db_conn: &C, import_id: i32) -> AppResult<Vec<ChangeRow>> {
    Ok(sbom_component_change::Entity::find()
        .filter(sbom_component_change::Column::SbomImportId.eq(import_id))
        .order_by_asc(sbom_component_change::Column::Id)
        .all(db_conn)
        .await
        .map_err(db)?
        .into_iter()
        .map(|c| ChangeRow {
            change_type: c.change_type,
            name: c.name,
            version_from: c.version_from.unwrap_or_default(),
            version_to: c.version_to.unwrap_or_default(),
        })
        .collect())
}

async fn 差分の件数<C: ConnectionTrait>(db_conn: &C, import_id: i32) -> AppResult<usize> {
    Ok(sbom_component_change::Entity::find()
        .filter(sbom_component_change::Column::SbomImportId.eq(import_id))
        .all(db_conn)
        .await
        .map_err(db)?
        .len())
}

async fn 件数<C: ConnectionTrait>(db_conn: &C, content_hash: &str) -> AppResult<i32> {
    Ok(
        entity::sbom_snapshot::Entity::find_by_id(content_hash.to_owned())
            .one(db_conn)
            .await
            .map_err(db)?
            .map(|s| s.component_count)
            .unwrap_or(0),
    )
}

async fn 氏名<C: ConnectionTrait>(db_conn: &C, id: i32) -> AppResult<String> {
    Ok(entity::app_user::Entity::find_by_id(id)
        .one(db_conn)
        .await
        .map_err(db)?
        .map(|u| u.name)
        .unwrap_or_default())
}

/// この機器がこのプロジェクトから見えるか。
///
/// **A-6により、過去に所属した機器の履歴も見える**（3章）。`to_date` で
/// 絞らないのは `device` モジュールと同じ判断である。
async fn 機器(state: &AppState, project_id: i32, device_id: i32) -> AppResult<device::Model> {
    use entity::device_assignment;

    let d = device::Entity::find_by_id(device_id)
        .one(&state.db)
        .await
        .map_err(db)?
        .ok_or(AppError::NotFound)?;

    let 所属したことがある = device_assignment::Entity::find()
        .filter(device_assignment::Column::DeviceId.eq(device_id))
        .filter(device_assignment::Column::LocationType.eq("Project"))
        .filter(device_assignment::Column::LocationId.eq(project_id))
        .one(&state.db)
        .await
        .map_err(db)?
        .is_some();

    if 所属したことがある {
        Ok(d)
    } else {
        Err(AppError::NotFound)
    }
}

async fn 入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<(project::Model, bool)> {
    let project = project::Entity::find_by_id(project_id)
        .one(&state.db)
        .await
        .map_err(db)?
        .ok_or(AppError::NotFound)?;

    authorization::require_project_member(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    let can_edit = authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .is_ok();

    Ok((project, can_edit))
}

fn db(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(anyhow::anyhow!(e))
}
