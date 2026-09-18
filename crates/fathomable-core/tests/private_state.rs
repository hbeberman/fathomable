//! Persistent private-state contracts at the public core APIs.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;

use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, Draft, LineRange, Store};
use fathomable_core::private_state;
use fathomable_core::review_points::ReviewPointStore;
use fathomable_core::session::{Id, Marker, Record};
use fathomable_core::workspace::Workspace;
use fathomable_testing::TempDir;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn refused<T, E: std::fmt::Display>(result: std::result::Result<T, E>) -> Result {
    let Err(error) = result else {
        return Err("unsafe state path accepted".into());
    };
    assert!(error.to_string().contains("unsafe state path"), "{error}");
    Ok(())
}

fn dirs(root: &Path) -> XdgDirs {
    XdgDirs::resolve(|name| (name == "XDG_STATE_HOME").then(|| root.into()))
}

fn mode(path: &Path) -> io::Result<u32> {
    Ok(fs::symlink_metadata(path)?.mode() & 0o7777)
}

fn assert_private_tree(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() {
            assert_eq!(mode(&path)?, 0o700, "{}", path.display());
            assert_private_tree(&path)?;
        } else {
            assert_eq!(mode(&path)?, 0o600, "{}", path.display());
        }
    }
    Ok(())
}

#[test]
fn public_state_apis_ignore_permissive_umasks() -> Result {
    for mask in ["000", "022"] {
        let output = Command::new("sh")
            .args([
                "-c",
                "umask \"$1\"; exec \"$2\" --exact private_state_child --nocapture",
                "state-test",
                mask,
            ])
            .arg(std::env::current_exe()?)
            .env("FATHOMABLE_PRIVATE_STATE_CHILD", mask)
            .env("UID", "4294967294")
            .output()?;
        assert!(
            output.status.success(),
            "umask {mask}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[test]
fn private_state_child() -> Result {
    let Ok(mask) = std::env::var("FATHOMABLE_PRIVATE_STATE_CHILD") else {
        return Ok(());
    };
    let fixture = TempDir::new(&format!("private-state-{mask}"))?;
    // An external, traversable ancestor is not application-owned.
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755))?;
    let sentinel = fixture.0.join("unrelated");
    fs::write(&sentinel, "untouched")?;
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o644))?;
    let source = fixture.0.join("source");
    fs::DirBuilder::new().mode(0o700).create(&source)?;
    private_state::write(source.join("private.md"), "synthetic private source\n")?;
    let mut workspace = Workspace::discover(&source)?;
    let dirs = dirs(&fixture.0.join("xdg"));
    let mut store = Store::open_workspace(&dirs, workspace.key())?;
    store.annotate(
        Draft::new(
            Author::User,
            Path::new("private.md"),
            LineRange::new(1, 1),
            "private comment",
        ),
        "synthetic private source\n",
        1,
    )?;
    assert_eq!(
        Store::open_workspace(&dirs, workspace.key())?
            .threads()
            .len(),
        1
    );

    let mut points = ReviewPointStore::open_workspace(&dirs, workspace.key())?;
    let captured = points.capture(&mut workspace, Some("private point"))?;
    let point = captured.point().ok_or("point not published")?;
    assert_eq!(
        points.load_bytes(point, &workspace, Path::new("private.md"))?,
        Some(b"synthetic private source\n".to_vec())
    );
    let key = workspace.key().to_path_buf();
    Record::new(Id::mint(), key.clone(), source.clone(), None).write(&dirs)?;
    Marker::new(key, vec![source.clone()]).write(&dirs)?;
    assert_eq!(Record::list(&dirs).len(), 1);
    assert_eq!(Marker::list(&dirs).len(), 1);
    assert_eq!(mode(&dirs.state_dir())?, 0o700);
    assert_private_tree(&dirs.state_dir())?;

    // The arbitrary-path store API protects its file without claiming ownership
    // of an existing caller directory. XDG entry points also protect the tree.
    let direct = fixture.0.join("direct.jsonl");
    Store::open(&direct)?.annotate(
        Draft::on_file(Author::User, Path::new("private.md"), "private"),
        "synthetic private source\n",
        2,
    )?;
    assert_eq!(mode(&direct)?, 0o600);
    assert_eq!(mode(&fixture.0)?, 0o755);
    assert_eq!(mode(&sentinel)?, 0o644);
    assert_eq!(fs::read_to_string(&sentinel)?, "untouched");
    assert_eq!(mode(&source)?, 0o700);
    assert_eq!(mode(&source.join("private.md"))?, 0o600);
    assert_eq!(
        fs::read_to_string(source.join("private.md"))?,
        "synthetic private source\n"
    );
    Ok(())
}

#[test]
fn xdg_refuses_insecure_owned_directories_without_repair() -> Result {
    for relative in ["", "workspaces", "workspaces/key"] {
        let fixture = TempDir::new(&format!("private-bad-dir-{}", relative.replace('/', "-")))?;
        let dirs = dirs(&fixture.0);
        let target = dirs.state_dir().join(relative);
        private_state::ensure_dir(&target)?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
        let sentinel = target.join("old-data");
        fs::write(&sentinel, "preserve old state")?;
        let Err(error) = dirs.prepare_state_dir(target.join("child")) else {
            return Err("unsafe directory accepted".into());
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            error
                .to_string()
                .contains(target.display().to_string().trim_end_matches('/')),
            "{error}"
        );
        assert_eq!(mode(&target)?, 0o755);
        assert_eq!(fs::read_to_string(sentinel)?, "preserve old state");
        assert!(!target.join("child").exists());
    }
    Ok(())
}

