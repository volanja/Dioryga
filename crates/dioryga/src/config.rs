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
        }
    }
}

impl Config {
    /// 既定値 → 設定ファイル → 環境変数 の順に重ねて読み込む。
    ///
    /// 環境変数は `DIORYGA_BIND` のようにプレフィックス付きで、
    /// ネストは `DIORYGA_DATABASE__URL` のように `__` で区切る。
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(path))
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

    #[test]
    fn 設定ファイルが無くても既定値で読み込める() {
        let config = Config::load(Path::new("存在しない.toml")).unwrap();
        assert_eq!(config.bind.port(), 8080);
        assert_eq!(config.timezone, "Asia/Tokyo");
    }

    #[test]
    fn 環境変数が設定ファイルより優先される() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("dioryga.toml", r#"bind = "0.0.0.0:9999""#)?;
            jail.set_env("DIORYGA_BIND", "127.0.0.1:7777");

            let config = Config::load(Path::new("dioryga.toml")).unwrap();
            assert_eq!(config.bind.port(), 7777);
            Ok(())
        });
    }

    #[test]
    fn ネストした設定を環境変数で上書きできる() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("DIORYGA_DATABASE__URL", "postgres://localhost/dioryga");

            let config = Config::load(Path::new("存在しない.toml")).unwrap();
            assert_eq!(config.database.url, "postgres://localhost/dioryga");
            Ok(())
        });
    }
}
