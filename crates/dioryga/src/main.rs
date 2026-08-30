use clap::Parser;

use dioryga::cli::{self, AdminCommand, Cli, Command};
use dioryga::config::Config;
use dioryga::{db, server, telemetry};

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

        Command::Admin(AdminCommand::Create { .. }) => {
            Err(cli::not_implemented("admin create", "P3: 認証"))
        }
        Command::Admin(AdminCommand::ResetPassword { .. }) => {
            Err(cli::not_implemented("admin reset-password", "P3: 認証"))
        }

        Command::Check => Err(cli::not_implemented("check", "設計書24.5")),

        Command::Import { .. } => Err(cli::not_implemented("import", "設計書23章")),
        Command::Export { .. } => Err(cli::not_implemented("export", "設計書23章")),
    }
}
