// @okf-doc: /decisions/0070-one-workspace-many-worktrees.md
//! The worktrees of a git workspace and the key they share (ADR 0070).
//!
//! A git workspace is the repository: every checkout that shares one
//! common dir. Its state directory is keyed by that common dir, so every
//! worktree reads and writes one thread store and one set of explicit
//! review points. [`Worktree`] is one checkout as the
//! viewer lists it: the main worktree, whose `.git` is the common dir,
//! or a linked one made by `git worktree add`. The union reach over
//! worktrees lives on [`Reach`](crate::reach::Reach).

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// One checkout of a workspace (ADR 0070).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    root: PathBuf,
    branch: Option<String>,
    head: Option<String>,
    main: bool,
    lock_reason: Option<String>,
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

    /// The lock reason, or `None` when unlocked; an empty reason still means locked.
    #[must_use]
    pub fn lock_reason(&self) -> Option<&str> {
        self.lock_reason.as_deref()
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
pub(crate) fn list(repo: &gix::Repository) -> io::Result<Vec<Worktree>> {
    let mut out = Vec::new();
    if let Some(root) = repo.workdir()
        && directory_available(root)?
    {
        out.push(describe(repo, canonical(root), true, None));
    }
    let linked = repo.worktrees()?;
    for proxy in linked {
        let base = proxy.base()?;
        if !directory_available(&base)? {
            continue;
        }
        let root = canonical(&base);
        if out.iter().any(|w| w.root == root) {
            continue;
        }
        let lock_reason = read_lock_reason(&proxy.git_dir().join("locked"))?;
        let linked = proxy
            .into_repo_with_possibly_inaccessible_worktree()
            .map_err(io::Error::other)?;
        out.push(describe(&linked, root, false, lock_reason));
    }
    Ok(out)
}

fn directory_available(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_lock_reason(path: &Path) -> io::Result<Option<String>> {
    // Lock reasons are display metadata, not an unbounded source document.
    const MAX_REASON_BYTES: u16 = 4096;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(u64::from(MAX_REASON_BYTES) + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > usize::from(MAX_REASON_BYTES) {
        return Err(io::Error::other("worktree lock reason exceeds 4096 bytes"));
    }
    Ok(Some(String::from_utf8_lossy(&bytes).trim().to_owned()))
}

fn describe(
    repo: &gix::Repository,
    root: PathBuf,
    main: bool,
    lock_reason: Option<String>,
) -> Worktree {
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
        lock_reason,
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

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::workspace::Workspace;
    use fathomable_testing::TempDir;
    use fathomable_testing::git;

    #[test]
    fn a_plain_directory_is_one_worktree_keyed_by_its_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("worktrees-plain")?;
        let root = dir.0.join("ws");
        fs::create_dir_all(&root)?;
        let workspace = Workspace::discover(&root)?;
        assert_eq!(workspace.key(), workspace.root());
        assert!(workspace.worktrees()?.is_empty());
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
        assert_ne!(host.key(), host.root(), "git state uses the common-dir key");
        assert_eq!(host.name(), "main");
        assert_eq!(
            guest.name(),
            host.name(),
            "repository name is checkout-independent"
        );

        let listed = host.worktrees()?;
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
            guest.worktrees()?,
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
        assert_eq!(host.worktrees()?.len(), 1);
        Ok(())
    }

    #[test]
    fn worktree_locks_include_empty_reasons_and_refresh() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("worktrees-locked")?;
        let main = dir.0.join("main");
        fs::create_dir_all(&main)?;
        git::init(&main)?;
        git::commit_and_stage(&main, &[("a.md", "one\n")])?;
        let linked = dir.0.join("feature");
        git::worktree_add(&main, &linked, "feature")?;
        let workspace = Workspace::discover(&main)?;
        let lock = main.join(".git/worktrees/feature/locked");
        fs::write(&lock, "synthetic lock\n")?;
        assert_eq!(
            workspace.worktrees()?[1].lock_reason(),
            Some("synthetic lock")
        );
        fs::write(&lock, "")?;
        assert_eq!(workspace.worktrees()?[1].lock_reason(), Some(""));
        fs::remove_file(&lock)?;
        assert_eq!(workspace.worktrees()?[1].lock_reason(), None);
        fs::write(&lock, "x".repeat(4097))?;
        let error = workspace
            .worktrees()
            .err()
            .ok_or("oversized metadata must fail discovery")?;
        assert!(error.to_string().contains("lock reason exceeds 4096 bytes"));
        Ok(())
    }

    #[test]
    fn removed_active_worktree_still_discovers_surviving_checkouts()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("worktrees-active-removed")?;
        let main = dir.0.join("main");
        fs::create_dir_all(&main)?;
        git::init(&main)?;
        git::commit_and_stage(&main, &[("a.md", "one\n")])?;
        let linked = dir.0.join("feature");
        git::worktree_add(&main, &linked, "feature")?;
        let workspace = Workspace::discover(&linked)?;
        fs::remove_dir_all(&linked)?;
        fs::remove_dir_all(main.join(".git/worktrees/feature"))?;

        let listed = workspace.worktrees()?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].root(), main.canonicalize()?);
        assert_eq!(workspace.name(), "main");
        assert!(
            workspace
                .worktree_watch_paths()
                .contains(&main.join(".git").canonicalize()?)
        );
        Ok(())
    }

    #[test]
    fn unreadable_worktree_registry_is_not_an_empty_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("worktrees-registry-failure")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("a.md", "one\n")])?;
        let workspace = Workspace::discover(&dir.0)?;
        fs::write(dir.0.join(".git/worktrees"), "not a registry directory")?;
        let error = workspace
            .worktrees()
            .err()
            .ok_or("an unreadable registry must fail discovery")?;
        assert!(error.to_string().contains("cannot list worktrees"));
        Ok(())
    }
}