#[test]
fn state_links_hardlinks_and_insecure_files_are_refused_unchanged() -> Result {
    let fixture = TempDir::new("private-state-links")?;
    let target = fixture.0.join("target");
    private_state::write(&target, "unrelated bytes")?;
    let link = fixture.0.join("threads.jsonl");
    symlink(&target, &link)?;
    refused(Store::open(&link))?;
    refused(private_state::write(&link, "replacement"))?;
    assert_eq!(fs::read_to_string(&target)?, "unrelated bytes");
    assert!(fs::symlink_metadata(&link)?.file_type().is_symlink());
    fs::remove_file(&link)?;
    fs::hard_link(&target, &link)?;
    refused(Store::open(&link))?;
    refused(private_state::write(&link, "replacement"))?;
    fs::remove_file(&link)?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644))?;
    refused(Store::open(&target))?;
    refused(private_state::write(&target, "replacement"))?;
    assert_eq!(mode(&target)?, 0o644);
    assert_eq!(fs::read_to_string(&target)?, "unrelated bytes");

    let external = fixture.0.join("external");
    private_state::ensure_dir(&external)?;
    let dirs = dirs(&fixture.0.join("xdg"));
    fs::create_dir_all(fixture.0.join("xdg"))?;
    symlink(&external, dirs.state_dir())?;
    refused(Store::open_workspace(&dirs, Path::new("workspace")))?;
    refused(ReviewPointStore::open_workspace(
        &dirs,
        Path::new("workspace"),
    ))?;
    assert!(fs::read_dir(&external)?.next().is_none());
    assert_eq!(mode(&external)?, 0o700);

    let review = fixture.0.join("review");
    private_state::ensure_dir(&review)?;
    symlink(&external, review.join("blobs"))?;
    refused(ReviewPointStore::open(&review))?;
    assert!(fs::read_dir(&external)?.next().is_none());
    Ok(())
}

#[test]
fn unsafe_ancestors_and_special_files_are_refused() -> Result {
    let fixture = TempDir::new("private-state-ancestor")?;
    let shared = fixture.0.join("shared");
    fs::create_dir(&shared)?;
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777))?;
    refused(private_state::write(shared.join("secret"), "secret"))?;
    assert_eq!(mode(&shared)?, 0o777);
    assert!(fs::read_dir(&shared)?.next().is_none());
    let fifo = fixture.0.join("fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status()?.success());
    refused(Store::open(fifo))?;
    Ok(())
}

#[test]
fn review_store_refuses_an_insecure_manifest() -> Result {
    let fixture = TempDir::new("private-state-manifest")?;
    let directory = fixture.0.join("review");
    drop(ReviewPointStore::open(&directory)?);
    let manifest = directory.join("review-points.jsonl");
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644))?;
    let before = fs::read(&manifest)?;
    refused(ReviewPointStore::open(&directory))?;
    assert_eq!(fs::read(&manifest)?, before);
    assert_eq!(mode(&manifest)?, 0o644);
    Ok(())
}

#[test]
fn review_capture_refuses_reusing_an_insecure_blob() -> Result {
    let fixture = TempDir::new("private-state-blob")?;
    let root = fixture.0.join("source");
    private_state::ensure_dir(&root)?;
    private_state::write(root.join("private.md"), "synthetic private bytes\n")?;
    let mut workspace = Workspace::discover(&root)?;
    let directory = fixture.0.join("review");
    let mut points = ReviewPointStore::open(&directory)?;
    let captured = points.capture(&mut workspace, None)?;
    let point = captured.point().ok_or("point not published")?;
    let blob = fs::read_dir(directory.join("blobs"))?
        .next()
        .ok_or("missing blob")??
        .path();
    let manifest = fs::read(directory.join("review-points.jsonl"))?;
    fs::set_permissions(&blob, fs::Permissions::from_mode(0o644))?;
    refused(points.load_bytes(point, &workspace, Path::new("private.md")))?;
    refused(points.capture(&mut workspace, None))?;
    assert_eq!(mode(&blob)?, 0o644);
    assert_eq!(fs::read_to_string(blob)?, "synthetic private bytes\n");
    assert_eq!(fs::read(directory.join("review-points.jsonl"))?, manifest);
    Ok(())
}

#[test]
fn records_and_markers_refuse_links_without_cleanup_or_overwrite() -> Result {
    let fixture = TempDir::new("private-state-record-links")?;
    let dirs = dirs(&fixture.0);
    let key = fixture.0.join("workspace");
    let record = Record::new(Id::mint(), key.clone(), key.clone(), None);
    record.write(&dirs)?;
    let marker = Marker::new(key.clone(), vec![key.clone()]);
    marker.write(&dirs)?;
    let unrelated = fixture.0.join("unrelated");
    private_state::write(&unrelated, "unchanged")?;
    let record_path = record.dir(&dirs).join("session.json");
    let marker_path = dirs.workspace_dir(&key).join("workspace.json");
    for path in [&record_path, &marker_path] {
        fs::remove_file(path)?;
        symlink(&unrelated, path)?;
    }
    refused(record.write(&dirs))?;
    refused(record.remove(&dirs))?;
    refused(marker.write(&dirs))?;
    assert!(fs::symlink_metadata(record_path)?.file_type().is_symlink());
    assert!(fs::symlink_metadata(marker_path)?.file_type().is_symlink());
    assert_eq!(fs::read_to_string(unrelated)?, "unchanged");
    Ok(())
}
