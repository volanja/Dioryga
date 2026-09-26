//! 設定の読み込み。
//!
//! 設定ファイル（TOML）を読み、環境変数 `DIORYGA_*` で上書きする。
//! どちらも無い場合は既定値で起動できる（単一バイナリを配布してすぐ動かせるようにするため）。

use std::net::SocketAddr;
use std::path::Path;

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

/// 設定ファイルの既定のパス。
pub const DEFAULT_CONFIG_PATH: &str = "dioryga.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// HTTPサーバの待ち受けアドレス。
    pub bind: SocketAddr,

    /// 表示に用いるタイムゾーン（IANA形式）。
    ///
    /// 日時はすべてUTCで保存し、表示時にのみこの値へ変換する（設計書24.2.3）。
    /// v1ではユーザーごとに持たず、サーバ全体で単一の値とする。
    pub timezone: String,

    /// `RUST_LOG` 形式のログフィルタ。
    pub log_filter: String,

    pub database: DatabaseConfig,

    pub password: PasswordConfig,

    pub session: SessionConfig,
}

/// パスワードハッシュ化の設定（設計書20.2、20.3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordConfig {
    /// Argon2idのメモリコスト（KiB）。
    pub memory_kib: u32,
    /// 反復回数。
    pub iterations: u32,
    /// 並列度。
    pub parallelism: u32,
    /// 同時に走るArgon2idの数。1回あたり memory_kib を占有するため、
    /// 制限しないとログインの集中がメモリ枯渇を招く（設計書24.2）。
    pub max_concurrent_hashes: usize,
    pub min_length: usize,
    pub max_length: usize,
}

/// セッションの設定（設計書20.5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// 最後の利用からこの時間が経過すると失効する。
    pub idle_timeout_secs: i64,
    /// 発行からこの時間が経過すると、利用中でも失効する。
    pub absolute_timeout_secs: i64,
    /// CookieにSecure属性を付けるか。HTTPSで動作している場合のみ真にする。
    /// ローカルHTTP起動という形態があるため設定で切り替える。
    pub cookie_secure: bool,
}

impl Default for PasswordConfig {
    fn default() -> Self {
        Self {
            // OWASPが示す最小構成（設計書20.2）
            memory_kib: 19_456,
            iterations: 2,
            parallelism: 1,
            max_concurrent_hashes: 4,
            min_length: 12,
            max_length: 256,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 8 * 60 * 60,
            absolute_timeout_secs: 24 * 60 * 60,
            cookie_secure: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// 接続URL。SQLite と PostgreSQL のどちらも受け付ける（設計書2章）。
    pub url: String,

    pub max_connections: u32,

    pub connect_timeout_secs: u64,

    /// 起動時に未適用のマイグレーションを自動で適用するか。
    ///
    /// 単一バイナリを配布してすぐ動かせることを重視し、既定で有効にする（設計書1.1）。
    /// 複数インスタンスが同一のPostgreSQLを共有する運用では、適用の時期を制御する
    /// ために無効化し、`dioryga migrate` を明示的に実行する。
    pub auto_migrate: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: ([127, 0, 0, 1], 8080).into(),
            timezone: "Asia/Tokyo".to_owned(),
            log_filter: "info".to_owned(),
            database: DatabaseConfig {
                url: "sqlite://dioryga.db?mode=rwc".to_owned(),
                max_connections: 10,
                connect_timeout_secs: 10,
                auto_migrate: true,
            },
            password: PasswordConfig::default(),
            session: SessionConfig::default(),
        }
    }
}

