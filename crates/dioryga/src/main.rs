use clap::Parser;

use dioryga::cli::{self, AdminCommand, Cli, Command};
use dioryga::config::Config;
use dioryga::{admin, db, import, server, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // **設定の読み込みより前に処理する。**ライセンス表示は設定ファイルにも
    // DBにも依存せず、配布物を受け取った人が最初に確認しうるものである。
    // 設定が無いと出せない、という状態にしてはならない。
    if let Some(Command::Licenses) = cli.command {
        print!("{}", dioryga::licenses::notices());
        return Ok(());
    }

    let config = Config::load(&cli.config)?;

    telemetry::init(&config.log_filter);

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => server::serve(config).await,

        Command::Migrate => {
            let conn = db::connect(&config.database).await?;
            db::migrate(&conn).await
        }

        Command::Admin(AdminCommand::Create { email }) => admin::create(&config, &email).await,
        Command::Admin(AdminCommand::ResetPassword { email }) => {
            admin::reset_password(&config, &email).await
        }

        Command::Check => Err(cli::not_implemented("check", "設計書24.5")),

        Command::Import {
            path,
            apply,
            as_user,
        } => {
            let conn = db::connect(&config.database).await?;
            // ファイルの kind で、カタログとインスタンスを振り分ける
            let executed = import::run::run(&conn, &path, &as_user, apply).await?;
            import::run::print_report(&executed, apply);
            Ok(())
        }
        Command::Export { .. } => Err(cli::not_implemented("export", "設計書23章")),

        // 設定の読み込み前に処理済み
        Command::Licenses => unreachable!(),
    }
}
