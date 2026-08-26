// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! `fathomable --doctor`: non-interactive diagnostics printed to stdout.

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use fathomable_core::XdgDirs;

/// Print diagnostics. Exit status is failure when any check fails.
pub fn run(dirs: &XdgDirs) -> ExitCode {
    let mut ok = true;

    println!("fathomable {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("directories");
    println!("  config   {}", dirs.config_dir().display());
    println!("  state    {}", dirs.state_dir().display());
    println!("  log      {}", dirs.log_dir().display());
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
