// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! Private Linux state files, without permission repair or migration.
//!
//! New directories and files request modes 0700 and 0600 at creation.
//! Existing application directories must be 0700; files must be singly
//! linked, regular, 0600, and owned by the effective UID. Symlinks are refused.
//! Ancestors outside the application may be readable, but must be owned by
//! root or this UID. Group-writable external ancestors are accepted with a
//! discoverable warning; non-sticky world-writable ancestors are refused.
//! Group members can interfere with shared ancestor entries, while root and
//! same-UID processes remain outside this protection.

use std::fs::{self, DirBuilder, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

/// Bound retries if concurrent initializers keep claiming a missing filename.
const OPEN_ATTEMPTS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Append,
    Write,
    Create,
}

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

fn ancestor_access(path: &Path, owner: u32, mode: u32, uid: u32) -> io::Result<()> {
    if owner != uid && owner != 0 {
        return Err(denied(path, "ancestor is owned by another UID"));
    }
    if mode & 0o002 != 0 && mode & 0o1000 == 0 {
        return Err(denied(
            path,
            "ancestor is world-writable without the sticky bit",
        ));
    }
    Ok(())
}

fn ancestor(path: &Path, metadata: &Metadata, uid: u32) -> io::Result<()> {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(denied(
            path,
            "expected a directory, not a link or special file",
        ));
    }
    ancestor_access(path, metadata.uid(), metadata.mode(), uid)
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

pub(crate) fn shared_ancestors(path: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
    let path = absolute(path.as_ref())?;
    let uid = effective_uid()?;
    let mut shared = Vec::new();
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        };
        ancestor(&current, &metadata, uid)?;
        if metadata.mode() & 0o020 != 0 && metadata.mode() & 0o1000 == 0 {
            shared.push(current.clone());
        }
    }
    Ok(shared)
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
    private_dir(&path, uid)
}

fn private_dir(path: &Path, uid: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(denied(
            path,
            "directory must be owned by the effective UID with mode 0700",
        ));
    }
    Ok(())
}

/// Validate an existing private application-owned directory without creating it.
pub(crate) fn validate_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = absolute(path.as_ref())?;
    let uid = effective_uid()?;
    parents(&path, uid, false)?;
    private_dir(&path, uid)
}

/// Exclusively create a private directory beneath validated existing ancestors.
///
/// # Errors
///
/// Refuses an existing path, unsafe ancestors, or filesystem and identity errors.
pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = absolute(path.as_ref())?;
    let uid = effective_uid()?;
    if let Some(parent) = path.parent() {
        parents(parent, uid, false)?;
    }
    DirBuilder::new().mode(0o700).create(path)
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

fn open(path: &Path, access: Access) -> io::Result<File> {
    let path = absolute(path)?;
    let uid = effective_uid()?;
    if let Some(parent) = path.parent() {
        parents(parent, uid, access != Access::Read)?;
    }
    for _ in 0..OPEN_ATTEMPTS {
        let exists = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                private_file(&path, &metadata, uid)?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && access != Access::Read => {
                false
            }
            Err(error) => return Err(error),
        };
        // Claim missing names exclusively and revalidate the opened handle.
        // Group-shared ancestors can still redirect an otherwise-valid owned
        // tree; callers report that accepted risk separately.
        let mut options = OpenOptions::new();
        match access {
            Access::Read => options.read(true),
            Access::Append => options.read(true).append(true),
            Access::Write | Access::Create => options.write(true),
        };
        match options
            .mode(0o600)
            .create_new(!exists || access == Access::Create)
            .open(&path)
        {
            Ok(file) => {
                private_file(&path, &file.metadata()?, uid)?;
                return Ok(file);
            }
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists && access != Access::Create => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(format!(
        "state path {} kept changing during creation; retry",
        path.display()
    )))
}

/// Open an existing private file for reading, without creating any paths.
///
/// # Errors
///
/// Returns an error for missing files, unsafe paths, ownership, links, or modes.
pub fn open_read(path: impl AsRef<Path>) -> io::Result<File> {
    open(path.as_ref(), Access::Read)
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
    open(path.as_ref(), Access::Append)
}

/// Exclusively create a new private writable file.
///
/// # Errors
///
/// Returns an error if the path exists, its ancestors are unsafe, or creation fails.
pub fn create_new(path: impl AsRef<Path>) -> io::Result<File> {
    open(path.as_ref(), Access::Create)
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
    let mut file = open(path.as_ref(), Access::Write)?;
    file.set_len(0)?;
    file.write_all(contents.as_ref())
}

#[cfg(test)]
mod tests {
    use super::ancestor_access;
    use std::io;
    use std::path::Path;

    const UID: u32 = 1000;
    const OTHER_UID: u32 = 1001;

    fn refused(result: io::Result<()>) -> io::Result<()> {
        let Err(error) = result else {
            return Err(io::Error::other("unsafe ancestor accepted"));
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        Ok(())
    }

    #[test]
    fn group_writable_ancestors_are_allowed() -> io::Result<()> {
        ancestor_access(Path::new("/synthetic/group-writable"), UID, 0o770, UID)?;
        ancestor_access(Path::new("/synthetic/group-readable"), UID, 0o775, UID)
    }

    #[test]
    fn non_sticky_world_writable_ancestor_is_refused() -> io::Result<()> {
        refused(ancestor_access(
            Path::new("/synthetic/world-writable"),
            UID,
            0o777,
            UID,
        ))
    }

    #[test]
    fn group_sharing_does_not_relax_ownership() -> io::Result<()> {
        refused(ancestor_access(
            Path::new("/synthetic/foreign"),
            OTHER_UID,
            0o770,
            UID,
        ))
    }

    #[test]
    fn sticky_world_writable_ancestor_is_allowed() -> io::Result<()> {
        ancestor_access(Path::new("/synthetic/sticky"), 0, 0o1777, UID)
    }
}
