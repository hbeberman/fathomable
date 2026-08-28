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

/// Commit `files` (path, content, kind) as `HEAD` and stage the same tree.
#[cfg(unix)]
fn commit_and_stage(
    root: &Path,
    files: &[(&str, &str, gix::objs::tree::EntryKind)],
) -> Result<(), Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let mut entries = Vec::new();
    for (name, content, kind) in files {
        let oid = repo.write_blob(content.as_bytes())?.detach();
        entries.push(gix::objs::tree::Entry {
            mode: (*kind).into(),
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

#[cfg(unix)]
#[test]
fn symlinks_diff_by_target_path_not_followed_content() -> TestResult {
    use fathomable_core::status::State;
    use gix::objs::tree::EntryKind;

    let dir = TempDir::new("symlink")?;
    init(&dir.0)?;
    fs::write(dir.0.join("a.md"), "one\ntwo\n")?;
    fs::write(dir.0.join("b.md"), "b\n")?;
    std::os::unix::fs::symlink("a.md", dir.0.join("link.md"))?;
    commit_and_stage(
        &dir.0,
        &[
            ("a.md", "one\ntwo\n", EntryKind::Blob),
            ("b.md", "b\n", EntryKind::Blob),
            ("link.md", "a.md", EntryKind::Link),
        ],
    )?;

    let mut workspace = Workspace::discover(&dir.0)?;
    assert!(
        workspace.status()?.is_empty(),
        "an unchanged committed symlink is clean"
    );
    assert_eq!(
        workspace.head_text(Path::new("link.md"))?.as_deref(),
        Some("a.md"),
        "a symlink's diff base is its committed target path"
    );

    // An untracked symlink counts its one-line target, not the file it
    // points at.
    std::os::unix::fs::symlink("a.md", dir.0.join("new-link.md"))?;
    let status = workspace.status()?;
    let entry = status
        .get(Path::new("new-link.md"))
        .ok_or("new-link.md missing")?;
    assert_eq!(
        (entry.state(), entry.added(), entry.removed()),
        (State::Untracked, 1, 0)
    );
    fs::remove_file(dir.0.join("new-link.md"))?;

    // Retargeting the committed symlink is an unstaged modification.
    fs::remove_file(dir.0.join("link.md"))?;
    std::os::unix::fs::symlink("b.md", dir.0.join("link.md"))?;
    let status = workspace.status()?;
    let entry = status.get(Path::new("link.md")).ok_or("link.md missing")?;
    assert_eq!(
        (
            entry.state(),
            entry.is_staged(),
            entry.added(),
            entry.removed()
        ),
        (State::Modified, false, 1, 1)
    );
    assert_eq!(status.len(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn directory_symlinks_browse_as_dirs_but_status_never_descends() -> TestResult {
    use fathomable_core::status::State;
    use gix::objs::tree::EntryKind;

    let dir = TempDir::new("dirlink")?;
    init(&dir.0)?;
    fs::create_dir(dir.0.join("real"))?;
    fs::write(dir.0.join("real/inner.md"), "i\n")?;
    std::os::unix::fs::symlink("real", dir.0.join("linkdir"))?;
    // A cycle back to the root must not hang the walk.
    std::os::unix::fs::symlink(".", dir.0.join("loop"))?;
    commit_and_stage(&dir.0, &[("linkdir", "real", EntryKind::Link)])?;

    let mut workspace = Workspace::discover(&dir.0)?;
    let listed: Vec<(String, bool, bool)> = workspace
        .list_dir("")?
        .iter()
        .map(|entry| (entry.name().to_owned(), entry.is_dir(), entry.is_symlink()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("linkdir".to_owned(), true, true),
            ("loop".to_owned(), true, true),
            ("real".to_owned(), true, false),
        ],
        "a symlink to a directory stays browsable and is flagged as a link"
    );

    let status = workspace.status()?;
    let describe: Vec<(String, State)> = status
        .entries()
        .iter()
        .map(|e| (e.path().display().to_string(), e.state()))
        .collect();
    assert_eq!(
        describe,
        vec![
            ("loop".to_owned(), State::Untracked),
            ("real/inner.md".to_owned(), State::Untracked),
        ],
        "the committed link is clean and nothing behind a link is walked"
    );
    Ok(())
}

/// Commit `files` on `reference` with `parent`, returning the new id.
fn commit_on(
    root: &Path,
    reference: &str,
    parent: Option<gix::ObjectId>,
    files: &[(&str, &str)],
) -> Result<String, Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let mut entries = Vec::new();
    for (name, content) in files {
        entries.push(gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: (*name).into(),
            oid: repo.write_blob(content.as_bytes())?.detach(),
        });
    }
    entries.sort();
    let tree = repo.write_object(gix::objs::Tree { entries })?.detach();
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time: "0 +0000",
    };
    let id = repo.commit_as(signature, signature, reference, "commit", tree, parent)?;
    Ok(id.to_hex().to_string())
}

/// `reachable` answers for `HEAD` and its ancestors only, so a thread
/// written on another branch is not on this work (ADR 0024).
#[test]
fn reachable_commits_are_head_and_its_ancestors() -> TestResult {
    let dir = TempDir::new("reachable")?;
    init(&dir.0)?;
    let plain = Workspace::discover(&dir.0)?;
    assert_eq!(plain.head_commit(), None, "unborn HEAD has no commit");
    assert_eq!(plain.reachable(["anything"]), None);

    let first = commit_on(&dir.0, "HEAD", None, &[("a.md", "1\n")])?;
    let first_id = gix::ObjectId::from_hex(first.as_bytes())?;
    let second = commit_on(&dir.0, "HEAD", Some(first_id), &[("a.md", "2\n")])?;
    // A branch off the first commit that HEAD does not contain.
    let elsewhere = commit_on(
        &dir.0,
        "refs/heads/elsewhere",
        Some(first_id),
        &[("a.md", "3\n")],
    )?;
    let workspace = Workspace::discover(&dir.0)?;
    assert_eq!(workspace.head_commit().as_deref(), Some(second.as_str()));
    let wanted = [first.as_str(), second.as_str(), elsewhere.as_str(), "nope"];
    let reachable = workspace.reachable(wanted).ok_or("git workspace")?;
    assert!(reachable.contains(&first));
    assert!(reachable.contains(&second));
    assert!(!reachable.contains(&elsewhere));
    assert!(!reachable.contains("nope"));
    assert_eq!(
        workspace.reachable(std::iter::empty()).map(|set| set.len()),
        Some(0)
    );
    Ok(())
}
