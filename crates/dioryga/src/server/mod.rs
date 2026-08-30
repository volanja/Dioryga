//! HTTPサーバ。

mod health;
pub mod login;
pub mod setup;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;

use sea_orm::DatabaseConnection;

use crate::auth::password::PasswordService;
use crate::auth::setup::SetupState;
use crate::config::Config;

/// ハンドラ間で共有する状態。
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: DatabaseConnection,
    pub passwords: Arc<PasswordService>,
    pub setup: SetupState,
}

pub fn router(state: AppState) -> Router {
    // 層の順序（外側から）：
    //   トレース → セットアップ誘導 → 認証 → CSRF検証 → System Adminガード
    // `.layer()` は後に足したものが外側になるため、逆順に積んでいる。
    //
    // System Adminガードを認証より内側に置くのは、認証が解決した利用者を
    // 見る必要があるため。ロール判定とは独立した層にしている（設計書3章）。
    Router::new()
        .route("/health", get(health::health))
        .route("/setup", get(setup::show).post(setup::submit))
        .route("/login", get(login::show).post(login::submit))
        .route("/logout", axum::routing::post(login::logout))
        // プロジェクト領域。画面の中身は後続のissueで実装する。
        // System Adminガードの対象であることを確かめるために置いている。
        .route("/projects", get(projects_placeholder))
        .layer(axum::middleware::from_fn(
            crate::auth::middleware::system_admin_guard,
        ))
        .layer(axum::middleware::from_fn(
            crate::auth::middleware::verify_csrf,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::middleware::authenticate,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            setup::redirect_while_pending,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// プロジェクト一覧の仮実装。中身は後続のissueで作る。
async fn projects_placeholder() -> &'static str {
    "プロジェクト一覧（未実装）"
}

/// サーバを起動し、終了シグナルを受けるまで待つ。
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let bind = config.bind;

    let db = crate::db::connect(&config.database).await?;
    if config.database.auto_migrate {
        crate::db::migrate(&db).await?;
    }

    let passwords = Arc::new(PasswordService::new(config.password.clone())?);
    let (setup_state, setup_token) = SetupState::initialize(&db).await?;
    if let Some(token) = &setup_token {
        crate::auth::setup::print_instructions(&bind, token);
    }

    let state = AppState {
        config: Arc::new(config),
        db,
        passwords,
        setup: setup_state,
    };

    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "サーバを起動しました");

    // ログイン試行の記録にクライアントIPが要るため ConnectInfo を有効にする
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
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

        let config = Config::default();
        let state = AppState {
            passwords: Arc::new(PasswordService::new(config.password.clone()).unwrap()),
            // 利用者が存在する体にして、セットアップへの誘導を無効にする
            setup: SetupState::default(),
            config: Arc::new(config),
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
