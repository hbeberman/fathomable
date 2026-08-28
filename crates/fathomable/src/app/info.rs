// @okf-doc: /decisions/0026-binary-files-and-file-info.md
//! The file-info pane: what the text column shows for a binary or
//! over-limit file (ADR 0026).

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use fathomable_core::content::{self, Content};

use super::App;

/// The pane's content: labelled rows, then a notice below them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Info {
    /// `(label, value)` rows, drawn with the labels right-aligned.
    pub rows: Vec<(String, String)>,
    /// Lines drawn under the rows: why the file is not shown as text,
    /// and how to change that when it can be changed.
    pub notice: Vec<String>,
}

impl App {
    /// The file-info pane for the current document, or `None` when it is
    /// text and the document itself is shown.
    #[must_use]
    pub fn info(&self) -> Option<Info> {
        let doc = self.current.and_then(|index| self.docs.get(index))?;
        let (size, format_row, notice) = match doc.document.content() {
            Content::Text(_) => return None,
            Content::Binary { size, format } => (
                *size,
                format.map_or_else(|| "binary data".to_owned(), |f| f.to_string()),
                vec!["Binary file: not shown as text.".to_owned()],
            ),
            Content::TooLarge { size, max_bytes } => (
                *size,
                "text, too large to view".to_owned(),
                vec![
                    format!(
                        "Too large to view: {}, limit is {}.",
                        content::human_size(*size),
                        content::human_size(*max_bytes)
                    ),
                    format!(
                        "Raise it with `viewer {{ max-file-size-mib {} }}` in {}",
                        content::suggested_max_mib(*size),
                        self.config_path.display()
                    ),
                ],
            ),
        };
        let mut rows = vec![
            ("format".to_owned(), format_row),
            ("size".to_owned(), exact_size(size)),
            ("mode".to_owned(), mode(doc.document.path())),
        ];
        if let Some(modified) = modified(doc.document.path()) {
            rows.push(("modified".to_owned(), modified));
        }
        let head_size = self.workspace.head_size(&doc.relative).ok().flatten();
        let git = match (self.status.get(&doc.relative), head_size) {
            (Some(entry), _) => {
                let staged = if entry.is_staged() { ", staged" } else { "" };
                format!("{}{staged}", entry.state())
            }
            (None, Some(_)) => "unchanged".to_owned(),
            (None, None) if self.workspace.is_git() => "not tracked".to_owned(),
            (None, None) => "no repository".to_owned(),
        };
        rows.push(("git".to_owned(), git));
        if let Some(head) = head_size {
            rows.push(("HEAD".to_owned(), size_delta(head, size)));
        }
        Some(Info { rows, notice })
    }
}

/// `1.5 MiB (1552850 bytes)`, or just `312 B` when they are the same.
fn exact_size(size: u64) -> String {
    let human = content::human_size(size);
    if size < 1024 {
        human
    } else {
        format!("{human} ({size} bytes)")
    }
}

/// `HEAD`'s size beside the working tree's: `1.4 MiB → 1.5 MiB (+120.3
/// KiB)`, or `1.5 MiB, unchanged`.
fn size_delta(head: u64, worktree: u64) -> String {
    if head == worktree {
        return format!("{}, unchanged", content::human_size(head));
    }
    let (sign, delta) = if worktree > head {
        ('+', worktree - head)
    } else {
        ('-', head - worktree)
    };
    format!(
        "{} → {} ({sign}{})",
        content::human_size(head),
        content::human_size(worktree),
        content::human_size(delta)
    )
}

/// `symlink → target`, `executable`, or `regular file`.
fn mode(path: &Path) -> String {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return "unreadable".to_owned();
    };
    if meta.is_symlink() {
        return match fs::read_link(path) {
            Ok(target) => format!("symlink → {}", target.display()),
            Err(_) => "symlink".to_owned(),
        };
    }
    if is_executable(&meta) {
        "executable".to_owned()
    } else {
        "regular file".to_owned()
    }
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

/// The modification time as UTC, when the file system reports one.
fn modified(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let secs = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(super::ui::format_time(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_deltas() {
        assert_eq!(exact_size(312), "312 B");
        assert_eq!(exact_size(1_552_850), "1.5 MiB (1552850 bytes)");
        assert_eq!(size_delta(1024, 1024), "1 KiB, unchanged");
        assert_eq!(size_delta(1024, 3072), "1 KiB → 3 KiB (+2 KiB)");
        assert_eq!(size_delta(3072, 1024), "3 KiB → 1 KiB (-2 KiB)");
    }
}
