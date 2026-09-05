// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! XDG base directories, resolved from the environment with the standard
//! library only (ADR 0001 and 0008).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The application subdirectory under each XDG base directory.
pub const APP_DIR: &str = "fathomable";

/// XDG base directories relevant to Fathomable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdgDirs {
    config_home: PathBuf,
    state_home: PathBuf,
    runtime_dir: Option<PathBuf>,
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
    /// `XDG_RUNTIME_DIR` has no fallback and is `None` when unset. Empty
    /// values are treated as unset. When `HOME` is also unset the fallbacks
    /// are relative paths.
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
            runtime_dir: get("XDG_RUNTIME_DIR"),
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
    /// state directory (ADR 0005); `hash` is the short SHA-256 of `root`.
    #[must_use]
    pub fn workspace_dir(&self, root: &Path) -> PathBuf {
        let hash = crate::annotations::short_hash(root.as_os_str().as_encoded_bytes());
        self.state_dir().join("workspaces").join(hash)
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/threads.jsonl`.
    #[must_use]
    pub fn threads_file(&self, root: &Path) -> PathBuf {
        self.workspace_dir(root)
            .join(crate::annotations::THREADS_FILE)
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/agents.jsonl`, the
    /// agent register (ADR 0040).
    #[must_use]
    pub fn agents_file(&self, root: &Path) -> PathBuf {
        self.workspace_dir(root).join(crate::agents::AGENTS_FILE)
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/workspace.json`, the
    /// marker that names the root behind the hash (ADR 0024).
    #[must_use]
    pub fn workspace_file(&self, root: &Path) -> PathBuf {
        self.workspace_dir(root)
            .join(crate::session::WORKSPACE_FILE)
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/seen`, where last-seen
    /// snapshots live (ADR 0015).
    #[must_use]
    pub fn seen_dir(&self, root: &Path) -> PathBuf {
        self.workspace_dir(root).join("seen")
    }

    /// `$XDG_STATE_HOME/fathomable/workspaces/<hash>/checkpoints`, where the
    /// reader's checkpoints live (ADR 0049).
    #[must_use]
    pub fn checkpoints_dir(&self, root: &Path) -> PathBuf {
        self.workspace_dir(root).join("checkpoints")
    }

    /// `$XDG_RUNTIME_DIR/fathomable`, or `None` when the runtime dir is unset.
    #[must_use]
    pub fn runtime_dir(&self) -> Option<PathBuf> {
        self.runtime_dir.as_ref().map(|dir| dir.join(APP_DIR))
    }

    /// `$XDG_RUNTIME_DIR/fathomable/<hash>/<pid>.sock`, one viewer's socket
    /// under its workspace (ADR 0024); `None` when the runtime dir is unset.
    #[must_use]
    pub fn viewer_socket(&self, root: &Path, pid: u32) -> Option<PathBuf> {
        let hash = crate::annotations::short_hash(root.as_os_str().as_encoded_bytes());
        self.runtime_dir()
            .map(|dir| dir.join(hash).join(format!("{pid}.sock")))
    }
}
