//! DB接続とマイグレーション。

use std::time::Duration;

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::config::DatabaseConfig;

/// 設定に従ってDBへ接続する。
pub async fn connect(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(config.url.clone());
    options
        .max_connections(config.max_connections)
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .sqlx_logging(false);

    let db = Database::connect(options).await?;
    tracing::info!(backend = ?db.get_database_backend(), "DBへ接続しました");
    Ok(db)
}

/// 未適用のマイグレーションを適用する。
pub async fn migrate(db: &DatabaseConnection) -> anyhow::Result<()> {
    let pending = Migrator::get_pending_migrations(db).await?.len();
    if pending == 0 {
        tracing::info!("未適用のマイグレーションはありません");
        return Ok(());
    }

    tracing::info!(pending, "マイグレーションを適用します");
    Migrator::up(db, None).await?;
    tracing::info!("マイグレーションを適用しました");
    Ok(())
}
