//! CLIの定義。
//!
//! サブコマンドの一覧は設計書24.1に対応する。単一バイナリで配布するため、
//! マイグレーションや管理者作成も外部ツールではなくこのバイナリが担う。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "dioryga",
    version,
    about = "システム構築・運用プロジェクトのためのインフラ台帳"
)]
pub struct Cli {
    /// 設定ファイルのパス。省略すると `dioryga.toml` を読み、無ければ既定値で動く。
    /// 指定したファイルが無ければエラーで終了する。
    ///
    /// **既定値を持たせず `Option` にする**（#189）。既定値を持たせると、
    /// 「指定されなかった」と「指定されたが存在しない」を区別できない。
    #[arg(long, short, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

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

        /// 取込を行う利用者のユーザー名。`created_by` と
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
    ///
    /// 対話なしで実行するには `--name` と `--password-stdin` を指定し、
    /// パスワードを標準入力から渡す（#129）。
    ///
    /// ```bash
    /// printf '%s\n' "$PASSWORD" | dioryga admin create --username admin --name 管理者 --password-stdin
    /// ```
    Create {
        /// ログインID。英小文字・数字と . _ -、2〜32文字（設計書20.1）。
        #[arg(long)]
        username: String,

        /// メールアドレス。**任意**（ログインIDではない）。
        #[arg(long)]
        email: Option<String>,

        /// 表示名。省略すると対話で聞く。
        #[arg(long)]
        name: Option<String>,

        /// パスワードを標準入力の1行目から読む。
        ///
        /// **コマンドライン引数では受け取らない。**シェルの履歴やプロセス一覧に
        /// 平文が残る。標準入力はパスワードで使うため、`--name` も必要になる。
        #[arg(long, requires = "name")]
        password_stdin: bool,
    },

    /// ユーザーのパスワードをリセットする。
    ResetPassword {
        /// 対象のユーザー名。
        #[arg(long)]
        username: String,
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

    /// **`-c` の指定の有無を区別できること**（#189）。既定値を持たせると、
    /// 指定されたが存在しないパスを「指定されなかった」と見分けられない。
    #[test]
    fn 設定ファイルの指定の有無を区別できる() {
        let cli = Cli::try_parse_from(["dioryga", "migrate"]).unwrap();
        assert!(cli.config.is_none());

        // サブコマンドの後ろでも受け付ける（global）
        let cli = Cli::try_parse_from(["dioryga", "migrate", "-c", "prod.toml"]).unwrap();
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("prod.toml"))
        );
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

    /// 引数を付けなければ、従来どおり対話で聞く（#129）。
    #[test]
    fn 管理者作成は引数なしなら対話になる() {
        let cli = Cli::try_parse_from(["dioryga", "admin", "create", "--username", "yarigatake"])
            .unwrap();
        match cli.command {
            Some(Command::Admin(AdminCommand::Create {
                name,
                password_stdin,
                ..
            })) => {
                assert!(name.is_none());
                assert!(!password_stdin);
            }
            other => panic!("admin createとして解釈されませんでした: {other:?}"),
        }
    }

    /// **標準入力はパスワードで使うため、表示名は引数で要る**（#129）。
    ///
    /// 受け付けると、表示名を対話で聞こうとして標準入力を読み、パスワードを
    /// 表示名として登録してしまう。
    #[test]
    fn 標準入力のパスワードには表示名の指定が要る() {
        assert!(Cli::try_parse_from([
            "dioryga",
            "admin",
            "create",
            "--username",
            "yarigatake",
            "--password-stdin",
        ])
        .is_err());

        let cli = Cli::try_parse_from([
            "dioryga",
            "admin",
            "create",
            "--username",
            "yarigatake",
            "--name",
            "槍ヶ岳 大川",
            "--password-stdin",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Admin(AdminCommand::Create {
                name,
                password_stdin,
                ..
            })) => {
                assert_eq!(name.as_deref(), Some("槍ヶ岳 大川"));
                assert!(password_stdin);
            }
            other => panic!("admin createとして解釈されませんでした: {other:?}"),
        }
    }

    /// **メールアドレスは任意、ユーザー名は必須**（設計書20.1、#136）。
    #[test]
    fn 管理者作成はユーザー名が必須でメールアドレスは任意() {
        assert!(Cli::try_parse_from(["dioryga", "admin", "create"]).is_err());
        assert!(Cli::try_parse_from([
            "dioryga",
            "admin",
            "create",
            "--email",
            "a@example.invalid"
        ])
        .is_err());

        let cli = Cli::try_parse_from([
            "dioryga",
            "admin",
            "create",
            "--username",
            "yarigatake",
            "--email",
            "yarigatake@example.invalid",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Admin(AdminCommand::Create {
                username, email, ..
            })) => {
                assert_eq!(username, "yarigatake");
                assert_eq!(email.as_deref(), Some("yarigatake@example.invalid"));
            }
            other => panic!("admin createとして解釈されませんでした: {other:?}"),
        }
    }

    /// **パスワードを引数で受け取る経路を作らない**（#129）。
    #[test]
    fn パスワードを引数では受け取らない() {
        assert!(Cli::try_parse_from([
            "dioryga",
            "admin",
            "create",
            "--username",
            "yarigatake",
            "--name",
            "槍ヶ岳 大川",
            "--password",
            "Tanigawa-Bridge-7391",
        ])
        .is_err());
    }
}
