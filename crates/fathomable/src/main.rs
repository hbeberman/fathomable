// @okf-doc: /decisions/0009-cli-and-diagnostics.md
#![forbid(unsafe_code)]
//! Fathomable binary: terminal UI, MCP server, and admin flags (ADR 0009).

mod app;
mod caller;
mod crash;
mod doctor;
mod logging;
mod mcp;

use std::num::{NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::Store;
use fathomable_core::config::Config;
use fathomable_core::highlight::Highlighter;
use fathomable_core::session::{Id, Marker, Record};
use fathomable_core::theme::Theme;
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

    /// Run the stdio MCP server bound to an optional repository directory.
    #[arg(long)]
    mcp: bool,

    /// Let MCP tools select a project root with their `workspace` parameter.
    #[arg(long, requires = "mcp")]
    allow_mutable_mcp_root: bool,

    /// Override the configuration file.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Select a theme from the config themes directory for this run.
    #[arg(long, value_name = "NAME")]
    theme: Option<String>,

    /// Maximum entries considered during workspace discovery.
    #[arg(long, value_name = "COUNT")]
    discovery_entries: Option<NonZeroUsize>,

    /// Maximum filesystem watches installed for one workspace.
    #[arg(long, value_name = "COUNT")]
    workspace_watches: Option<NonZeroUsize>,

    /// Maximum paths retained for workspace presentation.
    #[arg(long, value_name = "COUNT")]
    retained_paths: Option<NonZeroUsize>,

    /// Maximum paths included in one comparison.
    #[arg(long, value_name = "COUNT")]
    comparison_paths: Option<NonZeroUsize>,

    /// Maximum source bytes included in one comparison.
    #[arg(long, value_name = "BYTES")]
    comparison_bytes: Option<NonZeroU64>,

    /// Maximum filesystem events waiting to be processed.
    #[arg(long, value_name = "COUNT")]
    pending_events: Option<NonZeroUsize>,

    /// Print terminal, directory, and log diagnostics and exit.
    #[arg(long)]
    doctor: bool,

    /// List known workspaces and their viewer records.
    #[arg(long)]
    viewers: bool,

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
    if cli.viewers {
        return list_viewers(&dirs);
    }
    if cli.config_show {
        return config_show(&cli, &dirs);
    }
    let id = Id::mint();
    let guard = match logging::init(&dirs, &id) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("fathomable: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(session = %id, "starting");

    if cli.mcp {
        let root_mode = if cli.allow_mutable_mcp_root {
            mcp::RootMode::PerCall
        } else {
            mcp::RootMode::Fixed
        };
        return match mcp::run(&dirs, cli.path.as_deref(), root_mode) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(error = format!("{error:#}"), "mcp failed");
                eprintln!("fathomable: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    // Only the TUI needs the hook: it is the one that dies behind the
    // alternate screen, where the default message is never seen (ADR 0022).
    crash::arm(
        logging::crash_path(&dirs, &id),
        logging::log_path(&dirs, &id),
    );
    match run_tui(&cli, &dirs, id, guard.shared_state_ancestor_count()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "failed");
            crash::fatal(&error);
            ExitCode::FAILURE
        }
    }
}

fn run_tui(
    cli: &Cli,
    dirs: &XdgDirs,
    id: Id,
    shared_state_ancestor_count: usize,
) -> anyhow::Result<()> {
    let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    let config = load_config(cli, dirs)?;
    let theme = Theme::load(config.theme(), dirs)?;
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
    // The state is keyed by the git common dir, shared by every worktree
    // (ADR 0070).
    let key = workspace.key().to_path_buf();
    let roots = worktree_roots(&workspace)?;
    let record = Record::new(id, key.clone(), workspace.root().to_path_buf());
    record.write(dirs)?;
    tracing::info!(id = %record.id(), root = %record.root().display(), "viewer recorded");
    let marker = Marker::new(key.clone(), roots);
    if let Err(error) = marker.write(dirs) {
        tracing::warn!(%error, "cannot write the workspace marker");
    }

    let (store, thread_store_error) = match Store::open_workspace(dirs, &key) {
        Ok(store) => {
            tracing::info!(path = %store.path().display(), threads = store.threads().len(), "threads loaded");
            (Some(store), None)
        }
        Err(error) => {
            tracing::error!(%error, "cannot open the thread store; annotations disabled");
            (None, Some(error))
        }
    };
    let review_points = match fathomable_core::review_points::ReviewPointStore::open_workspace(
        dirs, &key,
    ) {
        Ok(review_points) => {
            tracing::info!(
                path = %review_points.dir().display(),
                points = review_points.len(),
                "review points loaded"
            );
            Some(review_points)
        }
        Err(error) => {
            tracing::error!(%error, "cannot open the review-point store; review points disabled");
            None
        }
    };
    let result = app::run::run(
        workspace,
        app::Options {
            limits: config.limits().clone(),
            record: record.clone(),
            dirs: dirs.clone(),
            store,
            thread_store_error,
            watch: config.watch().clone(),
            review_points,
            highlighter: Arc::new(highlighter),
            markdown: config.markdown().clone(),
            viewer: config.viewer().clone(),
            sidebar: config.layout().sidebar.clone(),
            menu_bar: config.layout().menu_bar,
            threads: config.threads().clone(),
            diff: config.diff().clone(),
            user: config.user().clone(),
            config_path: config_path(cli, dirs),
            shared_state_ancestor_count,
        },
        &theme,
        open.as_deref(),
    );
    if let Err(error) = record.remove(dirs) {
        tracing::warn!(%error, "cannot remove session record");
    }
    result
}

/// Every worktree root of `workspace`, or its root alone outside git
/// (ADR 0070).
fn worktree_roots(workspace: &Workspace) -> anyhow::Result<Vec<PathBuf>> {
    let roots: Vec<PathBuf> = workspace
        .worktrees()?
        .iter()
        .map(|w| w.root().to_path_buf())
        .collect();
    Ok(if roots.is_empty() {
        vec![workspace.root().to_path_buf()]
    } else {
        roots
    })
}

/// `--viewers`: one block per known workspace, its worktrees under it
/// when there are several (ADR 0070), then its viewer records, marking
/// dead ones (ADR 0024).
fn list_viewers(dirs: &XdgDirs) -> ExitCode {
    let markers = Marker::list(dirs);
    let records = Record::list(dirs);
    if markers.is_empty() && records.is_empty() {
        println!("no workspaces");
        return ExitCode::SUCCESS;
    }
    let mut workspaces: Vec<(PathBuf, Vec<PathBuf>)> = markers
        .iter()
        .map(|m| (m.key().to_path_buf(), m.roots().to_vec()))
        .collect();
    for record in &records {
        match workspaces.iter_mut().find(|(key, _)| key == record.key()) {
            Some((_, roots)) => {
                if !roots.iter().any(|root| root == record.root()) {
                    roots.push(record.root().to_path_buf());
                }
            }
            None => workspaces.push((
                record.key().to_path_buf(),
                vec![record.root().to_path_buf()],
            )),
        }
    }
    for (key, roots) in workspaces {
        match roots.first() {
            Some(first) if roots.len() == 1 => println!("{}", first.display()),
            _ => {
                println!("{}", key.display());
                for root in &roots {
                    println!("  worktree {}", root.display());
                }
            }
        }
        let mut any = false;
        for record in records.iter().filter(|r| r.key() == key) {
            any = true;
            let on = if roots.len() > 1 {
                format!("\ton {}", record.root().display())
            } else {
                String::new()
            };
            println!(
                "  {}\t{}{on}",
                record.id(),
                if record.is_alive() { "live" } else { "dead" },
            );
        }
        if !any {
            println!("  (no viewers)");
        }
    }
    ExitCode::SUCCESS
}

/// `--config-show`: the effective settings.
fn config_show(cli: &Cli, dirs: &XdgDirs) -> ExitCode {
    let config = match load_config(cli, dirs) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("fathomable: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("config {}", config_path(cli, dirs).display());
    print!("{config}");
    ExitCode::SUCCESS
}

/// The config file in use: `--config`, else the XDG one.
fn config_path(cli: &Cli, dirs: &XdgDirs) -> PathBuf {
    cli.config
        .clone()
        .unwrap_or_else(|| dirs.config_dir().join("config.kdl"))
}

/// Load the file and apply every command-line configuration override.
fn load_config(cli: &Cli, dirs: &XdgDirs) -> Result<Config, fathomable_core::config::ConfigError> {
    let mut config = Config::load(dirs, cli.config.as_deref())?;
    if let Some(theme) = &cli.theme {
        config.set_theme(theme);
    }
    let mut limits = config.limits().clone();
    if let Some(value) = cli.discovery_entries {
        limits.discovery_entries = value.get();
    }
    if let Some(value) = cli.workspace_watches {
        limits.workspace_watches = value.get();
    }
    if let Some(value) = cli.retained_paths {
        limits.retained_paths = value.get();
    }
    if let Some(value) = cli.comparison_paths {
        limits.comparison_paths = value.get();
    }
    if let Some(value) = cli.comparison_bytes {
        limits.comparison_bytes = value.get();
    }
    if let Some(value) = cli.pending_events {
        limits.pending_events = value.get();
    }
    config.set_limits(limits)?;
    Ok(config)
}
