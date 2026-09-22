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
    /// Upgrade ARGUS (verifies artifacts before install).
    Upgrade,
    /// Manage Argus Cloud connectivity.
    Cloud {
        #[command(subcommand)]
        command: CloudCommand,
    },
}

#[derive(Subcommand)]
enum CloudCommand {
    /// Enroll this installation using a one-time pairing code.
    ///
    /// Omit --code to be prompted; the prompt hides what you type. Passing the
    /// code as a flag is supported for automation, but it can leak into shell
    /// history and process listings.
    Enroll {
        /// The pairing code, for automation only; omit to be prompted securely.
        #[arg(long)]
        code: Option<String>,
    },
    /// Report this installation's cloud connectivity, identity, and backlog.
    Status,
    /// Remove the enrollment, leaving the installation running locally.
    Forget {
        /// Confirm the removal; without it nothing is changed.
        #[arg(long)]
        yes: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => argus_ai_tui::run(&cli.socket).await,
        Some(Command::Init) => {
            let defaults = argus_ai_tui::SetupConfig {
                socket_path: cli.socket.to_string_lossy().into_owned(),
                ..argus_ai_tui::SetupConfig::default()
            };
            argus_ai_tui::run_setup(defaults)
        }
        Some(Command::Upgrade) => run_upgrade(),
        Some(Command::Cloud { command }) => run_cloud(&cli.socket, command).await,
        Some(cmd) => run_query(&cli.socket, cmd).await,
    }
}

async fn run_cloud(socket: &PathBuf, command: CloudCommand) -> Result<()> {
    match command {
        CloudCommand::Enroll { code } => {
            let code = match code {
                Some(code) => code,
                None => read_masked_code()?,
            };
            let mut client = cloud_client(socket).await?;
            let response = client
                .request(
                    argus_ipc::Operation::CloudEnroll,
                    Uuid::new_v4(),
                    serde_json::json!({ "code": code }),
                )
                .await?;
            report(response)
        }
        CloudCommand::Status => {
            let mut client = cloud_client(socket).await?;
            let response = client
                .request(
                    argus_ipc::Operation::CloudStatus,
                    Uuid::new_v4(),
                    serde_json::json!({}),
                )
                .await?;
            report(response)
        }
        CloudCommand::Forget { yes } => {
            // Removal is destructive and irreversible locally, so it needs an
            // explicit confirmation rather than a bare invocation.
            if !yes {
                eprintln!("refusing to remove the enrollment without --yes");
                std::process::exit(2);
            }
            let mut client = cloud_client(socket).await?;
            let response = client
                .request(
                    argus_ipc::Operation::CloudForget,
                    Uuid::new_v4(),
                    serde_json::json!({ "confirm": true }),
                )
                .await?;
            report(response)
        }
    }
}

async fn cloud_client(socket: &PathBuf) -> Result<argus_ipc::Client> {
    argus_ipc::Client::connect(socket)
        .await
        .with_context(|| format!("failed to connect to argusd at {}", socket.display()))
}

fn report(response: argus_ipc::Response) -> Result<()> {
    if response.ok {
        if let Some(result) = response.result {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        return Ok(());
    }

    let label = response.error.as_ref().map(|error| error.code);
    let message = response
        .error
        .as_ref()
        .map(|error| error.message.as_str())
        .unwrap_or("unknown error");
    eprintln!("error ({label:?}): {message}");
    std::process::exit(1);
}

fn read_masked_code() -> Result<String> {
    use crossterm::event::{Event, KeyCode, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    eprint!("Pairing code: ");
    enable_raw_mode().context("cannot switch the terminal to raw mode")?;

    let mut code = String::new();
    let outcome = loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(anyhow::anyhow!("cancelled"));
                }
                KeyCode::Backspace => {
                    if code.pop().is_some() {
                        eprint!("\u{8} \u{8}");
                    }
                }
                KeyCode::Char(character) => {
                    code.push(character);
                    eprint!("*");
                }
                _ => {}
            },
            Ok(_) => {}
            Err(error) => break Err(anyhow::anyhow!("cannot read input: {error}")),
        }
    };

    let _ = disable_raw_mode();
    eprintln!();
    outcome.map(|()| code)
}

fn run_upgrade() -> Result<()> {
    println!(
        "argus {} — self-update is not yet implemented in the bootstrap.",
        env!("CARGO_PKG_VERSION")
    );
    println!("Artifact verification is provided by `argus-install` (ADR-017).");
    Ok(())
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
        Command::Init | Command::Upgrade | Command::Cloud { .. } => {
            unreachable!("handled before run_query")
        }
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
