//! テスト用のDBを用意するヘルパ。
//!
//! **コンテナランタイムへの依存をこのファイルに閉じ込める。**
//! `Dioryga_Design/docs/design/dev-environment.md` 付録Aの通り、現時点では
//! Colima + testcontainers-rs を採用しているが、Apple container + container-rs へ
//! 乗り換える余地を残すため、テスト本体からは起動方法が見えないようにする。
//! 乗り換えの際に書き換えるのはこのファイルだけで済む。

use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, DatabaseConnection};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// テスト用DB。`_container` を保持している間だけコンテナが生きる。
pub struct TestDb {
    pub conn: DatabaseConnection,
    _container: Option<ContainerAsync<Postgres>>,
}

/// SQLiteのインメモリDBを用意する。Dockerを必要としない。
pub async fn sqlite() -> TestDb {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("SQLiteへ接続できませんでした");
    Migrator::up(&conn, None)
        .await
        .expect("SQLiteへのマイグレーションに失敗しました");
    TestDb {
        conn,
        _container: None,
    }
}

/// PostgreSQLをコンテナで起動する。Dockerが必要。
///
/// macOSでColimaを使う場合、`DOCKER_HOST` の設定が要る
/// （`/var/run/docker.sock` が作られないため）。
pub async fn postgres() -> TestDb {
    let container = Postgres::default()
        .start()
        .await
        .expect("PostgreSQLコンテナを起動できませんでした");

    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("ポートを取得できませんでした");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    let conn = Database::connect(&url)
        .await
        .expect("PostgreSQLへ接続できませんでした");
    Migrator::up(&conn, None)
        .await
        .expect("PostgreSQLへのマイグレーションに失敗しました");

    TestDb {
        conn,
        _container: Some(container),
    }
}
