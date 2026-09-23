//! テスト用のDBを用意するヘルパ。
//!
//! **コンテナランタイムへの依存をこのファイルに閉じ込める。**
//! `Dioryga_Design/docs/design/dev-environment.md` 付録Aの通り、現時点では
//! Colima + testcontainers-rs を採用しているが、Apple container + container-rs へ
//! 乗り換える余地を残すため、テスト本体からは起動方法が見えないようにする。
//! 乗り換えの際に書き換えるのはこのファイルだけで済む。
//!
//! # PostgreSQL側はテンプレートDBを複製する（#76）
//!
//! 以前はテスト1本ごとにコンテナを起動してマイグレーションを流していた。
//! **これはテストの内容によらない固定費で、実測で1.30秒/本。**288本で383秒に
//! なっており、テストを1本足すごとにCIが1.3秒伸びる構造だった。
//!
//! 代わりに、**プロセスにつきコンテナを1つだけ起動し、マイグレーションも
//! 1回だけ流してテンプレートDBを作る。**各テストは
//! `CREATE DATABASE ... TEMPLATE` で自分のDBを得る。**PostgreSQLのテンプレート
//! 複製はファイルのコピーであり、マイグレーションを流し直すより桁で速い。**
//!
//! **テストごとに独立したDBである点は変わらない。**ここは崩してはいけない
//! ——共有すると、並列実行しているテストが互いのデータを見る。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;

/// テストで使うPostgreSQLの版。
///
/// **リポジトリ直下の `.postgres-version` を唯一の出所とする。**`scripts/schema-docs.sh`
/// も同じファイルを読む。**同じ版を2箇所に書くと必ず食い違う**——実際、
/// テストが既定の `11-alpine`、スキーマドキュメントの生成が `17-alpine` という
/// 状態が続いており、**スキーマの検査と動作の検証が別のPostgreSQLに対して
/// 行われていた**（#76）。
///
/// # 版を明示する理由
///
/// testcontainers-modules の既定は `11-alpine` で、**PostgreSQL 11 は2023年に
/// サポートが終わっている。**指定しないままだと、EOLの版に対して
/// 「PostgreSQLで動く」を検証することになる。
///
/// **版はテストの速さにも効く。**PostgreSQL 15 で `CREATE DATABASE` の既定の
/// 複製方式が `WAL_LOG` になり、複製のたびに2回走っていたチェックポイントが
/// 要らなくなった。同じ55本のテストで**11-alpineは15.9秒、17-alpineは4.7秒**
/// ——テンプレートDBを使う本体（#76）はこの性質に乗っている。
const PG: &str = include_str!("../../../../../.postgres-version");

/// 複製元になるDBの名前。
const テンプレート: &str = "dioryga_template";

/// テスト用DB。
pub struct TestDb {
    pub conn: DatabaseConnection,
    /// **保持している間だけコンテナが生きる。**最後の `TestDb` が落ちた時点で
    /// [`共有`] が drop され、testcontainers がコンテナを消す。
    _共有: Option<Arc<共有>>,
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
        conn, _共有: None
    }
}

/// PostgreSQLのテスト用DBを用意する。Dockerが必要。
///
/// macOSでColimaを使う場合、`DOCKER_HOST` の設定が要る
/// （`/var/run/docker.sock` が作られないため）。
pub async fn postgres() -> TestDb {
    // **コンテナと管理用接続は専用のランタイムの上で扱う。**理由は
    // [`ランタイム`] を参照。テスト自身のランタイムで触ってはいけない
    let (共有, 名前) = ランタイム
        .spawn(async {
            let 共有 = 共有を得る().await;
            let 名前 = 共有.複製を作る().await;
            (共有, 名前)
        })
        .await
        .expect("テスト用DBの準備に失敗しました");

    // こちらはテスト自身のランタイムでよい。**このテストと寿命を共にする**
    let conn = 接続(&共有.接続先(&名前)).await;

    TestDb {
        conn,
        _共有: Some(共有),
    }
}

// ---------------------------------------------------------------------------
// プロセス内で1つだけ持つコンテナ
// ---------------------------------------------------------------------------

/// 共有する資源を動かすための、プロセス内に1つだけあるランタイム。
///
/// # なぜテスト自身のランタイムではいけないか
///
/// `#[tokio::test]` は**テストごとに新しいランタイムを作り、終わったら畳む。**
/// コンテナのクライアントも sqlx のプールも、**自分を作ったランタイムの上に
/// バックグラウンドのタスクを持つ。**最初のテストのランタイムでこれらを作ると、
/// そのテストが終わった時点でタスクごと消え、**後続のテストは応答の来ない
/// 接続を掴んで待ち続ける**（実際に「Connection pool timed out」で落ちた）。
///
/// **落ちないランタイムを1つ用意し、共有する資源はその上でだけ動かす。**
static ランタイム: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("テスト用のランタイムを作れませんでした")
});

