// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! `fathomable --doctor`: non-interactive diagnostics printed to stdout.

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use fathomable_core::XdgDirs;
use fathomable_core::config::Config;
use fathomable_core::session::Record;
use fathomable_core::theme::{DEFAULT_THEME, Theme};
use fathomable_core::workspace::Workspace;

/// Print diagnostics. Exit status is failure when any check fails.
pub fn run(dirs: &XdgDirs) -> ExitCode {
    let mut ok = true;

    println!("fathomable {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("directories");
    println!("  config   {}", dirs.config_dir().display());
    println!("  state    {}", dirs.state_dir().display());
    println!("  log      {}", dirs.log_dir().display());
    println!("  themes   {}", dirs.themes_dir().display());
    println!("  sessions {}", dirs.sessions_dir().display());
    match dirs.runtime_dir() {
        Some(runtime) => println!("  runtime  {}", runtime.display()),
        None => {
            println!("  runtime  unset (XDG_RUNTIME_DIR); sessions will need it");
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

    let theme_name = match Config::load(dirs, None) {
        Ok(config) => {
            println!("  ok    config parsed");
            config.theme().unwrap_or(DEFAULT_THEME).to_owned()
        }
        Err(error) => {
            ok = false;
            println!("  FAIL  config: {error}");
            DEFAULT_THEME.to_owned()
        }
    };
    match Theme::load(&theme_name, dirs) {
        Ok(theme) => println!("  ok    theme `{}` loaded", theme.name()),
        Err(error) => {
            ok = false;
            println!("  FAIL  {error}");
        }
    }

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
            "  ok    {} is not a git work tree; no diff gutter",
            workspace.root().display()
        ),
        Err(error) => {
            ok = false;
            println!("  FAIL  workspace: {error}");
        }
    }

    let records = Record::list(dirs);
    let live = records.iter().filter(|record| record.is_alive()).count();
    println!(
        "  ok    {live} live session{} ({} record{} on disk)",
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

fn log_dir_writable(log_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(log_dir)?;
    let probe = log_dir.join(format!(".doctor-probe-{}", std::process::id()));
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)
}
