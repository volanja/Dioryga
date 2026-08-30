//! HTTPサーバ。

mod health;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;

use sea_orm::DatabaseConnection;

use crate::config::Config;

/// ハンドラ間で共有する状態。
#[derive(Clone)]
pub struct AppState {
    #[allow(dead_code)]
    pub config: Arc<Config>,
    pub db: DatabaseConnection,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// サーバを起動し、終了シグナルを受けるまで待つ。
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let bind = config.bind;

    let db = crate::db::connect(&config.database).await?;
    if config.database.auto_migrate {
        crate::db::migrate(&db).await?;
    }

    let state = AppState {
        config: Arc::new(config),
        db,
    };

    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "サーバを起動しました");

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("サーバを停止しました");
    Ok(())
}

/// Ctrl-C と SIGTERM のどちらでも停止できるようにする。
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Ctrl-Cハンドラを登録できません");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("SIGTERMハンドラを登録できません")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("終了シグナルを受信しました");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn ヘルスチェックが200を返す() {
        // ヘルスチェックはDBへの疎通まで確認するため、接続が必要。
        // マイグレーションは不要（ping するだけのため）。
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("SQLiteへ接続できませんでした");

        let state = AppState {
            config: Arc::new(Config::default()),
            db,
        };

        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
