//! CLIの定義。
//!
//! サブコマンドの一覧は設計書24.1に対応する。単一バイナリで配布するため、
//! マイグレーションや管理者作成も外部ツールではなくこのバイナリが担う。

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};

// **説明文は doc comment に書かない**（#191）。clap の derive は doc comment を
// そのまま `--help` に出すため、言語を切り替えられない。説明は locales の
// `cli.*` に置き、[`command`] が実行時に差し込む。ここのコメントは開発者向け。
#[derive(Debug, Parser)]
#[command(name = "dioryga", version)]
pub struct Cli {
    // 省略すると `dioryga.toml` を読み、無ければ既定値で動く。
    // 指定したファイルが無ければエラーで終了する。
    //
    // **既定値を持たせず `Option` にする**（#189）。既定値を持たせると、
    // 「指定されなかった」と「指定されたが存在しない」を区別できない。
    #[arg(long, short, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    // サブコマンドを省略した場合の既定
    Serve,

    Migrate,

    #[command(subcommand)]
    Admin(AdminCommand),

    // 読み取り専用
    Check,

    // **既定はドライラン**（設計書23.6）。
    //
    // 18.2により参照されたカタログ行は編集できず、誤った取込は事後修正が
    // 困難である。**書き込むには `--apply` を明示する。**既定を反映側にすると、
    // 「必ず2段階」という設計が手順書の中だけの約束になってしまう。
    Import {
        path: PathBuf,

        #[arg(long)]
        apply: bool,

        // `created_by` と `IMPORT_RUN.imported_by` に記録する
        #[arg(long)]
        as_user: String,
    },

    Export {
        path: PathBuf,
    },

    // **単一バイナリで配布するため、これが全文を見る唯一の経路になる。**
    Licenses,
}

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    // 初回セットアップ画面が使えない場合の復旧経路（設計書20.8）。
    // 対話なしで実行するには `--name` と `--password-stdin` を指定する（#129）。
    Create {
        // 規則は設計書20.1
        #[arg(long)]
        username: String,

        // **任意**（ログインIDではない）
        #[arg(long)]
        email: Option<String>,

        #[arg(long)]
        name: Option<String>,

        // **コマンドライン引数では受け取らない。**シェルの履歴やプロセス一覧に
        // 平文が残る。標準入力はパスワードで使うため、`--name` も必要になる。
        #[arg(long, requires = "name")]
        password_stdin: bool,
    },

    ResetPassword {
        #[arg(long)]
        username: String,
    },
}

/// 説明文をコンソールの言語で差し込んだ CLI の定義（#191）。
///
/// **`Cli::parse()` ではなくこれを通して解析する。**`Cli::parse()` では
/// 説明文の無い `--help` になる。
pub fn command() -> clap::Command {
    説明を付ける(Cli::command(), crate::console::言語())
}

/// `mut_arg` と `mut_subcommand` は、名前が無ければ panic する。
/// 引数の名前を変えて説明を付け忘れると、テストで落ちる。
fn 説明を付ける(cmd: clap::Command, l: &str) -> clap::Command {
    let t = |key: &str| rust_i18n::t!(key, locale = l).into_owned();
    cmd.about(t("cli.about"))
        .mut_arg("config", |a| a.help(t("cli.config")))
        .mut_subcommand("serve", |c| c.about(t("cli.serve")))
        .mut_subcommand("migrate", |c| c.about(t("cli.migrate")))
        .mut_subcommand("admin", |c| {
            c.about(t("cli.admin"))
                .mut_subcommand("create", |c| {
                    c.about(t("cli.admin_create"))
                        .long_about(t("cli.admin_create_long"))
                        .mut_arg("username", |a| a.help(t("cli.admin_username")))
                        .mut_arg("email", |a| a.help(t("cli.admin_email")))
                        .mut_arg("name", |a| a.help(t("cli.admin_name")))
                        .mut_arg("password_stdin", |a| a.help(t("cli.admin_password_stdin")))
                })
                .mut_subcommand("reset-password", |c| {
                    c.about(t("cli.reset_password"))
                        .mut_arg("username", |a| a.help(t("cli.reset_password_username")))
                })
        })
        .mut_subcommand("check", |c| c.about(t("cli.check")))
        .mut_subcommand("import", |c| {
            c.about(t("cli.import"))
                .long_about(t("cli.import_long"))
                .mut_arg("path", |a| a.help(t("cli.import_path")))
                .mut_arg("apply", |a| a.help(t("cli.import_apply")))
                .mut_arg("as_user", |a| a.help(t("cli.import_as_user")))
        })
        .mut_subcommand("export", |c| {
            c.about(t("cli.export"))
                .mut_arg("path", |a| a.help(t("cli.export_path")))
        })
        .mut_subcommand("licenses", |c| c.about(t("cli.licenses")))
}

/// まだ実装されていないサブコマンドが呼ばれたことを示す。
///
/// **設計書の章番号は出さない**（#191）。利用者が読めない参照を案内しても、
/// 状況の把握には役立たない。
pub fn not_implemented(what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{}",
        rust_i18n::t!(
            "console.not_implemented",
            locale = crate::console::言語(),
            what = what
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cliの定義が矛盾していない() {
        Cli::command().debug_assert();
        for l in ["ja", "en"] {
            説明を付ける(Cli::command(), l).debug_assert();
        }
    }

    /// **すべてのサブコマンドと引数に、両方の言語で説明があること**（#191）。
    ///
    /// 引数を足して `説明を付ける` への追加を忘れると、説明の無い `--help` になる。
    /// キーを locales に書き忘れると、rust-i18n はキーをそのまま返す。
    #[test]
    fn すべてに説明がある() {
        fn 調べる(cmd: &clap::Command, l: &str) {
            let about = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();
            assert!(
                !about.is_empty() && !about.starts_with("cli."),
                "{l}: {} に説明がありません",
                cmd.get_name()
            );
            for arg in cmd.get_arguments() {
                // clap が足す --help / --version は clap の文言のまま
                if matches!(arg.get_id().as_str(), "help" | "version") {
                    continue;
                }
                let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();
                assert!(
                    !help.is_empty() && !help.starts_with("cli."),
                    "{l}: {} の {} に説明がありません",
                    cmd.get_name(),
                    arg.get_id()
                );
            }
            for sub in cmd.get_subcommands() {
                // clap が足す help サブコマンドは clap の文言のまま
                if sub.get_name() != "help" {
                    調べる(sub, l);
                }
            }
        }
        for l in ["ja", "en"] {
            let mut cmd = 説明を付ける(Cli::command(), l);
            cmd.build();
            調べる(&cmd, l);
        }
    }

    #[test]
    fn 説明は言語で切り替わる() {
        let ja = 説明を付ける(Cli::command(), "ja");
        let en = 説明を付ける(Cli::command(), "en");
        assert_ne!(
            ja.get_about().unwrap().to_string(),
            en.get_about().unwrap().to_string()
        );
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
