//! CLIの定義。
//!
//! サブコマンドの一覧は設計書24.1に対応する。単一バイナリで配布するため、
//! マイグレーションや管理者作成も外部ツールではなくこのバイナリが担う。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::DEFAULT_CONFIG_PATH;

#[derive(Debug, Parser)]
#[command(
    name = "dioryga",
    version,
    about = "システム構築・運用プロジェクトのためのインフラ台帳"
)]
pub struct Cli {
    /// 設定ファイルのパス。
    #[arg(long, short, global = true, default_value = DEFAULT_CONFIG_PATH)]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// サーバを起動する（サブコマンドを省略した場合の既定）。
    Serve,

    /// マイグレーションを適用する。
    Migrate,

    /// 管理者アカウントを操作する。
    #[command(subcommand)]
    Admin(AdminCommand),

    /// データの整合性を検査する（読み取り専用）。
    Check,

    /// 一括取込を行う。
    Import {
        /// 取込ファイル、またはマニフェストのパス。
        path: PathBuf,

        /// 差分を表示するのみで、実際には反映しない。
        #[arg(long)]
        dry_run: bool,
    },

    /// データを取込フォーマットで書き出す。
    Export {
        /// 出力先ディレクトリ。
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// System Adminを作成する。
    ///
    /// 初回セットアップ画面が使えない場合の復旧経路（設計書20.8）。
    Create {
        #[arg(long)]
        email: String,
    },

    /// ユーザーのパスワードをリセットする。
    ResetPassword {
        #[arg(long)]
        email: String,
    },
}

/// まだ実装されていないサブコマンドが呼ばれたことを示す。
///
/// 対応するissueを案内し、利用者が状況を把握できるようにする。
pub fn not_implemented(what: &str, issue: &str) -> anyhow::Error {
    anyhow::anyhow!("`{what}` はまだ実装されていません（{issue}）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cliの定義が矛盾していない() {
        Cli::command().debug_assert();
    }

    #[test]
    fn サブコマンドを省略できる() {
        let cli = Cli::try_parse_from(["dioryga"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn dry_runを指定できる() {
        let cli = Cli::try_parse_from(["dioryga", "import", "--dry-run", "manifest.yaml"]).unwrap();
        match cli.command {
            Some(Command::Import { dry_run, .. }) => assert!(dry_run),
            other => panic!("importとして解釈されませんでした: {other:?}"),
        }
    }
}
