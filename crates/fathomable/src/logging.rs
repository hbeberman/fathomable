// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! Structured file logging to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`.

use std::fmt;
use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::session::Id;
use tracing_subscriber::EnvFilter;

/// Environment variable that sets the log filter; default `info`.
pub const LOG_ENV: &str = "FATHOMABLE_LOG";

/// Where the log for session `id` is written.
pub fn log_path(dirs: &XdgDirs, id: &Id) -> PathBuf {
    dirs.log_dir().join(format!("{id}.log"))
}

/// Keeps the log file open for the lifetime of the process.
#[must_use = "dropping the guard stops logging"]
pub struct Guard {
    path: PathBuf,
}

impl fmt::Debug for Guard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Guard").field("path", &self.path).finish()
    }
}

/// Install the global JSON-lines subscriber writing to the session log file.
pub fn init(dirs: &XdgDirs, id: &Id) -> anyhow::Result<Guard> {
    let log_dir = dirs.log_dir();
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("cannot create log directory {}", log_dir.display()))?;
    let path = log_path(dirs, id);
    let file = fs::File::options()
        .create_new(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("cannot create log file {}", path.display()))?;
    let filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("cannot install tracing subscriber: {error}"))?;
    Ok(Guard { path })
}
