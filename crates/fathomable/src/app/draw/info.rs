// @okf-doc: /decisions/0026-binary-files-and-file-info.md
//! The file-info pane: what the text column shows for a binary or
//! over-limit file (ADR 0026).

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use fathomable_core::config::DiffMode;
use fathomable_core::content::{self, Content};

use crate::app::App;

/// The pane's content: labelled rows, then a notice below them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Info {
    /// `(label, value)` rows, drawn with the labels right-aligned.
    pub(crate) rows: Vec<(String, String)>,
    /// Lines drawn under the rows: why the file is not shown as text,
    /// and how to change that when it can be changed.
    pub(crate) notice: Vec<String>,
}

impl App {
    /// The file-info pane for the current document, or `None` when it is
    /// text and the document itself is shown.
    #[must_use]
    pub(crate) fn info(&self) -> Option<Info> {
        let doc = self.current.and_then(|index| self.docs.get(index))?;
        let comparison_notice = doc.comparison_notice.clone();
        let (size, format_row, notice) = match doc.document.content() {
            Content::Text(text) if comparison_notice.is_none() => return None,
            Content::Text(text) => (
                text.len() as u64,
                "text".to_owned(),
                vec![
                    comparison_notice
                        .unwrap_or_else(|| "comparison content is unavailable".to_owned()),
                ],
            ),
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
        let off = self.diff_mode() == DiffMode::Off;
        let mut rows = vec![
            ("format".to_owned(), format_row),
            ("size".to_owned(), exact_size(size)),
        ];
        if off {
            rows.push(("target".to_owned(), self.comparison_target_label()));
        }
        if !off || self.displayed_target_is_working_tree() {
            rows.push(("mode".to_owned(), mode(doc.document.path())));
            if let Some(modified) = modified(doc.document.path()) {
                rows.push(("modified".to_owned(), modified));
            }
        }
        if off {
            return Some(Info { rows, notice });
        }
        let head_size = self.workspace.head_size(&doc.relative).ok().flatten();
        let git = match (self.status.get(&doc.relative), head_size) {
            (Some(entry), _) => format!("{} ({})", entry.changes(), entry.state()),
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
    Some(crate::app::draw::format_time(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing;

    #[test]
    fn sizes_read_as_deltas() {
        assert_eq!(exact_size(312), "312 B");
        assert_eq!(exact_size(1_552_850), "1.5 MiB (1552850 bytes)");
        assert_eq!(size_delta(1024, 1024), "1 KiB, unchanged");
        assert_eq!(size_delta(1024, 3072), "1 KiB → 3 KiB (+2 KiB)");
        assert_eq!(size_delta(3072, 1024), "3 KiB → 1 KiB (-2 KiB)");
    }

    #[test]
    fn off_binary_info_keeps_target_facts_without_git_or_head() -> anyhow::Result<()> {
        let dir = testing::workspace("binary-info-off", testing::README)?;
        let root = testing::root(&dir);
        std::fs::write(root.join("sentinel.bin"), b"\0TARGET_ONLY_SENTINEL")?;
        let mut app = testing::AppBuilder::new(&dir).build()?;
        app.open(Path::new("sentinel.bin"));

        let active = app.info().ok_or_else(|| anyhow::anyhow!("binary info"))?;
        assert!(active.rows.iter().any(|(label, _)| label == "git"));

        app.select_diff_mode(DiffMode::Off);
        app.settle_background();
        let off = app
            .info()
            .ok_or_else(|| anyhow::anyhow!("off binary info"))?;
        assert!(
            off.rows
                .iter()
                .any(|(label, value)| label == "target" && value == "WorkingTree")
        );
        assert!(off.rows.iter().any(|(label, _)| label == "format"));
        assert!(off.rows.iter().any(|(label, _)| label == "size"));
        assert!(off.rows.iter().any(|(label, _)| label == "mode"));
        assert!(
            off.rows
                .iter()
                .all(|(label, _)| label != "git" && label != "HEAD")
        );
        Ok(())
    }
}
