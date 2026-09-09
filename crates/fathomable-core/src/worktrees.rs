// @okf-doc: /decisions/0070-one-workspace-many-worktrees.md
//! The worktrees of a git workspace and the key they share (ADR 0070).
//!
//! A git workspace is the repository: every checkout that shares one
//! common dir. Its state directory is keyed by that common dir, so every
//! worktree reads and writes one thread store, one register, one set of
//! seen marks and checkpoints. [`Worktree`] is one checkout as the
//! viewer lists it: the main worktree, whose `.git` is the common dir,
//! or a linked one made by `git worktree add`. [`adopt`] moves a state
//! directory keyed by a root under the old rule to its common-dir key,
//! once. The union reach over worktrees lives on
//! [`Reach`](crate::reach::Reach).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One checkout of a workspace (ADR 0070).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    root: PathBuf,
    branch: Option<String>,
    head: Option<String>,
    main: bool,
}

impl Worktree {
    /// The absolute path of the checkout.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The branch `HEAD` is on, or `None` when detached or unborn.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// The `HEAD` commit as hex, or `None` before the first commit.
    #[must_use]
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    /// Whether this is the main worktree, whose `.git` is the common dir.
    #[must_use]
    pub fn is_main(&self) -> bool {
        self.main
    }

    /// The word the viewer shows for the checkout: its branch, else its
    /// short commit when detached, else `unborn`.
    #[must_use]
    pub fn label(&self) -> String {
        match (&self.branch, &self.head) {
            (Some(branch), _) => branch.clone(),
            (None, Some(head)) => head.chars().take(7).collect(),
            (None, None) => "unborn".to_owned(),
        }
    }
}

/// Every worktree of `repo`'s workspace: the main one first when the
/// repository is not bare, then the linked ones in the order git keeps
/// them. A linked worktree whose directory is gone is not listed.
pub(crate) fn list(repo: &gix::Repository) -> Vec<Worktree> {
    let mut out = Vec::new();
    match repo.main_repo() {
        Ok(main) => {
            if let Some(root) = main.workdir() {
                out.push(describe(&main, canonical(root), true));
            }
        }
        Err(error) => tracing::warn!(%error, "cannot open the main worktree"),
    }
    let linked = match repo.worktrees() {
        Ok(linked) => linked,
        Err(error) => {
            tracing::warn!(%error, "cannot list linked worktrees");
            return out;
        }
    };
    for proxy in linked {
        let Ok(base) = proxy.base() else {
            continue;
        };
        if !base.is_dir() {
            continue;
        }
        let root = canonical(&base);
        if out.iter().any(|w| w.root == root) {
            continue;
        }
        match proxy.into_repo_with_possibly_inaccessible_worktree() {
            Ok(linked) => out.push(describe(&linked, root, false)),
            Err(error) => tracing::warn!(%error, root = %root.display(), "cannot open worktree"),
        }
    }
    out
}

fn describe(repo: &gix::Repository, root: PathBuf, main: bool) -> Worktree {
    let branch = repo
        .head_name()
        .ok()
        .flatten()
        .map(|name| name.shorten().to_string());
    let head = repo.head_id().ok().map(|id| id.to_hex().to_string());
    Worktree {
        root,
        branch,
        head,
        main,
    }
}

/// `path` canonicalised, or as given when it cannot be.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The path `.git/worktrees` under `common_dir`, where linked worktrees
/// are registered; the viewer watches it for the set changing.
#[must_use]
pub fn registry(common_dir: &Path) -> PathBuf {
    common_dir.join("worktrees")
}

/// Move the state directory keyed by `root` under the old rule to the
/// one keyed by `key`, when the old exists and the new does not
/// (ADR 0070). Returns whether a move happened. A plain workspace, whose
/// key is its root, has nothing to move.
///
/// # Errors
///
/// Returns the I/O error when the rename fails.
pub fn adopt(dirs: &crate::XdgDirs, root: &Path, key: &Path) -> io::Result<bool> {
    if root == key {
        return Ok(false);
    }
    let old = dirs.workspace_dir(root);
    let new = dirs.workspace_dir(key);
    if !old.is_dir() || new.exists() {
        return Ok(false);
    }
    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&old, &new)?;
    tracing::info!(from = %old.display(), to = %new.display(), "workspace state moved to its common-dir key");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::XdgDirs;
    use crate::workspace::Workspace;
    use fathomable_testing::TempDir;
    use fathomable_testing::git;

    fn dirs(dir: &TempDir) -> XdgDirs {
        let base = dir.0.join("xdg");
        XdgDirs::resolve(|name| match name {
            "XDG_STATE_HOME" => Some(base.join("state").into()),
            "XDG_CONFIG_HOME" => Some(base.join("config").into()),
            "XDG_RUNTIME_DIR" => Some(base.join("run").into()),
            _ => None,
        })
    }

    #[test]
    fn a_plain_directory_is_one_worktree_keyed_by_its_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("worktrees-plain")?;
        let root = dir.0.join("ws");
        fs::create_dir_all(&root)?;
        let workspace = Workspace::discover(&root)?;
        assert_eq!(workspace.key(), workspace.root());
        assert!(workspace.worktrees().is_empty());
        Ok(())
    }

    #[test]
    fn linked_worktrees_share_the_main_key() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("worktrees-linked")?;
        let main = dir.0.join("main");
        fs::create_dir_all(&main)?;
        git::init(&main)?;
        git::commit_and_stage(&main, &[("a.md", "one\n")])?;
        let linked = dir.0.join("feature");
        git::worktree_add(&main, &linked, "feature")?;

        let host = Workspace::discover(&main)?;
        let guest = Workspace::discover(&linked)?;
        assert_eq!(host.key(), guest.key(), "one key per repository");
        assert_ne!(host.root(), guest.root());

        let listed = host.worktrees();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert!(listed[0].is_main());
        assert_eq!(listed[0].root(), host.root());
        assert!(!listed[1].is_main());
        assert_eq!(listed[1].root(), guest.root());
        assert_eq!(listed[1].label(), "feature");
        assert_eq!(
            listed[0].head(),
            listed[1].head(),
            "both at the same commit"
        );
        assert_eq!(
            guest.worktrees(),
            listed,
            "the list is the same from either side"
        );
        Ok(())
    }

    #[test]
    fn a_removed_worktree_is_not_listed() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("worktrees-removed")?;
        let main = dir.0.join("main");
        fs::create_dir_all(&main)?;
        git::init(&main)?;
        git::commit_and_stage(&main, &[("a.md", "one\n")])?;
        let linked = dir.0.join("gone");
        git::worktree_add(&main, &linked, "gone")?;
        fs::remove_dir_all(&linked)?;
        let host = Workspace::discover(&main)?;
        assert_eq!(host.worktrees().len(), 1);
        Ok(())
    }

    #[test]
    fn adopt_moves_a_root_keyed_store_once() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("worktrees-adopt")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("main");
        fs::create_dir_all(&root)?;
        git::init(&root)?;
        let workspace = Workspace::discover(&root)?;
        let old = dirs.workspace_dir(workspace.root());
        fs::create_dir_all(&old)?;
        fs::write(old.join("threads.jsonl"), "")?;

        assert!(adopt(&dirs, workspace.root(), workspace.key())?);
        assert!(!old.exists());
        assert!(
            dirs.workspace_dir(workspace.key())
                .join("threads.jsonl")
                .is_file()
        );
        assert!(
            !adopt(&dirs, workspace.root(), workspace.key())?,
            "nothing left to move"
        );
        Ok(())
    }
}
