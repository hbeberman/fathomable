// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! `fathomable --doctor`: non-interactive diagnostics printed to stdout.

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use fathomable_core::XdgDirs;
use fathomable_core::config::Config;
use fathomable_core::highlight::{self, Highlighter};
use fathomable_core::session::Record;
use fathomable_core::theme::Theme;
use fathomable_core::workspace::Workspace;

/// Print diagnostics. Exit status is failure when any check fails.
pub(crate) fn run(dirs: &XdgDirs) -> ExitCode {
    let mut ok = true;

    println!("fathomable {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("directories");
    println!("  config   {}", dirs.config_dir().display());
    println!("  state    {}", dirs.state_dir().display());
    println!("  log      {}", dirs.log_dir().display());
    println!("  themes   {}", dirs.themes_dir().display());
    println!("  viewers  {}", dirs.viewers_dir().display());
    match dirs.runtime_dir() {
        Some(runtime) => println!("  runtime  {}", runtime.display()),
        None => {
            println!("  runtime  unset (XDG_RUNTIME_DIR); the viewer socket needs it");
        }
    }

    println!();
    println!("terminal");
    match crossterm::terminal::size() {
        Ok((columns, rows)) => println!("  size     {columns}x{rows}"),
        Err(error) => println!("  size     unavailable ({error})"),
    }

    println!();
    println!("checks");
    match log_dir_writable(&dirs.log_dir()) {
        Ok(()) => println!("  ok    log directory is writable"),
        Err(error) => {
            ok = false;
            println!("  FAIL  log directory is not writable: {error}");
        }
    }

    match crash_reports(&dirs.log_dir()) {
        0 => println!("  ok    no crash reports"),
        n => println!(
            "  ok    {n} crash report{} in {} (`.crash`, newest last)",
            if n == 1 { "" } else { "s" },
            dirs.log_dir().display()
        ),
    }

    let config = match Config::load(dirs, None) {
        Ok(config) => {
            println!("  ok    config parsed");
            config
        }
        Err(error) => {
            ok = false;
            println!("  FAIL  config: {error}");
            Config::default()
        }
    };
    let theme_name = config.theme().to_owned();
    println!(
        "  ok    auto-jump {}",
        if config.jump().auto { "on" } else { "off" }
    );
    match Theme::load(&theme_name, dirs) {
        Ok(theme) => {
            println!("  ok    theme `{}` loaded", theme.name());
            match Highlighter::new(theme.syntect()) {
                Ok(highlighter) if highlighter.is_enabled() => {
                    println!(
                        "  ok    code highlighting with syntect theme `{}`",
                        theme.syntect()
                    );
                }
                Ok(_) => println!("  ok    code highlighting off (code.syntect unset)"),
                Err(error) => {
                    ok = false;
                    println!("  FAIL  {error}");
                }
            }
            println!(
                "  ok    bundled syntect themes: {}",
                highlight::theme_names().join(", ")
            );
        }
        Err(error) => {
            ok = false;
            println!("  FAIL  {error}");
        }
    }

    ok &= workspace_checks(dirs);

    let records = Record::list(dirs);
    let live = records.iter().filter(|record| record.is_alive()).count();
    println!(
        "  ok    {live} live viewer{} ({} record{} on disk)",
        if live == 1 { "" } else { "s" },
        records.len(),
        if records.len() == 1 { "" } else { "s" }
    );

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The git and snapshot checks for the workspace around the cwd.
fn workspace_checks(dirs: &XdgDirs) -> bool {
    let mut ok = true;
    match std::env::current_dir()
        .map_err(|error| error.to_string())
        .and_then(|cwd| Workspace::discover(&cwd).map_err(|error| error.to_string()))
    {
        Ok(workspace) if workspace.is_git() => {
            // Any path will do: the probe reads HEAD, not the file.
            match workspace.head_text(Path::new(".fathomable-doctor-probe")) {
                Ok(_) => println!(
                    "  ok    git work tree at {} (HEAD readable)",
                    workspace.root().display()
                ),
                Err(error) => {
                    ok = false;
                    println!("  FAIL  git: {error}");
                }
            }
        }
        Ok(workspace) => println!(
            "  ok    {} is not a git work tree; no HEAD diff base",
            workspace.root().display()
        ),
        Err(error) => {
            ok = false;
            println!("  FAIL  workspace: {error}");
        }
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(mut workspace) = Workspace::discover(&cwd)
    {
        if workspace.is_git() {
            match workspace.status() {
                Ok(status) => println!(
                    "  ok    {} uncommitted path{} ({} staged)",
                    status.len(),
                    if status.len() == 1 { "" } else { "s" },
                    status.entries().iter().filter(|e| e.is_staged()).count()
                ),
                Err(error) => {
                    ok = false;
                    println!("  FAIL  git status: {error}");
                }
            }
        }
        let dir = dirs.seen_dir(workspace.root());
        // Opening prunes, so pin what the viewer pins (ADR 0020).
        let threads =
            fathomable_core::annotations::Store::open(dirs.threads_file(workspace.root())).ok();
        let pinned = threads
            .iter()
            .flat_map(fathomable_core::annotations::Store::open_paths);
        match fathomable_core::seen::Store::open_pinned(&dir, pinned) {
            Ok(seen) => println!(
                "  ok    {} last-seen snapshot{} ({} bytes) in {}",
                seen.len(),
                if seen.len() == 1 { "" } else { "s" },
                seen.blob_bytes(),
                dir.display()
            ),
            Err(error) => {
                ok = false;
                println!("  FAIL  snapshots: {error}");
            }
        }
        let dir = dirs.checkpoints_dir(workspace.root());
        match fathomable_core::checkpoints::Store::open(&dir) {
            Ok(checkpoints) => println!(
                "  ok    {} checkpoint{} over {} file{} ({} bytes) in {}",
                checkpoints.events(),
                if checkpoints.events() == 1 { "" } else { "s" },
                checkpoints.len(),
                if checkpoints.len() == 1 { "" } else { "s" },
                checkpoints.blob_bytes(),
                dir.display()
            ),
            Err(error) => {
                ok = false;
                println!("  FAIL  checkpoints: {error}");
            }
        }
    }

    ok
}

/// How many crash reports a past run left behind (ADR 0022).
fn crash_reports(log_dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(log_dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "crash"))
        .count()
}

fn log_dir_writable(log_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(log_dir)?;
    let probe = log_dir.join(format!(".doctor-probe-{}", std::process::id()));
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)
}
