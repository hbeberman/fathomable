//! Behaviour of the `HEAD` diff base (ADR 0006) against a real repository
//! built with `gix`, so the tests need no host `git`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::workspace::Workspace;

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

/// Commit `files` (root-relative path, content) as the only tree of `HEAD`.
fn commit(root: &Path, files: &[(&str, &str)]) -> Result<(), Box<dyn Error>> {
    let repo = gix::open(root)?;
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
    gix::init(&dir.0)?;
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
    gix::init(&dir.0)?;
    fs::create_dir_all(dir.0.join("docs"))?;
    // Nested paths go through a subtree; build it explicitly.
    let repo = gix::open(&dir.0)?;
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
