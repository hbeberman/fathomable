//! Behaviour of the `HEAD` diff base (ADR 0006) against a real repository
//! built with `gix`, so the tests need no host `git`.

use std::error::Error;
use std::fs;
use std::path::Path;

use fathomable_core::workspace::{Workspace, open_options};
use fathomable_testing::TempDir;
use fathomable_testing::git::{init, stage, write_tree};

type TestResult = Result<(), Box<dyn Error>>;

/// Commit `files` (root-relative path, content) as the only tree of `HEAD`.
fn commit(root: &Path, files: &[(&str, &str)]) -> Result<(), Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let tree = write_tree(&repo, files)?;
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
    let dir = TempDir::new("git-plain")?;
    fs::write(dir.0.join("a.md"), "x\n")?;
    let workspace = Workspace::discover(&dir.0)?;
    assert!(!workspace.is_git());
    assert_eq!(workspace.head_text(Path::new("a.md"))?, None);
    Ok(())
}

#[test]
fn unborn_head_and_untracked_files_have_an_empty_base() -> TestResult {
    let dir = TempDir::new("git-unborn")?;
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
    let dir = TempDir::new("git-head")?;
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

#[test]
fn status_tells_staged_unstaged_and_untracked_apart() -> TestResult {
    use fathomable_core::status::State;

    let dir = TempDir::new("git-status")?;
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

#[test]
fn an_edited_root_ignore_file_takes_effect_on_reload() -> TestResult {
    use fathomable_core::workspace::{Filter, is_rules_file};

    let dir = TempDir::new("git-reload-rules")?;
    init(&dir.0)?;
    commit(&dir.0, &[("kept.md", "k\n")])?;
    stage(&dir.0, &[("kept.md", "k\n")])?;
    fs::write(dir.0.join("kept.md"), "k\n")?;
    fs::write(dir.0.join("scratch.md"), "s\n")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let dirty = |workspace: &mut Workspace| -> Result<Vec<String>, Box<dyn Error>> {
        Ok(workspace
            .status()?
            .entries()
            .iter()
            .map(|e| e.path().display().to_string())
            .collect())
    };
    assert_eq!(dirty(&mut workspace)?, vec!["scratch.md"]);

    // The root's rules were read when the workspace opened; the reload
    // is what makes the new pattern count, for the listing and the set.
    fs::write(dir.0.join(".gitignore"), "scratch.md\n")?;
    assert!(is_rules_file(Path::new(".gitignore")));
    assert!(is_rules_file(Path::new("docs/.gitattributes")));
    assert!(is_rules_file(Path::new(".git/info/exclude")));
    assert!(!is_rules_file(Path::new("docs/ignore.md")));
    workspace.reload_rules()?;
    assert_eq!(dirty(&mut workspace)?, vec![".gitignore"]);
    assert!(
        !workspace
            .walk_files(Filter::Visible)
            .iter()
            .any(|f| f == "scratch.md"),
        "the listing hides it too"
    );

    // Un-ignoring it brings it back the same way.
    fs::write(dir.0.join(".gitignore"), "")?;
    workspace.reload_rules()?;
    assert_eq!(dirty(&mut workspace)?, vec![".gitignore", "scratch.md"]);
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one walk through every kind of change"
)]
fn status_after_examines_only_the_named_paths() -> TestResult {
    use std::path::PathBuf;

    use fathomable_core::status::{State, Status};

    let dir = TempDir::new("git-status-after")?;
    init(&dir.0)?;
    let tree = [
        ("a.md", "a\n"),
        ("dir/b.md", "b\n"),
        ("dir/c.md", "c\n"),
        ("gone.md", "g\n"),
        (".gitignore", "build/\n"),
    ];
    commit(&dir.0, &tree)?;
    stage(&dir.0, &tree)?;
    for (name, content) in tree {
        let path = dir.0.join(name);
        fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        fs::write(path, content)?;
    }
    fs::write(dir.0.join("untracked.md"), "u\n")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let paths = |names: &[&str]| -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() };
    let describe = |status: &Status| -> Vec<(String, State, bool)> {
        status
            .entries()
            .iter()
            .map(|e| (e.path().display().to_string(), e.state(), e.is_staged()))
            .collect()
    };
    let mut current = workspace.status()?;
    assert_eq!(
        describe(&current),
        vec![("untracked.md".to_owned(), State::Untracked, false)]
    );

    // Only the named path is examined: the other edit waits for its
    // own event, as the full walk would find it.
    fs::write(dir.0.join("a.md"), "a\nmore\n")?;
    fs::write(dir.0.join("late.md"), "l\n")?;
    current = workspace.status_after(&current, &paths(&["a.md"]))?;
    assert_eq!(
        describe(&current),
        vec![
            ("a.md".to_owned(), State::Modified, false),
            ("untracked.md".to_owned(), State::Untracked, false),
        ]
    );
    assert_eq!(
        current
            .get(Path::new("a.md"))
            .map(|e| (e.added(), e.removed())),
        Some((1, 0)),
        "line counts come with the entry"
    );
    current = workspace.status_after(&current, &paths(&["late.md"]))?;
    assert_eq!(current, workspace.status()?);

    // An event on a directory covers its tracked files, its dirty
    // entries, and what it holds on disk: removed whole, created whole.
    fs::remove_dir_all(dir.0.join("dir"))?;
    current = workspace.status_after(&current, &paths(&["dir"]))?;
    assert_eq!(current, workspace.status()?);
    assert_eq!(
        current
            .get(Path::new("dir/b.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Deleted, false))
    );
    fs::create_dir(dir.0.join("new"))?;
    fs::write(dir.0.join("new/x.md"), "x\n")?;
    fs::write(dir.0.join("new/y.md"), "y\n")?;
    current = workspace.status_after(&current, &paths(&["new"]))?;
    assert_eq!(current, workspace.status()?);
    assert!(current.contains(Path::new("new/y.md")));

    // Ignored output never joins; an edit undone leaves.
    fs::create_dir(dir.0.join("build"))?;
    fs::write(dir.0.join("build/out.o"), "o\n")?;
    fs::write(dir.0.join("a.md"), "a\n")?;
    current = workspace.status_after(&current, &paths(&["build/out.o", "a.md"]))?;
    assert_eq!(current, workspace.status()?);
    assert!(!current.contains(Path::new("a.md")));
    assert!(!current.contains(Path::new("build/out.o")));

    // A staged deletion is covered by the file coming back untracked,
    // and uncovered when it goes again.
    fs::remove_file(dir.0.join("gone.md"))?;
    stage(&dir.0, &tree[..3])?;
    current = workspace.status()?;
    assert_eq!(
        current
            .get(Path::new("gone.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Deleted, true))
    );
    fs::write(dir.0.join("gone.md"), "g\n")?;
    let untracked_again = workspace.status_after(&current, &paths(&["gone.md"]))?;
    assert_eq!(untracked_again, workspace.status()?);
    assert_eq!(
        untracked_again
            .get(Path::new("gone.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Untracked, false))
    );
    fs::remove_file(dir.0.join("gone.md"))?;
    current = workspace.status_after(&untracked_again, &paths(&["gone.md"]))?;
    assert_eq!(current, workspace.status()?);
    assert_eq!(
        current
            .get(Path::new("gone.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Deleted, true))
    );

    // A rules file among the paths means the whole tree is walked.
    fs::write(dir.0.join(".gitignore"), "build/\nnew/\n")?;
    fs::write(dir.0.join("unseen.md"), "s\n")?;
    workspace.reload_rules()?;
    current = workspace.status_after(&current, &paths(&[".gitignore"]))?;
    assert_eq!(current, workspace.status()?);
    assert!(!current.contains(Path::new("new/x.md")));
    assert!(
        current.contains(Path::new("unseen.md")),
        "the full walk found it"
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

    let dir = TempDir::new("git-symlink")?;
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

    let dir = TempDir::new("git-dirlink")?;
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
    commit_on_at(root, reference, parent, files, "0 +0000")
}

/// [`commit_on`] with the committer `time` (`"<seconds> +0000"`).
fn commit_on_at(
    root: &Path,
    reference: &str,
    parent: Option<gix::ObjectId>,
    files: &[(&str, &str)],
    time: &str,
) -> Result<String, Box<dyn Error>> {
    let repo = gix::open_opts(root, open_options())?;
    let tree = write_tree(&repo, files)?;
    let signature = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@example.com".into(),
        time,
    };
    let id = repo.commit_as(signature, signature, reference, "commit", tree, parent)?;
    Ok(id.to_hex().to_string())
}

/// `reachable` answers for `HEAD` and its ancestors only, so a thread
/// written on another branch is not on this work (ADR 0024).
#[test]
fn reachable_commits_are_head_and_its_ancestors() -> TestResult {
    let dir = TempDir::new("git-reachable")?;
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
    // A well-formed id the object store never held, as after a `gc`.
    let gone = "0123456789abcdef0123456789abcdef01234567";
    let wanted = [
        first.as_str(),
        second.as_str(),
        elsewhere.as_str(),
        gone,
        "nope",
    ];
    let reachable = workspace.reachable(wanted).ok_or("git workspace")?;
    assert!(reachable.contains(&first));
    assert!(reachable.contains(&second));
    assert!(!reachable.contains(&elsewhere));
    assert!(!reachable.contains(gone));
    assert!(!reachable.contains("nope"));
    assert_eq!(
        workspace.reachable(std::iter::empty()).map(|set| set.len()),
        Some(0)
    );
    Ok(())
}

/// The walk is bounded by the oldest wanted commit's time, so a wanted
/// commit far older than `HEAD` is still met, and one only a rewrite
/// dropped is still missed, whatever their dates.
#[test]
fn reachable_finds_a_wanted_commit_much_older_than_head() -> TestResult {
    const MONTH: i64 = 30 * 24 * 60 * 60;
    let dir = TempDir::new("git-reachable-old")?;
    init(&dir.0)?;
    let old = commit_on_at(&dir.0, "HEAD", None, &[("a.md", "1\n")], "0 +0000")?;
    let old_id = gix::ObjectId::from_hex(old.as_bytes())?;
    let dropped = commit_on_at(
        &dir.0,
        "refs/heads/dropped",
        Some(old_id),
        &[("a.md", "2\n")],
        &format!("{MONTH} +0000"),
    )?;
    let mut parent = old_id;
    for (months, text) in (2..).zip(["3\n", "4\n", "5\n"]) {
        let id = commit_on_at(
            &dir.0,
            "HEAD",
            Some(parent),
            &[("a.md", text)],
            &format!("{} +0000", months * MONTH),
        )?;
        parent = gix::ObjectId::from_hex(id.as_bytes())?;
    }
    let workspace = Workspace::discover(&dir.0)?;
    let reachable = workspace
        .reachable([old.as_str(), dropped.as_str()])
        .ok_or("git workspace")?;
    assert!(
        reachable.contains(&old),
        "the root commit is an ancestor of HEAD"
    );
    assert!(
        !reachable.contains(&dropped),
        "a commit off HEAD's line is not"
    );
    Ok(())
}

#[test]
fn binary_files_follow_the_diff_attribute_then_the_nul_sniff() -> TestResult {
    use fathomable_core::content::Attr;
    use fathomable_core::status::State;

    let dir = TempDir::new("git-binary")?;
    init(&dir.0)?;
    let head = [
        (".gitattributes", "*.dat binary\n*.nul diff\n"),
        ("plain.dat", "text by content\n"),
        ("forced.nul", "a\0b\n"),
        ("blob.bin", "\0asm\x01\0\0\0"),
        ("notes.md", "one\n"),
    ];
    commit(&dir.0, &head)?;
    stage(&dir.0, &head)?;
    for (name, content) in head {
        fs::write(dir.0.join(name), content)?;
    }
    let mut workspace = Workspace::discover(&dir.0)?;

    // The attribute answers before any bytes are read.
    assert_eq!(workspace.diff_attr(Path::new("plain.dat")), Attr::Binary);
    assert_eq!(workspace.diff_attr(Path::new("forced.nul")), Attr::Text);
    assert_eq!(
        workspace.diff_attr(Path::new("blob.bin")),
        Attr::Unspecified
    );
    assert_eq!(
        workspace.diff_attr(Path::new("notes.md")),
        Attr::Unspecified
    );

    // HEAD sizes come from the object headers.
    assert_eq!(workspace.head_size(Path::new("blob.bin"))?, Some(8));
    assert_eq!(workspace.head_size(Path::new("missing.bin"))?, None);

    // Edit every file: the status flags the binaries and counts the rest.
    fs::write(dir.0.join("plain.dat"), "text by content, edited\n")?;
    fs::write(dir.0.join("forced.nul"), "a\0b\nc\n")?;
    fs::write(dir.0.join("blob.bin"), "\0asm\x01\0\0\0more")?;
    fs::write(dir.0.join("notes.md"), "one\ntwo\n")?;
    fs::write(dir.0.join("new.png"), b"\x89PNG\r\n\x1a\n\0\0")?;
    let status = workspace.status()?;
    let flags: Vec<(String, State, bool, usize)> = status
        .entries()
        .iter()
        .map(|e| {
            (
                e.path().display().to_string(),
                e.state(),
                e.is_binary(),
                e.added(),
            )
        })
        .collect();
    assert_eq!(
        flags,
        [
            ("blob.bin".to_owned(), State::Modified, true, 0),
            ("forced.nul".to_owned(), State::Modified, false, 1),
            ("new.png".to_owned(), State::Untracked, true, 0),
            ("notes.md".to_owned(), State::Modified, false, 1),
            ("plain.dat".to_owned(), State::Modified, true, 0),
        ]
    );
    Ok(())
}
