// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! XDG base directories, resolved from the environment with the standard
//! library only (ADR 0001 and 0008).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// The application subdirectory under each XDG base directory.
pub(crate) const APP_DIR: &str = "fathomable";

/// XDG base directories relevant to Fathomable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdgDirs {
    config_home: PathBuf,
    state_home: PathBuf,
}

impl XdgDirs {
    /// Resolve the directories from the current process environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::resolve(|name| std::env::var_os(name))
    }

    /// Resolve the directories from an arbitrary variable lookup.
    ///
    /// `XDG_CONFIG_HOME` and `XDG_STATE_HOME` fall back to `$HOME/.config`
    /// and `$HOME/.local/state` per the XDG Base Directory specification.
    /// Empty values are treated as unset. When `HOME` is also unset the
    /// fallbacks are relative paths.
    pub fn resolve(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let get = |name: &str| {
            lookup(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = get("HOME").unwrap_or_default();
        Self {
            config_home: get("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            state_home: get("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
        }
    }

    /// `$XDG_CONFIG_HOME/fathomable`.
    #[must_use]
    pub fn config_dir(&self) -> PathBuf {
        self.config_home.join(APP_DIR)
    }

    /// `$XDG_CONFIG_HOME/fathomable/themes`, where user theme files live.
    #[must_use]
    pub fn themes_dir(&self) -> PathBuf {
        self.config_dir().join("themes")
    }

    /// `$XDG_STATE_HOME/fathomable`.
    #[must_use]
    pub fn state_dir(&self) -> PathBuf {
        self.state_home.join(APP_DIR)
    }

    /// Create or validate an application-owned directory beneath the state root.
    ///
    /// Every component from `fathomable` through `directory` must be owned by
    /// the effective UID with mode 0700. External ancestors are not modified.
    ///
    /// # Errors
    ///
    /// Refuses paths outside this state root, links, unsafe ownership or modes,
    /// and filesystem errors. Existing state is never repaired or migrated.
    pub fn prepare_state_dir(&self, directory: impl AsRef<Path>) -> io::Result<()> {
        self.visit_state_dirs(directory, |path| crate::private_state::ensure_dir(path))
    }

    pub(crate) fn validate_state_dir(&self, directory: impl AsRef<Path>) -> io::Result<()> {
        self.visit_state_dirs(directory, |path| crate::private_state::validate_dir(path))
    }

    fn visit_state_dirs(
        &self,
        directory: impl AsRef<Path>,
        mut visit: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        let root = self.state_dir();
        let relative = directory.as_ref().strip_prefix(&root).map_err(|_prefix| {
            io::Error::other("directory is outside the Fathomable state root")
        })?;
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(io::Error::other("invalid Fathomable state directory"));
        }
        visit(&root)?;
        let mut path = root;
        for part in relative.components() {
            path.push(part);
            visit(&path)?;
        }
        Ok(())
    }

    /// `$XDG_STATE_HOME/fathomable/log`, where per-session logs are written.
    #[must_use]
    pub fn log_dir(&self) -> PathBuf {
        self.state_dir().join("log")
    }

    /// `$XDG_STATE_HOME/fathomable/viewers`, where viewer records live.
    #[must_use]
    pub fn viewers_dir(&self) -> PathBuf {
        self.state_dir().join("viewers")
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>`, the per-workspace
    /// state directory (ADR 0005); `hash` is the short SHA-256 of `key`,
    /// the workspace's [`key`](crate::workspace::Workspace::key): its git
    /// common dir, shared by every worktree (ADR 0070), or its root.
    #[must_use]
    pub fn workspace_dir(&self, key: &Path) -> PathBuf {
        let hash = crate::annotations::short_hash(key.as_os_str().as_encoded_bytes());
        self.state_dir().join("workspaces").join(hash)
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`.
    #[must_use]
    pub fn threads_file(&self, key: &Path) -> PathBuf {
        self.workspace_dir(key)
            .join(crate::annotations::THREADS_FILE)
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/workspace.json`, the
    /// marker that names the root behind the hash (ADR 0024).
    #[must_use]
    pub(crate) fn workspace_file(&self, key: &Path) -> PathBuf {
        self.workspace_dir(key).join(crate::session::WORKSPACE_FILE)
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/review-points`, where
    /// explicit workspace review points live.
    #[must_use]
    pub fn review_points_dir(&self, key: &Path) -> PathBuf {
        self.workspace_dir(key).join("review-points")
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/comparison`, where
    /// the viewer's checkout-local comparison preference lives.
    #[must_use]
    pub fn comparison_dir(&self, key: &Path) -> PathBuf {
        self.workspace_dir(key).join("comparison")
    }
}
