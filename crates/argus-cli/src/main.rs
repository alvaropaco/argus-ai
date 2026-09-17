//! ARGUS command-line interface.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "argus",
    version,
    about = "ARGUS AI infrastructure operations runtime"
)]
struct Cli {
    /// Path to the argusd Unix socket.
    #[arg(long, default_value = "/run/argus/argusd.sock")]
    socket: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Query daemon health.
    Health,
    /// Query daemon status.
    Status,
    /// List capabilities.
    Capabilities,
    /// List plugins.
    Plugins,
    /// Read non-secret configuration.
    Config,
    /// Launch the interactive setup TUI.
    Init,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Init) => argus_tui::run(&cli.socket).await,
        Some(cmd) => run_query(&cli.socket, cmd).await,
    }
}

async fn run_query(socket: &PathBuf, cmd: Command) -> Result<()> {
    let mut client = argus_ipc::Client::connect(socket)
        .await
        .with_context(|| format!("failed to connect to argusd at {}", socket.display()))?;

    let operation = match cmd {
        Command::Health => argus_ipc::Operation::HealthGet,
        Command::Status => argus_ipc::Operation::StatusGet,
        Command::Capabilities => argus_ipc::Operation::CapabilitiesList,
        Command::Plugins => argus_ipc::Operation::PluginsList,
        Command::Config => argus_ipc::Operation::ConfigGet,
        Command::Init => unreachable!("init is handled before run_query"),
    };

    let response = client
        .request(operation, Uuid::new_v4(), serde_json::json!({}))
        .await?;

    if response.ok {
        if let Some(result) = response.result {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Ok(())
    } else {
        let code = response.error.as_ref().map(|e| e.code);
        let message = response
            .error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or("unknown error");
        eprintln!("error ({code:?}): {message}");
        std::process::exit(1);
    }
}
