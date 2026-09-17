//! 一括取込の画面（設計書16.1のB領域、23.6）。
//!
//! # マニフェストを画面では使わない
//!
//! CLIはマニフェスト（23.5）でプロジェクト・`as_of`・`match_on`・ファイル一覧を
//! 受け取るが、**画面ではプロジェクトがURLで決まっており、残りはフォームの項目に
//! できる。**ブラウザは相対パスのCSVを解決できないため、マニフェストを要求すると
//! 「zipに固めて上げる」ような回り道になる。画面はCSV1本を直接受け取る。
//!
//! # ドライランと反映のあいだ、ファイルを預かる
//!
//! 23.6が2段階を求める以上、差分を見てから反映するまでのあいだ、**同じ内容**が
//! 要る。利用者に2回選ばせるのは誤り（1回目と2回目で違うファイルを選びうる）。
//! アップロードされた内容を短時間だけ記憶し、トークンで参照する。
//!
//! # htmxを使わない
//!
//! 16.8で「取込UIの実装時に再検討する」としていた。**結論は使わない。**
//! 再検討の理由だったドライラン結果の逐次表示と進捗は、**取込を非同期にした
//! 場合にのみ必要**になる。v1は同期実行であり、進捗を見せる相手がいない。
//! 大量件数はCLI（`dioryga import`）が担う。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Multipart, Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use chrono::{DateTime, Duration, Utc};
use entity::{app_user, import_run, project};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::auth::authorization;
use crate::auth::middleware::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::import::{catalog, file_hash, instances, Outcome, Report};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

/// 受け付けるファイルの上限。
///
/// **大量件数はCLIが担う**（23.3の想定は数十万行）。画面で数十MBを同期処理
/// させると、応答を待つあいだにプロキシが切る。上限に当たったら、その旨と
/// CLIの使い方を伝える。
const MAX_UPLOAD: usize = 8 * 1024 * 1024;

/// 預かったファイルを保持する時間。
const STAGE_TTL_MINUTES: i64 = 30;

// ---------------------------------------------------------------------------
// 預かり（ドライラン → 反映）
// ---------------------------------------------------------------------------

/// 預かる内容。
///
/// **ドライランの結果を決めた条件を丸ごと持つ。**ファイルだけを預かって
/// `match_on` や `as_of` を反映時のフォームから取り直すと、**差分を見せた条件と
/// 反映する条件がずれうる。**それでは2段階にした意味がない（23.6）。
struct Staged {
    user_id: i32,
    project_id: i32,
    filename: String,
    bytes: Vec<u8>,
    match_on: Vec<String>,
    as_of: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
}

/// ドライランと反映のあいだ、アップロード内容を預かる。
///
/// **ディスクに書かない。**上限が8MiBであり、消し忘れの掃除を持ち込むより
/// メモリで完結させるほうが単純である。プロセスが落ちれば消えるが、
/// その場合は取り込まれていないので害はない。
#[derive(Clone, Default)]
pub struct StagedUploads(Arc<Mutex<HashMap<String, Staged>>>);

impl StagedUploads {
    fn put(&self, token: String, staged: Staged) {
        let mut map = self.0.lock().expect("預かり庫のロックが壊れています");
        // 期限切れをここで捨てる。掃除のためのタスクを持たない
        let 期限 = Utc::now() - Duration::minutes(STAGE_TTL_MINUTES);
        map.retain(|_, s| s.at > 期限);
        map.insert(token, staged);
    }

