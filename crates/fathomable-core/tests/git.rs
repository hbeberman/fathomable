//! Behaviour of the `HEAD` diff base (ADR 0006) against a real repository
//! built with `gix`, so the tests need no host `git`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::workspace::{Workspace, open_options};

type TestResult = Result<(), Box<dyn Error>>;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> std::io::Result<Self> {
        let dir =
            std::env::temp_dir().join(format!("fathomable-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `git init` that ignores `GIT_*` overrides, as the workspace does.
fn init(dir: &Path) -> Result<(), Box<dyn Error>> {
    gix::ThreadSafeRepository::init_opts(
        dir,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        open_options(),
    )?;
    Ok(())
}

/// Commit `files` (root-relative path, content) as the only tree of `HEAD`.
fn commit(root: &Path, files: &[(&str, &str)]) -> Result<(), Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let mut entries = Vec::new();
    for (name, content) in files {
        let oid = repo.write_blob(content.as_bytes())?.detach();
        entries.push(gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: (*name).into(),
            oid,
        });
    }
    entries.sort();
    let tree = repo.write_object(gix::objs::Tree { entries })?.detach();
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "0 +0000",
    };
    let parent = repo.head_id().ok().map(gix::Id::detach);
    repo.commit_as(signature, signature, "HEAD", "commit", tree, parent)?;
    Ok(())
}

#[test]
fn plain_directory_has_no_diff_base() -> TestResult {
    let dir = TempDir::new("plain")?;
    fs::write(dir.0.join("a.md"), "x\n")?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(!workspace.is_git());
    assert_eq!(workspace.head_text(Path::new("a.md"))?, None);
    Ok(())
}

#[test]
fn unborn_head_and_untracked_files_have_an_empty_base() -> TestResult {
    let dir = TempDir::new("unborn")?;
    init(&dir.0)?;
    fs::write(dir.0.join("a.md"), "x\n")?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(workspace.is_git());
    assert_eq!(
        workspace.head_text(Path::new("a.md"))?,
        Some(String::new()),
        "unborn HEAD"
    );
    commit(&dir.0, &[("other.md", "o\n")])?;
    let workspace = Workspace::discover(&dir.0)?;
    assert_eq!(
        workspace.head_text(Path::new("a.md"))?,
        Some(String::new()),
        "not in HEAD"
    );
    Ok(())
}

#[test]
fn head_text_is_the_committed_content() -> TestResult {
    let dir = TempDir::new("head")?;
    init(&dir.0)?;
    fs::create_dir_all(dir.0.join("docs"))?;
    // Nested paths go through a subtree; build it explicitly.
    let repo = gix::open_opts(&dir.0, open_options())?;
    let readme = repo.write_blob(b"# One\n")?.detach();
    let guide = repo.write_blob(b"# Guide\n\nold\n")?.detach();
    let docs = repo
        .write_object(gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "guide.md".into(),
                oid: guide,
            }],
        })?
        .detach();
    let mut entries = vec![
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: "README.md".into(),
            oid: readme,
        },
        gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Tree.into(),
            filename: "docs".into(),
            oid: docs,
        },
    ];
    entries.sort();
    let tree = repo.write_object(gix::objs::Tree { entries })?.detach();
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "0 +0000",
    };
    let parent = repo.head_id().ok().map(gix::Id::detach);
    repo.commit_as(signature, signature, "HEAD", "commit", tree, parent)?;
    drop(repo);

    // The working tree differs; HEAD is what the base reports.
    fs::write(dir.0.join("README.md"), "# One\n\nchanged\n")?;
    fs::write(dir.0.join("docs/guide.md"), "# Guide\n\nnew\n")?;
    let workspace = Workspace::discover(dir.0.join("docs"))?;
    assert_eq!(
        workspace.head_text(Path::new("README.md"))?.as_deref(),
        Some("# One\n")
    );
    assert_eq!(
        workspace.head_text(Path::new("docs/guide.md"))?.as_deref(),
        Some("# Guide\n\nold\n")
    );
    assert!(
        workspace.head_text(Path::new("docs")).is_err(),
        "a tree is not a diff base"
    );
    Ok(())
}

