//! Behaviour of the workspace listing and the tree pane's tree (ADR 0012).

use std::fs;
use std::path::{Path, PathBuf};

use fathomable_testing::TempDir;

use fathomable_core::tree::{Activation, Row, Tree};
use fathomable_core::workspace::{EntryKind, Filter, Workspace};

/// A workspace with nested dirs, hidden entries, and mixed-case names.
fn fixture(name: &str) -> std::io::Result<TempDir> {
    let dir = TempDir::new(&format!("tree-{name}"))?;
    fs::create_dir_all(dir.0.join("src/nested"))?;
    fs::create_dir_all(dir.0.join(".git"))?;
    fs::create_dir_all(dir.0.join(".hidden"))?;
    fs::write(dir.0.join("README.md"), "# Readme\n")?;
    fs::write(dir.0.join("b.txt"), "")?;
    fs::write(dir.0.join("A.txt"), "")?;
    fs::write(dir.0.join("src/main.rs"), "")?;
    fs::write(dir.0.join("src/nested/deep.rs"), "")?;
    Ok(dir)
}

fn names(tree: &Tree) -> Vec<String> {
    tree.rows()
        .iter()
        .map(|row| format!("{}{}", "  ".repeat(row.depth()), row.name()))
        .collect()
}

#[test]
fn plain_directory_lists_dirs_first_and_hides_git() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fixture("plain")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    assert!(!workspace.is_git());
    let entries: Vec<_> = workspace
        .list_dir("")?
        .into_iter()
        .map(|e| e.name().to_owned())
        .collect();
    assert_eq!(entries, [".hidden", "src", "A.txt", "b.txt", "README.md"]);
    assert_eq!(
        workspace.walk_files(Filter::Visible),
        [
            "A.txt",
            "b.txt",
            "README.md",
            "src/main.rs",
            "src/nested/deep.rs"
        ]
    );
    Ok(())
}

#[test]
fn tree_expands_lazily_and_navigates() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fixture("nav")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let mut tree = Tree::new(&mut workspace)?;
    assert_eq!(
        names(&tree),
        [".hidden", "src", "A.txt", "b.txt", "README.md"]
    );

    tree.move_down(1);
    assert_eq!(tree.activate(&mut workspace)?, Some(Activation::Toggled));
    assert_eq!(names(&tree)[1..4], ["src", "  nested", "  main.rs"]);
    tree.expand(&mut workspace)?;
    assert_eq!(tree.current().map(Row::name), Some("nested"));
    tree.expand(&mut workspace)?;
    tree.expand(&mut workspace)?;
    assert_eq!(
        tree.activate(&mut workspace)?,
        Some(Activation::Open(PathBuf::from("src/nested/deep.rs")))
    );

    tree.collapse();
    assert_eq!(tree.current().map(Row::path), Some(Path::new("src/nested")));
    tree.collapse();
    assert!(!tree.current().is_some_and(Row::expanded));
    tree.collapse();
    assert_eq!(tree.current().map(Row::path), Some(Path::new("src")));
    tree.goto_bottom();
    assert_eq!(tree.current().map(Row::name), Some("README.md"));
    tree.goto_top();
    assert_eq!(tree.cursor(), 0);
    Ok(())
}

#[test]
fn reveal_and_refresh_keep_position() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fixture("reveal")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let mut tree = Tree::new(&mut workspace)?;
    assert!(tree.reveal(&mut workspace, Path::new("src/nested/deep.rs"))?);
    assert_eq!(tree.current().map(Row::name), Some("deep.rs"));
    assert!(!tree.reveal(&mut workspace, Path::new("src/gone.rs"))?);

    fs::write(dir.0.join("src/nested/new.rs"), "")?;
    tree.refresh(&mut workspace)?;
    assert!(names(&tree).contains(&"    new.rs".to_owned()));
    assert_eq!(
        tree.current().map(Row::name),
        Some("deep.rs"),
        "cursor survives refresh"
    );
    Ok(())
}

#[test]
fn refresh_dir_rereads_one_expanded_directory() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fixture("refresh-dir")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let mut tree = Tree::new(&mut workspace)?;
    assert!(tree.reveal(&mut workspace, Path::new("src/nested/deep.rs"))?);

    // A collapsed or unread directory is left alone.
    fs::write(dir.0.join(".hidden/x"), "")?;
    assert!(!tree.refresh_dir(&mut workspace, Path::new(".hidden"))?);
    assert!(!tree.contains(Path::new(".hidden/x")));

    // An expanded one is re-read; subdirectories keep their expansion and
    // the cursor stays on its row.
    fs::write(dir.0.join("src/lib.rs"), "")?;
    fs::remove_file(dir.0.join("src/main.rs"))?;
    assert!(tree.refresh_dir(&mut workspace, Path::new("src"))?);
    assert_eq!(
        names(&tree),
        [
            ".hidden",
            "src",
            "  nested",
            "    deep.rs",
            "  lib.rs",
            "A.txt",
            "b.txt",
            "README.md"
        ]
    );
    assert_eq!(tree.current().map(Row::name), Some("deep.rs"));

    // The root is a directory too.
    fs::write(dir.0.join("NEW.md"), "")?;
    assert!(tree.refresh_dir(&mut workspace, Path::new(""))?);
    assert!(tree.contains(Path::new("NEW.md")));

    // A directory that vanished collapses rather than failing.
    fs::remove_dir_all(dir.0.join("src/nested"))?;
    assert!(!tree.refresh_dir(&mut workspace, Path::new("src/nested"))?);
    assert!(!tree.contains(Path::new("src/nested/deep.rs")));
    assert!(tree.refresh_dir(&mut workspace, Path::new("src"))?);
    assert!(!tree.contains(Path::new("src/nested")));

    // `expand_to` opens the way to a file without moving the cursor.
    tree.goto_top();
    fs::create_dir_all(dir.0.join("docs/inner"))?;
    fs::write(dir.0.join("docs/inner/a.md"), "")?;
    tree.refresh_dir(&mut workspace, Path::new(""))?;
    assert!(tree.expand_to(&mut workspace, Path::new("docs/inner/a.md"))?);
    assert!(tree.contains(Path::new("docs/inner/a.md")));
    assert_eq!(tree.cursor(), 0);
    assert!(!tree.expand_to(&mut workspace, Path::new("docs/none/b.md"))?);
    Ok(())
}

