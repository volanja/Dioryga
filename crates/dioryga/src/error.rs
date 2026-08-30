//! アプリケーション全体のエラー型。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// HTTPハンドラが返すエラー。
///
/// 変種の多くはP3（認証・認可）以降のハンドラで使う。エラーの表現を先に
/// 決めておくことで、ハンドラごとに場当たりな応答を返すことを防ぐ。
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("リソースが見つかりません")]
    NotFound,

    #[error("この操作を行う権限がありません")]
    Forbidden,

    #[error("認証が必要です")]
    Unauthorized,

    #[error("入力が正しくありません: {0}")]
    Validation(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();

        // 内部エラーの詳細は記録するが、応答には含めない。
        if let Self::Internal(ref err) = self {
            tracing::error!(error = ?err, "内部エラー");
        }

        let body = match self {
            Self::Internal(_) => "内部エラーが発生しました".to_owned(),
            other => other.to_string(),
        };

        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
