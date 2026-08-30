use clap::Parser;

use dioryga::cli::{self, AdminCommand, Cli, Command};
use dioryga::config::Config;
use dioryga::{admin, db, server, telemetry};

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

        Command::Import { .. } => Err(cli::not_implemented("import", "設計書23章")),
        Command::Export { .. } => Err(cli::not_implemented("export", "設計書23章")),
    }
}
