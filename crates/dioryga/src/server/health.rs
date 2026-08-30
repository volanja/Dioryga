//! ヘルスチェック。
//!
//! 設計書2章のCI構成において、Windowsジョブが「バイナリ起動 → ヘルスチェック疎通」を
//! スモークテストとして実行する。認証を要求しない数少ないエンドポイントの1つ（20.10）。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::server::AppState;

pub async fn health(State(state): State<AppState>) -> AppResult<Json<Value>> {
    // DBへ到達できることまで確認する。接続できないまま起動している状態を
    // 「正常」と報告しないため。
    state
        .db
        .ping()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    })))
}
