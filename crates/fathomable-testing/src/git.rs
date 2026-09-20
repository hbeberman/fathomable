// @okf-doc: /decisions/0048-modules-by-concept.md
//! Git repositories for tests: init, commit, stage, tag, and amend through
//! `gix`, ignoring `GIT_*` overrides in the environment.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

/// Open fixture repositories at the requested path, ignoring `GIT_*` overrides.
///
/// This matches the workspace's private policy without exposing `gix` in
/// the production core API.
#[must_use]
pub fn open_options() -> gix::open::Options {
    let mut permissions = gix::open::Permissions::default();
    permissions.env.git_prefix = gix::sec::Permission::Deny;
    gix::open::Options::default().permissions(permissions)
}

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

/// Commit and stage one flat path with arbitrary regular-file bytes.
///
/// # Errors
///
/// Returns [`GitError`] when the repository objects or index cannot be written.
pub fn commit_bytes_and_stage(root: &Path, path: &str, bytes: &[u8]) -> Result<(), GitError> {
    commit_single_entry(root, path, bytes, gix::objs::tree::EntryKind::Blob)
}

/// Commit and stage one flat symbolic-link entry without creating the link.
///
/// # Errors
///
/// Returns [`GitError`] when the repository objects or index cannot be written.
pub fn commit_symlink_and_stage(root: &Path, path: &str, target: &str) -> Result<(), GitError> {
    commit_single_entry(
        root,
        path,
        target.as_bytes(),
        gix::objs::tree::EntryKind::Link,
    )
}

fn commit_single_entry(
    root: &Path,
    path: &str,
    bytes: &[u8],
    kind: gix::objs::tree::EntryKind,
) -> Result<(), GitError> {
    if path.is_empty() || path.contains('/') {
        return Err(GitError(
            "single-entry fixture path must be one non-empty component".to_owned(),
        ));
    }
    let repo = git(gix::open_opts(root, open_options()))?;
    let blob = git(repo.write_blob(bytes))?.detach();
    let tree = git(repo.write_object(gix::objs::Tree {
        entries: vec![gix::objs::tree::Entry {
            mode: kind.into(),
            filename: path.into(),
            oid: blob,
        }],
    }))?
    .detach();
    let parent = repo.head_id().ok().map(gix::Id::detach);
    let author = signature("2 +0000");
    git(repo.commit_as(author, author, "HEAD", "raw commit", tree, parent))?;
    let state = git(gix::index::State::from_tree(
        &tree,
        &repo.objects,
        gix::validate::path::component::Options::default(),
    ))?;
    let mut file = gix::index::File::from_state(state, repo.index_path());
    git(file.write(gix::index::write::Options::default()))?;
    Ok(())
}

/// Create a lightweight tag named `name` at `HEAD`.
///
/// # Errors
///
/// Returns [`GitError`] when the repository, `HEAD`, or tag cannot be written.
pub fn tag(root: &Path, name: &str) -> Result<(), GitError> {
    let repo = git(gix::open_opts(root, open_options()))?;
    let head = git(repo.head_id())?.detach();
    git(repo.tag_reference(
        name,
        head,
        gix::refs::transaction::PreviousValue::MustNotExist,
    ))?;
    Ok(())
}

/// Move an existing lightweight tag named `name` to `HEAD`.
///
/// # Errors
///
/// Returns [`GitError`] when the repository, `HEAD`, or tag cannot be updated.
pub fn retag(root: &Path, name: &str) -> Result<(), GitError> {
    let repo = git(gix::open_opts(root, open_options()))?;
    let head = git(repo.head_id())?.detach();
    git(repo.tag_reference(name, head, gix::refs::transaction::PreviousValue::Any))?;
    Ok(())
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
/// The commit names its committer outright, as [`commit_and_stage`]
/// does: the reflog entry needs one, and a fixture repository has no
/// identity configured, nor does every contributor's machine.
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
    let id = git(repo.write_object(&commit))?.detach();
    let edit = gix::refs::transaction::RefEdit {
        change: gix::refs::transaction::Change::Update {
            log: gix::refs::transaction::LogChange {
                mode: gix::refs::transaction::RefLog::AndReference,
                force_create_reflog: false,
                message: "amend".into(),
            },
            expected: gix::refs::transaction::PreviousValue::Any,
            new: gix::refs::Target::Object(id),
        },
        name: git(gix::refs::FullName::try_from("refs/heads/main"))?,
        deref: false,
    };
    git(repo.edit_references_as(Some(edit), Some(author)))?;
    stage(root, files)
}

/// `git worktree add <linked> -b <branch>` at `HEAD`: register the
/// linked worktree under `main`'s `.git/worktrees/`, point `linked/.git`
/// back at it, and check `HEAD`'s tree out into `linked`.
///
/// # Errors
///
/// Returns [`GitError`] when the repository cannot be opened or the
/// worktree cannot be written.
pub fn worktree_add(main: &Path, linked: &Path, branch: &str) -> Result<(), GitError> {
    let repo = git(gix::open_opts(main, open_options()))?;
    let head = git(repo.head_id())?.detach();
    let name = linked
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| GitError("worktree path has no name".to_owned()))?;
    let git_dir = repo.common_dir().join("worktrees").join(&name);
    git(std::fs::create_dir_all(&git_dir))?;
    git(std::fs::create_dir_all(linked))?;
    let linked_abs = git(linked.canonicalize())?;
    git(std::fs::write(
        git_dir.join("gitdir"),
        format!("{}\n", linked_abs.join(".git").display()),
    ))?;
    git(std::fs::write(git_dir.join("commondir"), "../..\n"))?;
    git(std::fs::write(
        git_dir.join("HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    ))?;
    let heads = repo.common_dir().join("refs").join("heads");
    git(std::fs::create_dir_all(&heads))?;
    git(std::fs::write(heads.join(branch), format!("{head}\n")))?;
    git(std::fs::write(
        linked_abs.join(".git"),
        format!("gitdir: {}\n", git_dir.display()),
    ))?;
    // The checkout: every blob of HEAD's tree, at its path.
    let tree = git(repo.head_tree())?;
    let mut recorder = gix::traverse::tree::Recorder::default();
    git(tree.traverse().breadthfirst(&mut recorder))?;
    for entry in recorder.records {
        if !entry.mode.is_blob() {
            continue;
        }
        let blob = git(repo.find_object(entry.oid))?;
        let path: &gix::bstr::BStr = entry.filepath.as_ref();
        let target = linked_abs.join(gix::path::from_bstr(path));
        if let Some(parent) = target.parent() {
            git(std::fs::create_dir_all(parent))?;
        }
        git(std::fs::write(target, &blob.data))?;
    }
    Ok(())
}