impl Config {
    /// 既定値 → 設定ファイル → 環境変数 の順に重ねて読み込む。
    ///
    /// 環境変数は `DIORYGA_BIND` のようにプレフィックス付きで、
    /// ネストは `DIORYGA_DATABASE__URL` のように `__` で区切る。
    ///
    /// `explicit` は `-c` で明示されたパス。**明示されたファイルが無ければ
    /// 誤りにする**（設計書24.1、#189）。読み飛ばすと、パスを打ち間違えたまま
    /// 既定値で起動する。既定値の接続先はカレントディレクトリのSQLiteなので、
    /// PostgreSQLを使うつもりの環境で空のSQLiteを作って動いてしまう。
    ///
    /// 明示しなければ既定のパス（[`DEFAULT_CONFIG_PATH`]）を読み、無ければ
    /// 読み飛ばす。ファイルを置かずに起動できることを優先する。
    pub fn load(explicit: Option<&Path>) -> anyhow::Result<Self> {
        let file = match explicit {
            Some(path) => {
                if !path.is_file() {
                    anyhow::bail!(
                        "設定ファイルが見つかりません: {}（-c で指定したパス）",
                        path.display()
                    );
                }
                // **指定されたパスだけを読む。**`Toml::file` は親ディレクトリまで
                // 探すため、別の場所の同名ファイルを読みうる
                Toml::file_exact(path)
            }
            None => Toml::file(DEFAULT_CONFIG_PATH),
        };

        let config = Figment::from(Serialized::defaults(Config::default()))
            .merge(file)
            .merge(Env::prefixed("DIORYGA_").split("__"))
            .extract()?;
        Ok(config)
    }
}

#[cfg(test)]
// figment::Jail のクロージャが返すエラー型が大きいという指摘だが、
// これはfigment側のAPI形状であり、こちらでは変更できない。
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    // Jail は実際のプロセスの環境変数を書き換えるため、Jail を使わないテストが
    // 並行実行されると他のテストが設定した値を読んでしまう。Jail 同士は
    // figment 側のロックで直列化されるので、環境変数を触らないテストも
    // Jail の中で実行する。
    #[test]
    fn 設定ファイルが無くても既定値で読み込める() {
        figment::Jail::expect_with(|_| {
            let config = Config::load(None).unwrap();
            assert_eq!(config.bind.port(), 8080);
            assert_eq!(config.timezone, "Asia/Tokyo");
            assert!(config.database.auto_migrate);
            Ok(())
        });
    }

    #[test]
    fn 環境変数が設定ファイルより優先される() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("dioryga.toml", r#"bind = "0.0.0.0:9999""#)?;
            jail.set_env("DIORYGA_BIND", "127.0.0.1:7777");

            let config = Config::load(None).unwrap();
            assert_eq!(config.bind.port(), 7777);
            Ok(())
        });
    }

    #[test]
    fn ネストした設定を環境変数で上書きできる() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("DIORYGA_DATABASE__URL", "postgres://localhost/dioryga");

            let config = Config::load(None).unwrap();
            assert_eq!(config.database.url, "postgres://localhost/dioryga");
            Ok(())
        });
    }

    /// **明示したファイルが無ければ誤りにする**（#189）。打ち間違えたパスで
    /// 既定値のまま動かない。
    #[test]
    fn 明示したファイルが無ければ誤りにする() {
        figment::Jail::expect_with(|_| {
            let e = Config::load(Some(Path::new("typo.toml"))).unwrap_err();
            let message = e.to_string();
            assert!(
                message.contains("typo.toml"),
                "パスが示されていません: {message}"
            );
            Ok(())
        });
    }

    #[test]
    fn 明示したファイルを読む() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("custom.toml", r#"bind = "0.0.0.0:9999""#)?;

            let config = Config::load(Some(Path::new("custom.toml"))).unwrap();
            assert_eq!(config.bind.port(), 9999);
            Ok(())
        });
    }

    /// **明示したパスは、親ディレクトリを探さない。**探すと、打ち間違えたパスが
    /// 別の場所の同名ファイルに当たって黙って読まれる。
    #[test]
    fn 明示したパスは親ディレクトリを探さない() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("custom.toml", r#"bind = "0.0.0.0:9999""#)?;
            jail.change_dir(jail.create_dir("sub")?)?;

            assert!(Config::load(Some(Path::new("custom.toml"))).is_err());
            Ok(())
        });
    }

    /// 明示したファイルがあっても、環境変数が優先する（優先順位は変わらない）。
    #[test]
    fn 明示したファイルより環境変数が優先される() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("custom.toml", r#"bind = "0.0.0.0:9999""#)?;
            jail.set_env("DIORYGA_BIND", "127.0.0.1:7777");

            let config = Config::load(Some(Path::new("custom.toml"))).unwrap();
            assert_eq!(config.bind.port(), 7777);
            Ok(())
        });
    }
}