#[test]
fn refresh_dir_brings_a_new_directory_into_view() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fixture("refresh-new-dir")?;
    let mut workspace = Workspace::discover(&dir.0)?;
    let mut tree = Tree::new(&mut workspace)?;
    assert!(tree.reveal(&mut workspace, Path::new("src/main.rs"))?);

    // An agent makes a directory and writes into it in one burst: the
    // event lands on a directory the tree has never listed, and the
    // listing that names it is re-read instead.
    fs::create_dir_all(dir.0.join("src/fresh/deeper"))?;
    fs::write(dir.0.join("src/fresh/deeper/new.rs"), "")?;
    assert!(tree.refresh_dir(&mut workspace, Path::new("src/fresh/deeper"))?);
    assert!(tree.contains(Path::new("src/fresh")));
    assert!(!tree.contains(Path::new("src/fresh/deeper")), "still lazy");
    assert_eq!(
        tree.current().map(Row::name),
        Some("main.rs"),
        "cursor stays"
    );

    // A collapsed directory the tree has read keeps its listing current,
    // so re-expanding it shows what arrived while it was shut.
    tree.reveal(&mut workspace, Path::new("src"))?;
    tree.collapse();
    assert!(!tree.contains(Path::new("src/main.rs")));
    fs::write(dir.0.join("src/late.rs"), "")?;
    assert!(tree.refresh_dir(&mut workspace, Path::new("src"))?);
    tree.expand(&mut workspace)?;
    assert!(tree.contains(Path::new("src/late.rs")));
    assert!(tree.contains(Path::new("src/nested")), "subdirectory kept");
    Ok(())
}

/// A minimal repository gix can discover: HEAD, config, and empty
/// object and ref stores, so the test never depends on host git.
fn init_git(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir.join(".git/objects"))?;
    fs::create_dir_all(dir.join(".git/refs/heads"))?;
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n")?;
    fs::write(
        dir.join(".git/config"),
        "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n",
    )
}

#[test]
fn git_workspace_roots_at_the_repository_and_ignores_files()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = fixture("git")?;
    init_git(&dir.0)?;
    fs::write(dir.0.join(".gitignore"), "target/\n*.log\n!keep.log\n")?;
    fs::create_dir_all(dir.0.join("target/debug"))?;
    fs::write(dir.0.join("src/nested/.gitignore"), "deep.rs\n")?;
    fs::write(dir.0.join("src/app.log"), "")?;
    fs::write(dir.0.join("src/keep.log"), "")?;

    let mut workspace = Workspace::discover(dir.0.join("src/nested"))?;
    assert!(workspace.is_git());
    assert_eq!(workspace.root(), dir.0.canonicalize()?);
    assert_eq!(
        workspace.relative(&dir.0.join("src/main.rs")),
        Path::new("src/main.rs")
    );
    assert!(workspace.is_ignored(Path::new("target"), EntryKind::Dir));
    assert!(workspace.is_ignored(Path::new("src/app.log"), EntryKind::File));
    assert!(!workspace.is_ignored(Path::new("src/keep.log"), EntryKind::File));
    assert!(workspace.is_ignored(Path::new("src/nested/deep.rs"), EntryKind::File));

    let top: Vec<_> = workspace
        .list_dir("")?
        .into_iter()
        .map(|e| e.name().to_owned())
        .collect();
    assert_eq!(
        top,
        [
            ".hidden",
            "src",
            ".gitignore",
            "A.txt",
            "b.txt",
            "README.md"
        ]
    );
    assert_eq!(
        workspace.walk_files(Filter::Visible),
        [
            ".gitignore",
            "A.txt",
            "b.txt",
            "README.md",
            "src/keep.log",
            "src/main.rs",
            "src/nested/.gitignore"
        ]
    );
    let all = workspace.walk_files(Filter::All);
    assert!(all.iter().any(|p| p == "src/app.log"));
    assert!(all.iter().any(|p| p == "src/nested/deep.rs"));
    assert!(!all.iter().any(|p| p.starts_with(".git/")));

    let mut tree = Tree::new(&mut workspace)?;
    assert!(!names(&tree).iter().any(|n| n == "target"));
    tree.set_filter(&mut workspace, Filter::All)?;
    assert!(names(&tree).iter().any(|n| n == "target"));
    Ok(())
}
