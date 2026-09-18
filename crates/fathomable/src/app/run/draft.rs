// @okf-doc: /decisions/0018-comment-editor.md
//! Private scratch storage for one external-editor invocation.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fathomable_core::private_state;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// Bound retries when stale or pre-created names collide.
const ATTEMPTS: usize = 128;

#[derive(Debug)]
pub(super) struct Draft {
    directory: PathBuf,
    path: PathBuf,
    closed: bool,
}

impl Draft {
    pub(super) fn new(text: &str) -> io::Result<Self> {
        Self::in_directory(&std::env::temp_dir(), text)
    }

    fn in_directory(parent: &Path, text: &str) -> io::Result<Self> {
        for _ in 0..ATTEMPTS {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!(
                "fathomable-draft-{}-{sequence}",
                std::process::id()
            ));
            match Self::create(directory, text) {
                Ok(draft) => return Ok(draft),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate a fresh private editor directory",
        ))
    }

    fn create(directory: PathBuf, text: &str) -> io::Result<Self> {
        // Exclusivity and permissions, not secrecy of the name, protect it.
        private_state::create_dir(&directory)?;
        let draft = Self {
            path: directory.join("comment.md"),
            directory,
            closed: false,
        };
        private_state::ensure_dir(&draft.directory)?;
        private_state::create_new(&draft.path)?.write_all(text.as_bytes())?;
        Ok(draft)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn close(mut self) -> io::Result<()> {
        let result = fs::remove_dir_all(&self.directory);
        self.closed = true;
        result
    }
}

impl Drop for Draft {
    fn drop(&mut self) {
        if !self.closed
            && let Err(error) = fs::remove_dir_all(&self.directory)
        {
            tracing::warn!(%error, "cannot remove private editor scratch directory");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;

    use anyhow::Context;
    use fathomable_testing::TempDir;

    use super::{Draft, fs, io};

    #[test]
    fn private_under_permissive_umasks() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var_os("FATHOMABLE_DRAFT_MODE_PROBE").is_none() {
            for mask in ["000", "022", "077"] {
                let output = Command::new("sh")
                    .args([
                        "-c",
                        "umask \"$1\"; shift; exec \"$@\"",
                        "draft-modes",
                        mask,
                    ])
                    .arg(std::env::current_exe()?)
                    .args([
                        "--exact",
                        "app::run::draft::tests::private_under_permissive_umasks",
                        "--nocapture",
                    ])
                    .env("FATHOMABLE_DRAFT_MODE_PROBE", "1")
                    .output()?;
                assert!(
                    output.status.success(),
                    "umask {mask}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                    "the child must execute the permission assertions"
                );
            }
            return Ok(());
        }
        let parent = TempDir::new("private-draft-modes")?;
        fs::set_permissions(&parent.0, fs::Permissions::from_mode(0o755))?;
        let draft = Draft::in_directory(&parent.0, "synthetic private draft")?;
        assert_eq!(
            fs::metadata(&draft.directory)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(draft.path())?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(draft.path())?, "synthetic private draft");
        draft.close()?;
        assert_eq!(fs::read_dir(&parent.0)?.count(), 0);
        Ok(())
    }

    #[test]
    fn existing_directories_and_symlinks_are_not_reused() -> anyhow::Result<()> {
        let parent = TempDir::new("private-draft-collisions")?;
        let directory = parent.0.join("existing");
        fs::create_dir(&directory)?;
        fs::write(directory.join("comment.md"), "keep this")?;
        let error = Draft::create(directory.clone(), "replacement")
            .err()
            .context("an existing directory must be refused")?;
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        let link = parent.0.join("link");
        symlink(&directory, &link)?;
        let error = Draft::create(link, "replacement")
            .err()
            .context("an existing symlink must be refused")?;
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(directory.join("comment.md"))?,
            "keep this"
        );
        Ok(())
    }

    #[test]
    fn separate_invocations_keep_independent_drafts() -> io::Result<()> {
        let parent = TempDir::new("private-draft-invocations")?;
        let first = Draft::in_directory(&parent.0, "first draft")?;
        let second = Draft::in_directory(&parent.0, "second draft")?;
        assert_ne!(first.path(), second.path());
        assert_eq!(fs::read_to_string(first.path())?, "first draft");
        first.close()?;
        assert_eq!(fs::read_to_string(second.path())?, "second draft");
        second.close()?;
        assert_eq!(fs::read_dir(&parent.0)?.count(), 0);
        Ok(())
    }

    #[test]
    fn unsafe_temporary_parent_is_refused_before_creating_a_draft() -> anyhow::Result<()> {
        let parent = TempDir::new("private-draft-unsafe-parent")?;
        fs::set_permissions(&parent.0, fs::Permissions::from_mode(0o777))?;
        let error = Draft::in_directory(&parent.0, "synthetic private draft")
            .err()
            .context("a writable non-sticky parent must be refused")?;
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_dir(&parent.0)?.count(), 0);
        let target = parent.0.join("target");
        fs::create_dir(&target)?;
        fs::set_permissions(&parent.0, fs::Permissions::from_mode(0o755))?;
        let link = parent.0.join("link");
        symlink(&target, &link)?;
        let error = Draft::in_directory(&link, "synthetic private draft")
            .err()
            .context("a symlink parent must be refused")?;
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_dir(&target)?.count(), 0);
        Ok(())
    }

    #[test]
    fn cleanup_removes_editor_backups_without_following_links() -> io::Result<()> {
        let parent = TempDir::new("private-draft-cleanup")?;
        let outside = parent.0.join("unrelated");
        fs::create_dir(&outside)?;
        fs::write(outside.join("keep"), "unrelated content")?;
        let draft = Draft::in_directory(&parent.0, "original")?;
        let directory = draft.directory.clone();
        fs::rename(draft.path(), draft.path().with_extension("md~"))?;
        fs::write(draft.path(), "replacement from editor")?;
        symlink(&outside, directory.join("link"))?;
        assert_eq!(fs::read_to_string(draft.path())?, "replacement from editor");
        draft.close()?;
        assert!(!directory.exists());
        assert_eq!(
            fs::read_to_string(outside.join("keep"))?,
            "unrelated content"
        );
        Ok(())
    }

    #[test]
    fn early_error_still_cleans_the_draft() -> anyhow::Result<()> {
        let parent = TempDir::new("private-draft-error")?;
        let result = (|| -> io::Result<()> {
            let draft = Draft::in_directory(&parent.0, "synthetic draft")?;
            fs::read_to_string(draft.directory.join("missing"))?;
            Ok(())
        })();
        assert_eq!(
            result.err().context("the read must fail")?.kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(fs::read_dir(&parent.0)?.count(), 0);
        Ok(())
    }
}
