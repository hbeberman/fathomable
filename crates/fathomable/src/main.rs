// @okf-doc: /decisions/0009-cli-and-diagnostics.md
#![forbid(unsafe_code)]
//! Fathomable binary: terminal UI, MCP server, and admin flags (ADR 0009).

mod doctor;
mod logging;
mod viewer;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use fathomable_core::XdgDirs;

/// Read-only terminal workspace viewer and annotation side-car.
#[derive(Debug, Parser)]
#[command(name = "fathomable", version, about)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each admin flag is a distinct switch mandated by ADR 0009"
)]
struct Cli {
    /// File to view, or workspace directory (default: current directory).
    path: Option<PathBuf>,

    /// Run the stdio MCP server instead of the TUI.
    #[arg(long)]
    mcp: bool,

    /// Override the configuration file.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Select a theme from the config themes directory for this run.
    #[arg(long, value_name = "NAME")]
    theme: Option<String>,

    /// Print terminal, directory, and log diagnostics and exit.
    #[arg(long)]
    doctor: bool,

    /// List running sessions and their sockets.
    #[arg(long)]
    sessions: bool,

    /// Print session and thread state as JSON.
    #[arg(long)]
    dump_state: bool,

    /// Re-emit the log for a session in order.
    #[arg(long)]
    replay_log: bool,

    /// Print the effective configuration after defaults and overrides.
    #[arg(long)]
    config_show: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let dirs = XdgDirs::from_env();

    if cli.doctor {
        return doctor::run(&dirs);
    }

    let session = logging::Session::new();
    let _guard = match logging::init(&dirs, &session) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("fathomable: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(session = %session.id(), "starting");

    if !cli.mcp && !cli.sessions && !cli.dump_state && !cli.replay_log && !cli.config_show {
        let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
        if path.is_file() {
            return match viewer::run(&path, session.id()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    tracing::error!(error = format!("{error:#}"), "viewer failed");
                    eprintln!("fathomable: {error:#}");
                    ExitCode::FAILURE
                }
            };
        }
    }

    let unimplemented = if cli.mcp {
        "--mcp"
    } else if cli.sessions {
        "--sessions"
    } else if cli.dump_state {
        "--dump-state"
    } else if cli.replay_log {
        "--replay-log"
    } else if cli.config_show {
        "--config-show"
    } else {
        "workspace mode (fathomable [DIR])"
    };
    tracing::warn!(feature = unimplemented, "not implemented");
    eprintln!(
        "fathomable: {unimplemented} is not implemented yet; only --doctor works in this build"
    );
    ExitCode::FAILURE
}
