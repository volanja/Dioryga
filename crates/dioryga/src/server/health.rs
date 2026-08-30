//! ヘルスチェック。
//!
//! 設計書2章のCI構成において、Windowsジョブが「バイナリ起動 → ヘルスチェック疎通」を
//! スモークテストとして実行する。認証を要求しない数少ないエンドポイントの1つ（20.10）。

use axum::Json;
use serde_json::{json, Value};

use crate::error::AppResult;

pub async fn health() -> AppResult<Json<Value>> {
    Ok(Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    })))
}
