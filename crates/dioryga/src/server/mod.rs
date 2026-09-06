//! HTTPサーバ。

pub mod account;
pub mod admin;
pub mod cable;
pub mod catalog;
pub mod component;
pub mod cost;
pub mod device;
mod health;
pub mod import;
pub mod login;
pub mod member;
pub mod merge;
pub mod network;
pub mod part;
pub mod project;
pub mod rack;
pub mod sbom;
pub mod setup;
pub mod software_catalog;
pub mod view;
pub mod vlan;
pub mod work_order;
pub mod workspace;

use std::sync::Arc;

use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;

use sea_orm::DatabaseConnection;

use crate::auth::middleware::CurrentUser;
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
    /// 取込のドライランと反映のあいだ、アップロード内容を預かる（23.6）。
    pub staged: import::StagedUploads,
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
        .route("/", get(home))
        .route("/setup", get(setup::show).post(setup::submit))
        .route("/login", get(login::show).post(login::submit))
        .route("/logout", post(login::logout))
        .route(
            "/account/password",
            get(account::show).post(account::submit),
        )
        // System Admin領域（設計書16.1のA領域）
        .route("/admin/users", get(admin::list).post(admin::create))
        .route("/admin/users/new", get(admin::new_form))
        .route(
            "/admin/users/{id}",
            get(admin::edit_form).post(admin::update),
        )
        .route("/admin/users/{id}/disable", post(admin::disable))
        .route("/admin/users/{id}/enable", post(admin::enable))
        .route(
            "/admin/users/{id}/reset-password",
            post(admin::reset_password),
        )
        .route("/admin/projects", get(project::list).post(project::create))
        .route("/admin/projects/new", get(project::new_form))
        .route(
            "/admin/projects/{id}",
            get(project::edit_form).post(project::update),
        )
        .route(
            "/admin/projects/{id}/archive",
            get(project::archive_form).post(project::archive),
        )
        .route("/admin/projects/{id}/unarchive", post(project::unarchive))
        .route(
            "/admin/projects/{id}/members",
            get(project::members_form).post(project::update_members),
        )
        // プロジェクト領域（設計書16.1のB領域）。ロールでアクセス制御される
        .route("/projects", get(workspace::list))
        .route(
            "/projects/{id}/devices",
            get(device::list).post(device::create),
        )
        .route(
            "/projects/{id}/members",
            get(member::list).post(member::update),
        )
        .route(
            "/projects/{id}/containers",
            get(rack::list).post(rack::create),
        )
        .route(
            "/projects/{id}/containers/{container_id}",
            get(rack::detail),
        )
        .route(
            "/projects/{id}/containers/{container_id}/mounts",
            post(rack::mount),
        )
        .route(
            "/projects/{id}/containers/{container_id}/unmount",
            post(rack::unmount),
        )
        .route("/projects/{id}/devices/new", get(device::new_form))
        .route(
            "/projects/{id}/devices/{device_id}",
            get(device::detail).post(device::update),
        )
        .route(
            "/projects/{id}/devices/{device_id}/edit",
            get(device::edit_form),
        )
        // 発注は専用の一覧画面を持たず、機器詳細から登録する（10.2）
        .route(
            "/projects/{id}/devices/{device_id}/orders",
            post(device::add_order),
        )
        .route(
            "/projects/{id}/devices/{device_id}/sbom",
            get(sbom::show).post(sbom::upload),
        )
        .route("/projects/{id}/software/components", get(component::search))
        // コスト・契約管理（設計書16.1のB領域、10.2、10.3）
        .route("/projects/{id}/costs", get(cost::dashboard))
        .route(
            "/projects/{id}/costs/maintenance-contracts",
            get(cost::contracts).post(cost::create_contract),
        )
        .route(
            "/projects/{id}/costs/maintenance-contracts/items",
            post(cost::add_contract_item),
        )
        .route(
            "/projects/{id}/costs/fixed-assets",
            get(cost::assets).post(cost::create_asset),
        )
        .route(
            "/projects/{id}/costs/recurring",
            get(cost::recurring).post(cost::create_recurring),
        )
        // ネットワーク管理（設計書16.1のB領域、8.5、14章）
        .route(
            "/projects/{id}/network/subnets",
            get(network::subnets).post(network::create_subnet),
        )
        .route(
            "/projects/{id}/network/ip-addresses",
            get(network::ip_addresses),
        )
        .route(
            "/projects/{id}/devices/{device_id}/interfaces",
            get(network::interfaces).post(network::create_interface),
        )
        .route(
            "/projects/{id}/devices/{device_id}/interfaces/vlans",
            post(network::add_vlan),
        )
        .route(
            "/projects/{id}/devices/{device_id}/interfaces/roles",
            post(network::add_role),
        )
        .route(
            "/projects/{id}/devices/{device_id}/interfaces/ips",
            post(network::add_ip),
        )
        .route(
            "/projects/{id}/devices/{device_id}/interfaces/members",
            post(network::add_member),
        )
        .route(
            "/projects/{id}/devices/{device_id}/interfaces/close",
            post(network::close),
        )
        .route(
            "/projects/{id}/import",
            get(import::show).post(import::upload),
        )
        .route("/projects/{id}/import/apply", post(import::apply))
        .route(
            "/projects/{id}/work-orders",
            get(work_order::list).post(work_order::create),
        )
        .route("/projects/{id}/work-orders/new", get(work_order::new_form))
        .route(
            "/projects/{id}/work-orders/{work_order_id}",
            get(work_order::detail),
        )
        .route(
            "/projects/{id}/work-orders/{work_order_id}/approve",
            post(work_order::approve),
        )
        .route(
            "/projects/{id}/work-orders/{work_order_id}/reserve",
            post(work_order::reserve),
        )
        .route(
            "/projects/{id}/work-orders/{work_order_id}/transition",
            post(work_order::transition),
        )
        // 共有カタログ領域（設計書16.1のD領域、18章）。プロジェクトを横断する
        .route(
            "/catalog/vendors",
            get(catalog::vendors).post(catalog::save_vendor),
        )
        .route(
            "/catalog/chassis-models",
            get(catalog::chassis_models).post(catalog::create_chassis_model),
        )
        .route(
            "/catalog/configurations",
            get(catalog::configurations).post(catalog::create_configuration),
        )
        .route(
            "/catalog/chassis-models/{id}",
            get(catalog::chassis_model_detail),
        )
        .route(
            "/catalog/chassis-models/{id}/slots",
            post(catalog::add_slot),
        )
        .route(
            "/catalog/chassis-models/{id}/slots/remove",
            post(catalog::remove_slot),
        )
        .route(
            "/catalog/configurations/{id}",
            get(catalog::configuration_detail),
        )
        .route(
            "/catalog/configurations/{id}/parts",
            post(catalog::add_part),
        )
        .route(
            "/catalog/configurations/{id}/parts/remove",
            post(catalog::remove_part),
        )
        // 想定消費電力（12.8）
        .route(
            "/catalog/configurations/{id}/power",
            post(catalog::update_power),
        )
        .route("/catalog/merge", get(merge::show))
        .route("/catalog/merge/vendors", post(merge::merge_vendor))
        .route("/catalog/merge/parts", post(merge::merge_part))
        .route("/catalog/parts", get(part::list).post(part::create))
        .route("/catalog/parts/retire", post(part::retire))
        .route("/catalog/parts/{id}", get(part::detail))
        .route("/catalog/parts/{id}/ports", post(part::add_port))
        .route("/catalog/parts/{id}/ports/remove", post(part::remove_port))
        // 電源定格は方式ごとに1行を持つ子テーブル（12.7）
        .route("/catalog/parts/{id}/power-ratings", post(part::add_rating))
        .route(
            "/catalog/parts/{id}/power-ratings/remove",
            post(part::remove_rating),
        )
        // 残りのカタログ（8.7、9.4、8.5）。**取込は無いが手入力はできる**
        .route("/catalog/cables", get(cable::list).post(cable::create))
        .route("/catalog/cables/retire", post(cable::retire))
        .route("/catalog/cables/{id}", get(cable::detail))
        .route("/catalog/cables/{id}/ends", post(cable::add_end))
        .route("/catalog/cables/{id}/ends/remove", post(cable::remove_end))
        .route(
            "/catalog/software",
            get(software_catalog::list).post(software_catalog::create),
        )
        .route("/catalog/software/retire", post(software_catalog::retire))
        .route("/catalog/vlans", get(vlan::list).post(vlan::create))
        .route("/catalog/vlans/retire", post(vlan::retire))
        // 廃番は静的パスで置く。`/catalog/{kind}/retire` は詳細と衝突する
        .route("/catalog/vendors/retire", post(catalog::retire_vendor))
        .route(
            "/catalog/chassis-models/retire",
            post(catalog::retire_chassis_model),
        )
        .route(
            "/catalog/configurations/retire",
            post(catalog::retire_configuration),
        )
        .route("/assets/{*path}", get(view::asset))
        .layer(axum::middleware::from_fn(
            crate::auth::middleware::system_admin_only,
        ))
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

/// ログイン後の入口。
///
/// **利用者によって行き先が違う。**System Adminはプロジェクトデータに触れられない
/// （設計書3章）ため、共通のダッシュボードを置くと片方には常に空になる。
async fn home(Extension(current): Extension<CurrentUser>) -> Response {
    if current.user.is_system_admin {
        Redirect::to("/admin/users").into_response()
    } else {
        Redirect::to("/projects").into_response()
    }
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
        staged: import::StagedUploads::default(),
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
            staged: import::StagedUploads::default(),
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
