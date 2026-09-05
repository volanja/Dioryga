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

    /// 一括取込を行う。**既定はドライラン**（設計書23.6）。
    ///
    /// 18.2により参照されたカタログ行は編集できず、誤った取込は事後修正が
    /// 困難である。**書き込むには `--apply` を明示する。**既定を反映側にすると、
    /// 「必ず2段階」という設計が手順書の中だけの約束になってしまう。
    Import {
        /// 取込ファイル、またはマニフェストのパス。
        path: PathBuf,

        /// 実際に反映する。指定しない場合は差分を表示するのみ。
        #[arg(long)]
        apply: bool,

        /// 取込を行う利用者のメールアドレス。`created_by` と
        /// `IMPORT_RUN.imported_by` に記録する。
        #[arg(long)]
        as_user: String,
    },

    /// データを取込フォーマットで書き出す。
    Export {
        /// 出力先ディレクトリ。
        path: PathBuf,
    },

    /// 本体と依存のライセンス表示を出力する。
    ///
    /// **単一バイナリで配布するため、これが全文を見る唯一の経路になる。**
    Licenses,
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

    /// **既定はドライラン**（設計書23.6）。
    ///
    /// 反映側を既定にすると「必ず2段階」という設計が手順書の中だけの約束に
    /// なる。ここが逆になっていないことを固定する。
    #[test]
    fn 取込の既定はドライラン() {
        let cli = Cli::try_parse_from([
            "dioryga",
            "import",
            "catalog.yaml",
            "--as-user",
            "you@example.com",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Import { apply, .. }) => {
                assert!(!apply, "--apply を指定していないのに反映されます")
            }
            other => panic!("importとして解釈されませんでした: {other:?}"),
        }
    }

    #[test]
    fn applyを指定すると反映になる() {
        let cli = Cli::try_parse_from([
            "dioryga",
            "import",
            "catalog.yaml",
            "--as-user",
            "you@example.com",
            "--apply",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Import { apply, as_user, .. }) => {
                assert!(apply);
                assert_eq!(as_user, "you@example.com");
            }
            other => panic!("importとして解釈されませんでした: {other:?}"),
        }
    }

    /// 取込者の指定は必須。`created_by` と `IMPORT_RUN.imported_by` に
    /// 実在する利用者が要るため（23.7）。
    #[test]
    fn 取込者の指定がなければ受け付けない() {
        assert!(Cli::try_parse_from(["dioryga", "import", "catalog.yaml"]).is_err());
    }
}