/// Write an index holding exactly `files`, as `git add` of them would.
fn stage(root: &Path, files: &[(&str, &str)]) -> Result<(), Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let mut entries = Vec::new();
    for (name, content) in files {
        let oid = repo.write_blob(content.as_bytes())?.detach();
        entries.push(gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: (*name).into(),
            oid,
        });
    }
    entries.sort();
    let tree = repo.write_object(gix::objs::Tree { entries })?.detach();
    let state = gix::index::State::from_tree(
        &tree,
        &repo.objects,
        gix::validate::path::component::Options::default(),
    )
    .map_err(|e| format!("from_tree: {e}"))?;
    let mut file = gix::index::File::from_state(state, repo.index_path());
    file.write(gix::index::write::Options::default())
        .map_err(|e| format!("index write: {e}"))?;
    Ok(())
}

#[test]
fn status_tells_staged_unstaged_and_untracked_apart() -> TestResult {
    use fathomable_core::status::State;

    let dir = TempDir::new("status")?;
    init(&dir.0)?;
    let mut workspace = Workspace::discover(&dir.0)?;
    assert!(workspace.status()?.is_empty(), "empty repo is clean");

    let head = [
        ("clean.md", "c\n"),
        ("edited.md", "one\ntwo\n"),
        ("gone.md", "g\n"),
        ("staged.md", "s\n"),
    ];
    commit(&dir.0, &head)?;
    // The index holds HEAD plus a staged edit and a staged new file.
    stage(
        &dir.0,
        &[
            ("clean.md", "c\n"),
            ("edited.md", "one\ntwo\n"),
            ("gone.md", "g\n"),
            ("staged.md", "s2\n"),
            ("new.md", "n\n"),
        ],
    )?;
    for (name, content) in [
        ("clean.md", "c\n"),
        ("edited.md", "one\nthree\nfour\n"),
        ("staged.md", "s2\n"),
        ("new.md", "n\n"),
        ("untracked.md", "u\nu\n"),
        (".gitignore", "ignored.md\n"),
        ("ignored.md", "i\n"),
    ] {
        fs::write(dir.0.join(name), content).map_err(|e| format!("write {name}: {e}"))?;
    }

    let mut workspace = Workspace::discover(&dir.0)?;
    let status = workspace.status().map_err(|e| format!("status: {e}"))?;
    let describe: Vec<(String, State, bool, usize, usize)> = status
        .entries()
        .iter()
        .map(|e| {
            (
                e.path().display().to_string(),
                e.state(),
                e.is_staged(),
                e.added(),
                e.removed(),
            )
        })
        .collect();
    assert_eq!(
        describe,
        vec![
            (".gitignore".to_owned(), State::Untracked, false, 1, 0),
            ("edited.md".to_owned(), State::Modified, false, 2, 1),
            ("gone.md".to_owned(), State::Deleted, false, 0, 1),
            ("new.md".to_owned(), State::Added, true, 1, 0),
            ("staged.md".to_owned(), State::Modified, true, 1, 1),
            ("untracked.md".to_owned(), State::Untracked, false, 2, 0),
        ]
    );
    assert_eq!(
        workspace.index_text(Path::new("staged.md"))?.as_deref(),
        Some("s2\n")
    );
    assert_eq!(
        workspace.index_text(Path::new("untracked.md"))?.as_deref(),
        Some("")
    );
    assert_eq!(
        workspace.head_text(Path::new("staged.md"))?.as_deref(),
        Some("s\n")
    );

    // Removing the index entry for a HEAD file is a staged deletion.
    stage(
        &dir.0,
        &[
            ("clean.md", "c\n"),
            ("edited.md", "one\ntwo\n"),
            ("staged.md", "s2\n"),
        ],
    )?;
    let status = workspace.status().map_err(|e| format!("status: {e}"))?;
    assert_eq!(
        status
            .get(Path::new("gone.md"))
            .map(|gone| (gone.state(), gone.is_staged())),
        Some((State::Deleted, true))
    );
    Ok(())
}
