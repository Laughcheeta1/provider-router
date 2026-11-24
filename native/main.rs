//! Native Provider Router executable entry point.

mod infrastructure;
mod presentation;

use std::{io::IsTerminal, path::PathBuf};

use anyhow::{Result, ensure};
use clap::Parser;
use provider_router::{application, domain};

use infrastructure::{documents, inference::LocalInference, profile::Profile, supervisor};
use presentation::{cli as commands, http, terminal};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Store settings and credentials in a dedicated account-private directory.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<commands::Command>,
} // RevExt: 5

fn main() -> Result<()> {
    if std::env::args_os().len() == 2
        && std::env::args_os()
            .nth(1)
            .is_some_and(|arg| arg == "--document-worker")
    {
        return documents::worker();
    } // RevExt: 1
    application()
} // RevExt: 6

#[tokio::main]
async fn application() -> Result<()> {
    let cli = Cli::parse();
    if cli.command.is_none() {
        ensure!(
            std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            "Open provider-router in an interactive terminal, or run provider-router --help."
        );
    } // RevExt: 2

    let profile = Profile::open(
        cli.data_dir
            .or_else(|| std::env::var_os("PROVIDER_ROUTER_DATA_DIR").map(PathBuf::from)),
    )?;
    if !matches!(
        &cli.command,
        Some(
            commands::Command::Serve
                | commands::Command::InternalServe { .. }
                | commands::Command::InternalWorker
                | commands::Command::InternalHandoff
                | commands::Command::InternalInstall { .. }
                | commands::Command::InternalUninstall
        )
    ) {
        supervisor::forward_cli(&profile).await?;
    } // RevExt: 3
    match cli.command {
        Some(commands::Command::InternalWorker) => {
            supervisor::serve_worker(|shutdown| {
                http::router(LocalInference::new(profile), shutdown)
            })
            .await
        }
        Some(command) => commands::execute(profile, command).await,
        None => terminal::run(profile, infrastructure::provider::Catalog::new()?).await,
    } // RevExt: 4
} // RevExt: 7
