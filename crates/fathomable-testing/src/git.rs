//! Git repositories for tests: init, commit, stage, and amend through
//! `gix`, with the workspace's own open options so `GIT_*` overrides in
//! the environment are ignored.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

use fathomable_core::workspace::open_options;

/// A git operation the fixture could not perform, with the cause's text.
#[derive(Debug)]
pub struct GitError(String);

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "test git fixture: {}", self.0)
    }
}

impl Error for GitError {}

/// Any `gix` error becomes a [`GitError`] with its text, so the helpers
/// use `?` across the many error types the crate returns.
fn git<T, E: fmt::Display>(result: Result<T, E>) -> Result<T, GitError> {
    result.map_err(|error| GitError(error.to_string()))
}

/// The signature every fixture commit is made with, at `time`.
fn signature(time: &'static str) -> gix::actor::SignatureRef<'static> {
    gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time,
    }
}

/// `git init` at `dir`, ignoring `GIT_*` overrides as the workspace does.
///
/// # Errors
///
/// Returns [`GitError`] when the repository cannot be created.
pub fn init(dir: &Path) -> Result<(), GitError> {
    git(gix::ThreadSafeRepository::init_opts(
        dir,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        open_options(),
    ))?;
    Ok(())
}

/// Write `files` (root-relative path, content) as a tree object, nested
/// directories and all, and return its id.
///
/// # Errors
///
/// Returns [`GitError`] when an object cannot be written.
pub fn write_tree(
    repo: &gix::Repository,
    files: &[(&str, &str)],
) -> Result<gix::ObjectId, GitError> {
    let mut entries = Vec::new();
    let mut subdirs: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (path, content) in files {
        match path.split_once('/') {
            Some((dir, rest)) => subdirs.entry(dir).or_default().push((rest, content)),
            None => entries.push(gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: (*path).into(),
                oid: git(repo.write_blob(content.as_bytes()))?.detach(),
            }),
        }
    }
    for (dir, files) in subdirs {
        entries.push(gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Tree.into(),
            filename: dir.into(),
            oid: write_tree(repo, &files)?,
        });
    }
    entries.sort();
    Ok(git(repo.write_object(gix::objs::Tree { entries }))?.detach())
}

/// Commit `files` as `HEAD` and stage the same tree, as `git add -A`
/// then `git commit` would leave things.
///
/// # Errors
///
/// Returns [`GitError`] when the repository cannot be opened or written.
pub fn commit_and_stage(root: &Path, files: &[(&str, &str)]) -> Result<(), GitError> {
    let repo = git(gix::open_opts(root, open_options()))?;
    let tree = write_tree(&repo, files)?;
    let parent = repo.head_id().ok().map(gix::Id::detach);
    let author = signature("0 +0000");
    git(repo.commit_as(author, author, "HEAD", "commit", tree, parent))?;
    stage(root, files)
}

/// Replace the index with `files`.
///
/// # Errors
///
/// Returns [`GitError`] when the repository cannot be opened or the index
/// cannot be written.
pub fn stage(root: &Path, files: &[(&str, &str)]) -> Result<(), GitError> {
    let repo = git(gix::open_opts(root, open_options()))?;
    let tree = write_tree(&repo, files)?;
    let state = git(gix::index::State::from_tree(
        &tree,
        &repo.objects,
        gix::validate::path::component::Options::default(),
    ))?;
    let mut file = gix::index::File::from_state(state, repo.index_path());
    git(file.write(gix::index::write::Options::default()))?;
    Ok(())
}

/// Rewrite `refs/heads/main` as a fresh root commit of `files` and stage
/// it: what an amend or a squash leaves behind.
///
/// # Errors
///
/// Returns [`GitError`] when the repository cannot be opened or written.
pub fn amend(root: &Path, files: &[(&str, &str)]) -> Result<(), GitError> {
    let repo = git(gix::open_opts(root, open_options()))?;
    let tree = write_tree(&repo, files)?;
    let author = signature("1 +0000");
    let commit = gix::objs::Commit {
        message: "amended".into(),
        tree,
        author: author.into(),
        committer: author.into(),
        encoding: None,
        parents: std::iter::empty().collect(),
        extra_headers: Vec::default(),
    };
    let id = git(repo.write_object(&commit))?;
    git(repo.reference(
        "refs/heads/main",
        id,
        gix::refs::transaction::PreviousValue::Any,
        "amend",
    ))?;
    stage(root, files)
}
