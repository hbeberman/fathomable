// @okf-doc: /decisions/0009-cli-and-diagnostics.md
#![forbid(unsafe_code)]
//! Fathomable binary: terminal UI, MCP server, and admin flags (ADR 0009).

mod app;
mod doctor;
mod logging;
mod mcp;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::Store;
use fathomable_core::config::Config;
use fathomable_core::highlight::Highlighter;
use fathomable_core::session::{Id, Record};
use fathomable_core::theme::{DEFAULT_THEME, Theme};
use fathomable_core::workspace::Workspace;

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
    if cli.sessions {
        return list_sessions(&dirs);
    }
    if cli.config_show {
        return config_show(&cli, &dirs);
    }

    let id = Id::mint();
    let _guard = match logging::init(&dirs, &id) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("fathomable: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(session = %id, "starting");

    if cli.mcp {
        return match mcp::run(&dirs) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(error = format!("{error:#}"), "mcp failed");
                eprintln!("fathomable: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    if cli.dump_state || cli.replay_log {
        let unimplemented = if cli.dump_state {
            "--dump-state"
        } else {
            "--replay-log"
        };
        tracing::warn!(feature = unimplemented, "not implemented");
        eprintln!("fathomable: {unimplemented} is not implemented yet");
        return ExitCode::FAILURE;
    }

    match run_tui(&cli, &dirs, id) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "failed");
            eprintln!("fathomable: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_tui(cli: &Cli, dirs: &XdgDirs, id: Id) -> anyhow::Result<()> {
    let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    let theme = load_theme(cli, dirs)?;
    tracing::info!(theme = theme.name(), "theme loaded");
    let highlighter = Highlighter::new(theme.syntect())
        .with_context(|| format!("theme `{}`: code.syntect", theme.name()))?;
    tracing::info!(
        syntect = theme.syntect(),
        enabled = highlighter.is_enabled(),
        "highlighter loaded"
    );
    let workspace = Workspace::discover(&path)?;
    let open = path.is_file().then(|| workspace.relative(&path));

    let removed = Record::sweep_dead(dirs);
    if removed > 0 {
        tracing::info!(removed, "swept dead session records");
    }
    let socket = dirs.runtime_dir().map(|dir| dir.join(format!("{id}.sock")));
    let record = Record::new(id, workspace.root().to_path_buf(), socket);
    record.write(dirs)?;
    tracing::info!(id = %record.id(), root = %record.root().display(), "session recorded");

    let config = Config::load(dirs, cli.config.as_deref())?;
    let seen = match fathomable_core::seen::Store::open(&dirs.seen_dir(workspace.root())) {
        Ok(seen) => {
            tracing::info!(path = %seen.dir().display(), files = seen.len(), "last-seen snapshots loaded");
            Some(seen)
        }
        Err(error) => {
            tracing::error!(%error, "cannot open the snapshot store; no last-seen base");
            None
        }
    };
    let store = match Store::open(dirs.threads_file(workspace.root())) {
        Ok(store) => {
            tracing::info!(path = %store.path().display(), threads = store.threads().len(), "threads loaded");
            Some(store)
        }
        Err(error) => {
            tracing::error!(%error, "cannot open the thread store; annotations disabled");
            None
        }
    };
    let result = app::run(
        workspace,
        app::Options {
            open,
            record: &record,
            theme: &theme,
            store,
            follow: config.follow().clone(),
            seen,
            highlighter: Arc::new(highlighter),
            markdown: config.markdown().clone(),
        },
    );
    if let Err(error) = record.remove(dirs) {
        tracing::warn!(%error, "cannot remove session record");
    }
    result
}

/// `--sessions`: one line per record, marking dead ones.
fn list_sessions(dirs: &XdgDirs) -> ExitCode {
    let records = Record::list(dirs);
    if records.is_empty() {
        println!("no sessions");
        return ExitCode::SUCCESS;
    }
    for record in records {
        println!(
            "{}\t{}\t{}\t{}",
            record.id(),
            if record.is_alive() { "live" } else { "dead" },
            record.root().display(),
            record
                .socket()
                .map_or_else(|| "(no socket)".to_owned(), |p| p.display().to_string()),
        );
    }
    ExitCode::SUCCESS
}

/// `--config-show`: the effective settings.
fn config_show(cli: &Cli, dirs: &XdgDirs) -> ExitCode {
    let config = match Config::load(dirs, cli.config.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("fathomable: {error}");
            return ExitCode::FAILURE;
        }
    };
    let theme = cli
        .theme
        .as_deref()
        .or_else(|| config.theme())
        .unwrap_or(DEFAULT_THEME);
    println!(
        "config {}",
        cli.config
            .clone()
            .unwrap_or_else(|| dirs.config_dir().join("config.kdl"))
            .display()
    );
    println!("theme \"{theme}\"");
    let follow = config.follow();
    println!("follow {{");
    println!("    source \"{}\"", follow.source);
    println!("    auto #{}", follow.auto);
    if !follow.ignore.is_empty() {
        let globs: Vec<String> = follow.ignore.iter().map(|g| format!("{g:?}")).collect();
        println!("    ignore {}", globs.join(" "));
    }
    println!("    hint-debounce {}", follow.hint_debounce.as_millis());
    println!("    jump-debounce {}", follow.jump_debounce.as_millis());
    println!("    seen-idle {}", follow.seen_idle.as_millis());
    println!("    toast {}", follow.toast.as_millis());
    println!("}}");
    let markdown = config.markdown();
    let extensions: Vec<String> = markdown
        .extensions
        .iter()
        .map(|e| format!("{e:?}"))
        .collect();
    println!("markdown {{");
    println!("    extensions {}", extensions.join(" "));
    println!("    extensionless #{}", markdown.extensionless);
    println!("}}");
    ExitCode::SUCCESS
}

/// Pick the theme: `--theme`, then `config.kdl`, then the built-in default.
fn load_theme(cli: &Cli, dirs: &XdgDirs) -> anyhow::Result<Theme> {
    let config = Config::load(dirs, cli.config.as_deref())?;
    let name = cli
        .theme
        .as_deref()
        .or_else(|| config.theme())
        .unwrap_or(DEFAULT_THEME);
    Ok(Theme::load(name, dirs)?)
}
