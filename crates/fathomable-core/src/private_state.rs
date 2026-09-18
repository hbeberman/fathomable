// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! Private Linux state files, without permission repair or migration.
//!
//! New directories and files request modes 0700 and 0600 at creation.
//! Existing application directories must be 0700; files must be singly
//! linked, regular, 0600, and owned by the effective UID. Symlinks are refused.
//! Ancestors outside the application may be readable, but must be owned by
//! root or this UID and not writable by other users (except sticky directories).
//! This protects against other local UIDs, not root or same-UID processes.

use std::fs::{self, DirBuilder, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

// Linux open(2): reject final symlinks and never block on a substituted FIFO.
const NOFOLLOW_NONBLOCK: i32 = 0x2_0000 | 0x800;

fn denied(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "unsafe state path {}: {reason}; left unchanged",
            path.display()
        ),
    )
}

fn effective_uid() -> io::Result<u32> {
    // Kernel-provided identity; USER/UID/HOME are not authentication sources.
    fs::read_to_string("/proc/self/status")?
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|uids| uids.split_whitespace().nth(1))
        .and_then(|uid| uid.parse().ok())
        .ok_or_else(|| io::Error::other("cannot determine effective UID from /proc/self/status"))
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(denied(&path, "parent traversal is not allowed"));
    }
    Ok(path)
}

fn ancestor(path: &Path, metadata: &Metadata, uid: u32) -> io::Result<()> {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(denied(
            path,
            "expected a directory, not a link or special file",
        ));
    }
    if metadata.uid() != uid && metadata.uid() != 0 {
        return Err(denied(path, "ancestor is owned by another UID"));
    }
    if metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0 {
        return Err(denied(
            path,
            "ancestor is writable by other users without the sticky bit",
        ));
    }
    Ok(())
}

fn parents(path: &Path, uid: u32, create: bool) -> io::Result<()> {
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                match DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                fs::symlink_metadata(&current)?
            }
            Err(error) => return Err(error),
        };
        ancestor(&current, &metadata, uid)?;
    }
    Ok(())
}

/// Create or validate a private application-owned directory.
///
/// Missing ancestors are created privately; existing external ancestors are
/// validated, never chmodded. Call this on each application-owned ancestor,
/// not just a leaf directory.
///
/// # Errors
///
/// Refuses links, unsafe ancestors, wrong ownership, or a final mode other
/// than 0700. Also returns filesystem and kernel-identity lookup errors.
pub fn ensure_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = absolute(path.as_ref())?;
    let uid = effective_uid()?;
    parents(&path, uid, true)?;
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(denied(
            &path,
            "directory must be owned by the effective UID with mode 0700",
        ));
    }
    Ok(())
}

fn private_file(path: &Path, metadata: &Metadata, uid: u32) -> io::Result<()> {
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(denied(
            path,
            "file must be regular, singly linked, owned by the effective UID with mode 0600",
        ));
    }
    Ok(())
}

fn open(path: &Path, options: &mut OpenOptions, create: bool) -> io::Result<File> {
    let path = absolute(path)?;
    let uid = effective_uid()?;
    if let Some(parent) = path.parent() {
        parents(parent, uid, create)?;
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) => private_file(&path, &metadata, uid)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let file = options
        .mode(0o600)
        .custom_flags(NOFOLLOW_NONBLOCK)
        .open(&path)?;
    private_file(&path, &file.metadata()?, uid)?;
    Ok(file)
}

/// Open an existing private file for reading, without creating any paths.
///
/// # Errors
///
/// Returns an error for missing files, unsafe paths, ownership, links, or modes.
pub fn open_read(path: impl AsRef<Path>) -> io::Result<File> {
    open(path.as_ref(), OpenOptions::new().read(true), false)
}

/// Open or create a private appendable file, also readable for locked replay.
///
/// Existing external parent directories need not be private. Applications
/// must separately validate their owned hierarchy with [`ensure_dir`].
///
/// # Errors
///
/// Returns an error for unsafe paths, ownership, links, modes, or filesystem errors.
pub fn open_append(path: impl AsRef<Path>) -> io::Result<File> {
    open(
        path.as_ref(),
        OpenOptions::new().read(true).append(true).create(true),
        true,
    )
}

/// Exclusively create a new private writable file.
///
/// # Errors
///
/// Returns an error if the path exists, its ancestors are unsafe, or creation fails.
pub fn create_new(path: impl AsRef<Path>) -> io::Result<File> {
    open(
        path.as_ref(),
        OpenOptions::new().write(true).create_new(true),
        true,
    )
}

/// Read bytes from an existing private file.
///
/// # Errors
///
/// Returns the errors of [`open_read`] or reading the file.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_read(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Write a private file, validating it before truncating existing contents.
///
/// # Errors
///
/// Returns an error for unsafe paths, ownership, links, modes, or filesystem errors.
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let mut file = open(
        path.as_ref(),
        OpenOptions::new().write(true).create(true),
        true,
    )?;
    file.set_len(0)?;
    file.write_all(contents.as_ref())
}
