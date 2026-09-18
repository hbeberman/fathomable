// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! Structured file logging to `$XDG_STATE_HOME/fathomable/log/<session-id>.log`.

use std::fmt;
use std::path::PathBuf;

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::session::Id;
use tracing_subscriber::EnvFilter;

/// Environment variable that sets the log filter; default `info`.
pub(crate) const LOG_ENV: &str = "FATHOMABLE_LOG";

/// Where the log for session `id` is written.
pub(crate) fn log_path(dirs: &XdgDirs, id: &Id) -> PathBuf {
    dirs.log_dir().join(format!("{id}.log"))
}

/// Where the crash report for session `id` is written (ADR 0022).
pub(crate) fn crash_path(dirs: &XdgDirs, id: &Id) -> PathBuf {
    dirs.log_dir().join(format!("{id}.crash"))
}

/// Keeps the log file open for the lifetime of the process.
#[must_use = "dropping the guard stops logging"]
pub(crate) struct Guard {
    path: PathBuf,
    shared_state_ancestor_count: usize,
}

impl fmt::Debug for Guard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Guard")
            .field("path", &self.path)
            .field(
                "shared_state_ancestor_count",
                &self.shared_state_ancestor_count,
            )
            .finish()
    }
}

impl Guard {
    pub(crate) const fn shared_state_ancestor_count(&self) -> usize {
        self.shared_state_ancestor_count
    }
}

/// Install the global JSON-lines subscriber writing to the session log file.
pub(crate) fn init(dirs: &XdgDirs, id: &Id) -> anyhow::Result<Guard> {
    let log_dir = dirs.log_dir();
    dirs.prepare_state_dir(&log_dir)
        .with_context(|| format!("cannot create log directory {}", log_dir.display()))?;
    let path = log_path(dirs, id);
    let file = fathomable_core::private_state::create_new(&path)
        .with_context(|| format!("cannot create log file {}", path.display()))?;
    let shared_state_ancestors = dirs
        .shared_state_ancestors()
        .context("cannot inspect state directory ancestors")?;
    let filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .try_init()
        .map_err(|error| anyhow::anyhow!("cannot install tracing subscriber: {error}"))?;
    for ancestor in &shared_state_ancestors {
        tracing::warn!(
            path = %ancestor.display(),
            recommendation = "remove group write (chmod g-w) or choose a private XDG_STATE_HOME",
            "state ancestor is group-writable"
        );
    }
    Ok(Guard {
        path,
        shared_state_ancestor_count: shared_state_ancestors.len(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::process::Command;

    use fathomable_core::XdgDirs;
    use fathomable_core::session::Id;
    use fathomable_testing::TempDir;

    use super::init;

    const CHILD: &str = "FATHOMABLE_SHARED_STATE_LOGGING_CHILD";
    const STATE_HOME: &str = "FATHOMABLE_SHARED_STATE_LOGGING_HOME";

    #[test]
    fn group_shared_state_allows_logging_and_records_warning() -> anyhow::Result<()> {
        if std::env::var_os(CHILD).is_some() {
            let state_home = std::env::var_os(STATE_HOME)
                .ok_or_else(|| anyhow::anyhow!("missing child state home"))?;
            let dirs =
                XdgDirs::resolve(|name| (name == "XDG_STATE_HOME").then(|| state_home.clone()));
            let guard = init(&dirs, &Id::mint())?;
            assert_eq!(guard.shared_state_ancestor_count(), 1);
            tracing::info!("logging child complete");
            return Ok(());
        }

        for permissions in [0o770, 0o775] {
            let fixture = TempDir::new(&format!("logging-shared-state-{permissions:o}"))?;
            fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755))?;
            let state_home = fixture.0.join("shared-state");
            fs::create_dir(&state_home)?;
            fs::set_permissions(&state_home, fs::Permissions::from_mode(permissions))?;
            let output = Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "logging::tests::group_shared_state_allows_logging_and_records_warning",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env(STATE_HOME, &state_home)
                .output()?;
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );

            let app = state_home.join("fathomable");
            let log_dir = app.join("log");
            assert_eq!(
                fs::symlink_metadata(&state_home)?.mode() & 0o7777,
                permissions
            );
            assert_eq!(fs::symlink_metadata(&app)?.mode() & 0o7777, 0o700);
            assert_eq!(fs::symlink_metadata(&log_dir)?.mode() & 0o7777, 0o700);
            let logs = fs::read_dir(log_dir)?.collect::<std::io::Result<Vec<_>>>()?;
            assert_eq!(logs.len(), 1);
            let log = logs[0].path();
            assert_eq!(fs::symlink_metadata(&log)?.mode() & 0o7777, 0o600);
            let text = fs::read_to_string(log)?;
            assert!(text.contains("state ancestor is group-writable"));
            assert!(text.contains(&state_home.display().to_string()));
        }
        Ok(())
    }
}