    /// 取り出して消す。**反映は一度きり**——同じトークンで二度流させない。
    fn take(&self, token: &str, user_id: i32, project_id: i32) -> Option<Staged> {
        let mut map = self.0.lock().expect("預かり庫のロックが壊れています");
        match map.get(token) {
            // 他人が預けたもの・他プロジェクトのものは渡さない
            Some(s) if s.user_id == user_id && s.project_id == project_id => map.remove(token),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct HistoryRow {
    kind: String,
    imported_at: String,
    imported_by: String,
    summary: String,
}

#[derive(askama::Template)]
#[template(path = "import.html")]
struct ImportPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_file: String,
    t_file_hint: String,
    t_match_on: String,
    t_match_on_hint: String,
    t_as_of: String,
    t_as_of_hint: String,
    t_submit: String,
    t_history: String,
    t_kind: String,
    t_imported_at: String,
    t_imported_by: String,
    t_summary: String,
    t_history_empty: String,
    t_cli_hint: String,
    match_on_options: Vec<&'static str>,
    history: Vec<HistoryRow>,
    error: Option<String>,
}

struct ReportRow {
    outcome: String,
    target: String,
    detail: String,
    is_error: bool,
}

#[derive(askama::Template)]
#[template(path = "import_report.html")]
struct ReportPage {
    chrome: Chrome,
    project_id: i32,
    project_name: String,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_outcome: String,
    t_target: String,
    t_detail: String,
    t_apply: String,
    t_blocked: String,
    filename: String,
    summary: String,
    rows: Vec<ReportRow>,
    token: String,
    /// **エラーがあれば反映させない**（23.6）。ボタン自体を出さない
    can_apply: bool,
}

/// 突合キーの選択肢（設計書23.2の③）。
const MATCH_ON: &[&str] = &["serial_number", "asset_number", "hostname", "external_id"];

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

pub async fn show(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
) -> AppResult<Response> {
    let project = 入場(&state, &current, project_id).await?;
    render(&画面(&state, &current, &project, None).await?)
}

async fn 画面(
    state: &AppState,
    current: &CurrentUser,
    project: &project::Model,
    error: Option<String>,
) -> AppResult<ImportPage> {
    let l = Locale::parse(&current.user.locale).as_str();

    Ok(ImportPage {
        chrome: Chrome::project(
            &state.db,
            &current.user,
            current.csrf_token.clone(),
            project,
            "import",
        )
        .await,
        project_id: project.id,
        project_name: project.name.clone(),
        t_title: rust_i18n::t!("import.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("import.lead", locale = l).to_string(),
        t_file: rust_i18n::t!("import.file", locale = l).to_string(),
        t_file_hint: rust_i18n::t!("import.file_hint", locale = l).to_string(),
        t_match_on: rust_i18n::t!("import.match_on", locale = l).to_string(),
        t_match_on_hint: rust_i18n::t!("import.match_on_hint", locale = l).to_string(),
        t_as_of: rust_i18n::t!("import.as_of", locale = l).to_string(),
        t_as_of_hint: rust_i18n::t!("import.as_of_hint", locale = l).to_string(),
        t_submit: rust_i18n::t!("import.submit", locale = l).to_string(),
        t_history: rust_i18n::t!("import.history", locale = l).to_string(),
        t_kind: rust_i18n::t!("import.kind", locale = l).to_string(),
        t_imported_at: rust_i18n::t!("import.imported_at", locale = l).to_string(),
        t_imported_by: rust_i18n::t!("import.imported_by", locale = l).to_string(),
        t_summary: rust_i18n::t!("import.summary", locale = l).to_string(),
        t_history_empty: rust_i18n::t!("import.history_empty", locale = l).to_string(),
        t_cli_hint: rust_i18n::t!("import.cli_hint", locale = l).to_string(),
        match_on_options: MATCH_ON.to_vec(),
        history: 取込履歴(state, project.id).await?,
        error,
    })
}

/// このプロジェクトへの取込履歴（設計書23.7）。
async fn 取込履歴(state: &AppState, project_id: i32) -> AppResult<Vec<HistoryRow>> {
    let runs = import_run::Entity::find()
        .filter(import_run::Column::ProjectId.eq(project_id))
        .order_by_desc(import_run::Column::ImportedAt)
        .all(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut rows = Vec::new();
    for run in runs {
        let 実行者 = app_user::Entity::find_by_id(run.imported_by)
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
            .map(|u| u.name)
            .unwrap_or_default();

        rows.push(HistoryRow {
            kind: run.kind,
            imported_at: run.imported_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            imported_by: 実行者,
            summary: format!(
                "新規 {} / 更新 {} / 警告 {}",
                run.created_count, run.updated_count, run.warning_count
            ),
        });
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// アップロード → ドライラン（設計書23.6）
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Upload {
    filename: String,
    bytes: Vec<u8>,
    match_on: Vec<String>,
    as_of: Option<DateTime<Utc>>,
}

pub async fn upload(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    multipart: Multipart,
) -> AppResult<Response> {
    let project = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let upload = match 受け取る(multipart).await {
        Ok(u) if !u.bytes.is_empty() => u,
        Ok(_) => return 差し戻す(&state, &current, &project, "import.no_file", l).await,
        Err(key) => return 差し戻す(&state, &current, &project, key, l).await,
    };

    // ドライランはここで行う。**DBを書き換えない**
    let source = String::from_utf8_lossy(&upload.bytes).into_owned();
    let report = match 差分を出す(&state, project_id, &upload, &source).await {
        Ok(Ok(r)) => r,
        Ok(Err(message)) => {
            let page = 画面(&state, &current, &project, Some(message)).await?;
            return render(&page);
        }
        Err(e) => return Err(e),
    };

    // 反映のために内容を預かる（トークンで参照する）
    let token =
        crate::auth::csrf::generate().map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    state.staged.put(
        token.clone(),
        Staged {
            user_id: current.user.id,
            project_id,
            filename: upload.filename.clone(),
            bytes: upload.bytes,
            // ドライランで使った条件をそのまま預かる
            match_on: upload.match_on.clone(),
            as_of: upload.as_of,
            at: Utc::now(),
        },
    );

    render(&レポート画面(
        &current,
        &project,
        &upload.filename,
        report,
        token,
        l,
    ))
}

/// 入力の誤りを伝えて、アップロード画面へ戻す。
async fn 差し戻す(
    state: &AppState,
    current: &CurrentUser,
    project: &project::Model,
    key: &str,
    locale: &str,
) -> AppResult<Response> {
    let page = 画面(
        state,
        current,
        project,
        Some(rust_i18n::t!(key, locale = locale).to_string()),
    )
    .await?;
    render(&page)
}

async fn 受け取る(mut multipart: Multipart) -> Result<Upload, &'static str> {
    let mut upload = Upload::default();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "file" => {
                upload.filename = field.file_name().unwrap_or("upload").to_owned();
                let bytes = field.bytes().await.map_err(|_| "import.too_large")?;
                if bytes.len() > MAX_UPLOAD {
                    return Err("import.too_large");
                }
                upload.bytes = bytes.to_vec();
            }
            "match_on" => {
                if let Ok(v) = field.text().await {
                    if MATCH_ON.contains(&v.as_str()) {
                        upload.match_on.push(v);
                    }
                }
            }
            "as_of" => {
                if let Ok(v) = field.text().await {
                    if !v.trim().is_empty() {
                        // `<input type="date">` は YYYY-MM-DD を送る
                        match chrono::NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d") {
                            Ok(d) => {
                                upload.as_of = d
                                    .and_hms_opt(0, 0, 0)
                                    .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                            }
                            Err(_) => return Err("import.bad_as_of"),
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(upload)
}

/// ファイルの種別を判別し、ドライランを行う。
///
/// **拡張子で決める。**中身を推測して外すより、利用者に見えている情報で
/// 決めるほうが説明できる。
async fn 差分を出す(
    state: &AppState,
    project_id: i32,
    upload: &Upload,
    source: &str,
) -> AppResult<Result<Report, String>> {
    let lower = upload.filename.to_lowercase();

    if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        return Ok(match catalog::parse(source) {
            Ok(file) => Ok(catalog::dry_run(&state.db, &file)
                .await
                .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?),
            Err(e) => Err(e.to_string()),
        });
    }

    if lower.ends_with(".csv") {
        return Ok(match instances::parse_devices(source) {
            Ok(rows) => Ok(
                instances::dry_run(&state.db, project_id, &rows, &upload.match_on)
                    .await
                    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?,
            ),
            Err(e) => Err(e.to_string()),
        });
    }

    Ok(Err(format!(
        "「{}」は取り込めません（.csv か .yaml）",
        upload.filename
    )))
}

fn レポート画面(
    current: &CurrentUser,
    project: &project::Model,
    filename: &str,
    report: Report,
    token: String,
    l: &str,
) -> ReportPage {
    let can_apply = !report.has_error();
    let summary = report.to_string();

    let rows = report
        .entries
        .iter()
        // 変更なしの行は出さない。**差分を見る画面**であり、
        // 数百行の「変更なし」に埋もれさせない（件数は要約に出ている）
        .filter(|e| e.outcome != Outcome::Unchanged)
        .map(|e| ReportRow {
            outcome: e.outcome.as_str().to_owned(),
            target: e.target.clone(),
            detail: e.detail.clone(),
            is_error: e.outcome == Outcome::Error,
        })
        .collect();

    ReportPage {
        // 取込は編集権を確かめてから入る画面なので、取込の項目は常に出る
        chrome: Chrome::project_known(
            &current.user,
            current.csrf_token.clone(),
            project,
            "import",
            true,
        ),
        project_id: project.id,
        project_name: project.name.clone(),
        t_title: rust_i18n::t!("import.report_title", locale = l).to_string(),
        t_lead: rust_i18n::t!("import.report_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("import.back", locale = l).to_string(),
        t_outcome: rust_i18n::t!("import.outcome", locale = l).to_string(),
        t_target: rust_i18n::t!("import.target", locale = l).to_string(),
        t_detail: rust_i18n::t!("import.detail", locale = l).to_string(),
        t_apply: rust_i18n::t!("import.apply", locale = l).to_string(),
        t_blocked: rust_i18n::t!("import.blocked", locale = l).to_string(),
        filename: filename.to_owned(),
        summary,
        rows,
        token,
        can_apply,
    }
}

// ---------------------------------------------------------------------------
// 反映
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct ApplyForm {
    /// 預かりを指す。**条件はここから取り出す**——フォームから取り直さない。
    pub token: String,
}

pub async fn apply(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<i32>,
    axum::Form(form): axum::Form<ApplyForm>,
) -> AppResult<Response> {
    let project = 入場(&state, &current, project_id).await?;
    let l = Locale::parse(&current.user.locale).as_str();

    let Some(staged) = state.staged.take(&form.token, current.user.id, project_id) else {
        // 期限切れか、二度目の反映
        let page = 画面(
            &state,
            &current,
            &project,
            Some(rust_i18n::t!("import.expired", locale = l).to_string()),
        )
        .await?;
        return render(&page);
    };

    let source = String::from_utf8_lossy(&staged.bytes).into_owned();
    let lower = staged.filename.to_lowercase();
    let now = Utc::now();
    // **ドライランと同じ条件で反映する。**預かった値を使う
    let as_of = staged.as_of.unwrap_or(now);

    let カタログ = lower.ends_with(".yaml") || lower.ends_with(".yml");

    let run = import_run::ActiveModel {
        // カタログ取込はプロジェクトに属さない（18.1）
        project_id: Set(if カタログ { None } else { Some(project_id) }),
        kind: Set(if カタログ { "catalog" } else { "instances" }.to_owned()),
        file_hash: Set(file_hash(&staged.bytes)),
        as_of: Set(if カタログ { now } else { as_of }),
        created_count: Set(0),
        updated_count: Set(0),
        warning_count: Set(0),
        imported_by: Set(current.user.id),
        imported_at: Set(now),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let 結果 = if カタログ {
        let file = catalog::parse(&source).map_err(|e| AppError::Validation(e.to_string()))?;
        catalog::apply(&state.db, &file, current.user.id, run.id).await
    } else {
        let rows =
            instances::parse_devices(&source).map_err(|e| AppError::Validation(e.to_string()))?;
        instances::apply(
            &state.db,
            project_id,
            &rows,
            &staged.match_on,
            as_of,
            run.id,
        )
        .await
    };

    let report = 結果.map_err(|e| AppError::Validation(e.to_string()))?;

    // 件数を確定させる（設計書23.7）
    let mut active: import_run::ActiveModel = run.clone().into();
    active.created_count = Set(report.count(Outcome::Created) as i32);
    active.updated_count = Set(report.count(Outcome::Updated) as i32);
    active.warning_count = Set(report.count(Outcome::Warning) as i32);
    active
        .update(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to(&format!("/projects/{project_id}/import")).into_response())
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// **取込にはOperator以上が要る**（設計書23.5）。System Adminは入れない（3章）。
async fn 入場(
    state: &AppState,
    current: &CurrentUser,
    project_id: i32,
) -> AppResult<project::Model> {
    let project = project::Entity::find_by_id(project_id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    authorization::require_project_editor(&state.db, &current.user, project_id)
        .await
        .map_err(|_| AppError::Forbidden)?;

    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 預かる(store: &StagedUploads, token: &str, user_id: i32, project_id: i32) {
        store.put(
            token.to_owned(),
            Staged {
                user_id,
                project_id,
                filename: "devices.csv".to_owned(),
                bytes: b"hostname\nweb01\n".to_vec(),
                match_on: vec!["serial_number".to_owned()],
                as_of: None,
                at: Utc::now(),
            },
        );
    }

    /// **ドライランで使った条件が預かられること。**反映時のフォームから
    /// 取り直すと、差分を見せた条件と反映する条件がずれる（23.6）。
    #[test]
    fn 条件も一緒に預かる() {
        let store = StagedUploads::default();
        預かる(&store, "t0", 1, 10);

        let staged = store.take("t0", 1, 10).unwrap();
        assert_eq!(staged.match_on, ["serial_number"]);
    }

    /// **反映は一度きり。**同じトークンで二度流せると、履歴が二重に入りうる。
    #[test]
    fn 預かりは一度しか取り出せない() {
        let store = StagedUploads::default();
        預かる(&store, "t1", 1, 10);

        assert!(store.take("t1", 1, 10).is_some());
        assert!(store.take("t1", 1, 10).is_none(), "二度取り出せます");
    }

    /// **他人の預かりを取り出せないこと。**トークンが漏れても、
    /// 別の利用者・別プロジェクトの取込を実行させない。
    #[test]
    fn 他人の預かりは取り出せない() {
        let store = StagedUploads::default();
        預かる(&store, "t2", 1, 10);

        assert!(store.take("t2", 2, 10).is_none(), "他人が取り出せます");
        assert!(
            store.take("t2", 1, 99).is_none(),
            "他プロジェクトから取り出せます"
        );
        assert!(store.take("t2", 1, 10).is_some());
    }

    #[test]
    fn 期限切れは捨てられる() {
        let store = StagedUploads::default();
        store.0.lock().unwrap().insert(
            "古い".to_owned(),
            Staged {
                user_id: 1,
                project_id: 10,
                filename: "old.csv".to_owned(),
                bytes: Vec::new(),
                match_on: Vec::new(),
                as_of: None,
                at: Utc::now() - Duration::minutes(STAGE_TTL_MINUTES + 1),
            },
        );

        // 次の預け入れが掃除の契機になる
        預かる(&store, "新しい", 1, 10);
        assert!(store.take("古い", 1, 10).is_none());
        assert!(store.take("新しい", 1, 10).is_some());
    }
}
