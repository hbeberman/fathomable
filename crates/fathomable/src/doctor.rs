// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! `fathomable --doctor`: non-interactive diagnostics printed to stdout.

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use fathomable_core::XdgDirs;
use fathomable_core::config::Config;
use fathomable_core::theme::{DEFAULT_THEME, Theme};

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
