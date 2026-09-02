use clap::Parser;

use dioryga::cli::{self, AdminCommand, Cli, Command};
use dioryga::config::Config;
use dioryga::{admin, db, import, server, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
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
            // 現時点で扱えるのはカタログYAMLのみ。インスタンスCSVは後続で足す
            let executed = import::run::catalog_file(&conn, &path, &as_user, apply).await?;
            import::run::print_report(&executed, apply);
            Ok(())
        }
        Command::Export { .. } => Err(cli::not_implemented("export", "設計書23章")),
    }
}