/// 起動済みのコンテナとテンプレートDB。
struct 共有 {
    /// **drop されるとコンテナが消えるため、使い終わるまで持ち続ける。**
    _container: ContainerAsync<Postgres>,
    port: u16,
    /// `CREATE DATABASE` を投げるための接続（`postgres` DBに繋ぐ）。
    admin: DatabaseConnection,
    /// 複製したDBの通し番号。
    連番: AtomicU32,
    /// **複製を直列化する。**`CREATE DATABASE ... TEMPLATE` は複製元が使用中だと
    /// 失敗しうるため、同時に走らせない。複製自体はファイルコピーで速く、
    /// 直列にしても支配的にはならない。
    複製の順番: tokio::sync::Mutex<()>,
}

/// **静的に持たず `Weak` で持つ。**`static` に `ContainerAsync` を置くと drop が
/// 走らず、**プロセスが終わってもコンテナが残る。**最後の `TestDb` が落ちれば
/// 片付くようにしておく。
static 現在の共有: LazyLock<tokio::sync::Mutex<Weak<共有>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(Weak::new()));

async fn 共有を得る() -> Arc<共有> {
    let mut 枠 = 現在の共有.lock().await;
    if let Some(生きている) = 枠.upgrade() {
        return 生きている;
    }

    let 新しい = Arc::new(共有::起動する().await);
    *枠 = Arc::downgrade(&新しい);
    新しい
}

impl 共有 {
    async fn 起動する() -> Self {
        let container = Postgres::default()
            // `.postgres-version` の末尾には改行が入る
            .with_tag(PG.trim())
            .start()
            .await
            .expect("PostgreSQLコンテナを起動できませんでした");

        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("ポートを取得できませんでした");

        let admin = 接続(&接続先(port, "postgres")).await;

        admin
            .execute_unprepared(&format!("CREATE DATABASE {テンプレート}"))
            .await
            .expect("テンプレートDBを作成できませんでした");

        // **マイグレーションはここで1回だけ流す。**
        //
        // 流し終えたら接続を閉じる——`CREATE DATABASE ... TEMPLATE` は
        // **複製元に接続が残っていると失敗する。**
        let 種 = 接続(&接続先(port, テンプレート)).await;
        Migrator::up(&種, None)
            .await
            .expect("テンプレートDBへのマイグレーションに失敗しました");
        種.close()
            .await
            .expect("テンプレートDBの接続を閉じられませんでした");

        Self {
            _container: container,
            port,
            admin,
            連番: AtomicU32::new(0),
            複製の順番: tokio::sync::Mutex::new(()),
        }
    }

    fn 接続先(&self, db: &str) -> String {
        接続先(self.port, db)
    }

    /// テンプレートを複製して、このテスト専用のDBを作る。
    ///
    /// **作ったDBは消さない。**コンテナごと捨てるため後始末の必要がなく、
    /// `DROP DATABASE` のぶんテストが遅くなるだけになる。
    async fn 複製を作る(&self) -> String {
        let 名前 = format!("test_{}", self.連番.fetch_add(1, Ordering::Relaxed));

        let _順番 = self.複製の順番.lock().await;
        self.admin
            .execute_unprepared(&format!("CREATE DATABASE {名前} TEMPLATE {テンプレート}"))
            .await
            .unwrap_or_else(|e| panic!("{名前} を複製できませんでした: {e}"));

        名前
    }
}

fn 接続先(port: u16, db: &str) -> String {
    format!("postgres://postgres:postgres@127.0.0.1:{port}/{db}")
}

/// **接続数を明示的に絞る。**コンテナを共有するようになったため、
/// 並列実行しているテストのプールが1つの`max_connections`（既定100）を
/// 取り合う。sea-ormの既定は1プールあたり100で、放っておくと枯渇する。
async fn 接続(url: &str) -> DatabaseConnection {
    let mut opt = ConnectOptions::new(url.to_owned());
    opt.max_connections(4)
        .min_connections(0)
        .connect_timeout(Duration::from_secs(30))
        .idle_timeout(Duration::from_secs(5));

    Database::connect(opt)
        .await
        .expect("PostgreSQLへ接続できませんでした")
}
