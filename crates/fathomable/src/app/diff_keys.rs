// @okf-doc: /decisions/0069-the-diffs-keys-on-the-bar.md
//! The diff's keys (ADR 0069): the hints the text's key bar carries
//! while a diff is on screen, and `D`, which steps the diff along the
//! file's row of pairs: the file itself, its unstaged and staged Git
//! layers, `last seen · now`, the newest checkpoint against the working
//! file, and the file again. The aggregate `HEAD · now` pair remains an
//! explicit `Space d d` action. The diff header is its words alone;
//! every key is on the bar.

use super::App;
use super::diff::Side;
use super::draw::header::HintOf;
use super::input::bindings::{Action, Where};

/// The diff's hints for the bar: `h/l page` on a checkpoint base only
/// (ADR 0064), then the side pickers, the next pair, whitespace, and
/// close. Empty outside a diff.
pub(crate) fn diff_hints(app: &App) -> Vec<HintOf> {
    let Some(diff) = app.view().diff() else {
        return Vec::new();
    };
    let place = Where::View;
    let mut hints = Vec::new();
    if diff.on_checkpoint() {
        hints.push(HintOf::paired(
            place,
            Action::MoveLeft,
            Action::MoveRight,
            "page",
        ));
    }
    hints.extend([
        HintOf::keyed(place, Action::DiffBase, "base"),
        HintOf::keyed(place, Action::DiffTarget, "target"),
        HintOf::keyed(place, Action::DiffNext, "next diff"),
        HintOf::keyed(place, Action::DiffWhitespace, "whitespace"),
        HintOf::keyed(place, Action::Escape, "close"),
    ]);
    hints
}

impl App {
    /// `D`: the next pair along the file's row of diffs: unstaged,
    /// staged, last seen, and the newest checkpoint, skipping unavailable
    /// pairs; after the last, the file. From a pair off the row, including
    /// the explicit net diff, return to the file.
    pub(crate) fn diff_next(&mut self) {
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        let mut row = Vec::new();
        if let Some(entry) = self.status().get(self.current_path()) {
            if entry.unstaged_state().is_some() && self.view().has_index() {
                row.push((Side::Index, Side::Working));
            }
            if entry.staged_state().is_some() && self.view().has_head() && self.view().has_index() {
                row.push((Side::Head, Side::Index));
            }
        }
        if self.view().has_seen() {
            row.push((Side::Seen, Side::Working));
        }
        let count = self.timeline_len();
        if count > 0 {
            row.push((Side::Checkpoint(count - 1), Side::Working));
        }
        let shown = self
            .view()
            .diff()
            .map(|d| (d.base.clone(), d.target.clone()));
        let at = match shown {
            None => None,
            Some(pair) if row.contains(&pair) => {
                row.iter().position(|candidate| *candidate == pair)
            }
            Some(_) => {
                self.leave_diff();
                return;
            }
        };
        let next = at.map_or(0, |at| at + 1);
        match row.get(next).cloned() {
            Some((base, target)) => self.show_diff(base, target),
            None if at.is_some() => self.leave_diff(),
            None => self.notice("no staged, unstaged, last-seen, or checkpoint diff"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crossterm::event::KeyCode;
    use fathomable_core::checkpoints::Store;
    use fathomable_core::workspace::Workspace;
    use fathomable_testing::{TempDir, git};

    use crate::app::diff::{DiffView, Side};
    use crate::app::testing::{press, press_key, screen};
    use crate::app::{App, Options};

    fn header(app: &App) -> String {
        app.view()
            .diff()
            .map(|d| d.header.clone())
            .unwrap_or_default()
    }

    /// The bar's row on the 100×30 test screen, past the sidebar.
    fn bar(app: &App) -> anyhow::Result<String> {
        Ok(screen(app)?[app.text_bar_row()]
            .chars()
            .skip(app.sidebar_width())
            .collect::<String>()
            .trim()
            .to_owned())
    }

    /// `D` walks the file, unstaged, staged, last seen, the newest
    /// checkpoint, and the file again, skipping what the file lacks;
    /// from a pair off the row it returns to the file (ADR 0069).
    #[test]
    fn d_steps_the_diffs_and_skips_what_is_missing() -> anyhow::Result<()> {
        let dir = TempDir::new("diff-next")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("a.md", "one\n")])?;
        git::stage(&dir.0, &[("a.md", "two\n")])?;
        fs::write(dir.0.join("a.md"), "three\n")?;
        let options = Options {
            checkpoints: Some(Store::open(&dir.0.join(".state"))?),
            ..Options::for_test(dir.0.clone())
        };
        let mut app = App::new(Workspace::discover(&dir.0)?, 100, 30, options);
        app.open(Path::new("a.md"));
        app.settle_status();

        // No snapshot or checkpoint: unstaged, staged, then the file.
        press(&mut app, "D");
        assert_eq!(header(&app), "INDEX · now");
        press(&mut app, "D");
        assert_eq!(header(&app), "HEAD · INDEX");
        press(&mut app, "D");
        assert!(!app.view().diff_view(), "after the last pair, the file");

        // A snapshot and a checkpoint join the row.
        app.view_mut().set_bases(
            Some("one\n".to_owned()),
            Some("two\n".to_owned()),
            Some("one\n".to_owned()),
        );
        app.checkpoint_file();
        press(&mut app, "D");
        assert_eq!(header(&app), "INDEX · now");
        press(&mut app, "D");
        assert_eq!(header(&app), "HEAD · INDEX");
        press(&mut app, "D");
        assert_eq!(header(&app), "last seen · now");
        press(&mut app, "D");
        assert!(
            header(&app).starts_with("checkpoint 1/1"),
            "{}",
            header(&app)
        );
        assert_eq!(app.view().diff_base(), Some(&Side::Checkpoint(0)));
        press(&mut app, "D");
        assert!(!app.view().diff_view());

        // From a pair off the row, the file; then the relevant layer again.
        app.show_diff(Side::Head, Side::Head);
        press(&mut app, "D");
        assert!(!app.view().diff_view(), "off the row: back to the file");
        press(&mut app, "D");
        assert_eq!(header(&app), "INDEX · now");
        Ok(())
    }

    /// The diff's keys sit on the bar with the diff header words alone;
    /// `h/l page` shows on a checkpoint base only; another pane's focus
    /// leaves the focus tip (ADR 0069, ADR 0064).
    #[test]
    fn the_bar_carries_the_diffs_keys() -> anyhow::Result<()> {
        let dir = TempDir::new("diff-bar")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("a.md", "one\n")])?;
        fs::write(dir.0.join("a.md"), "two\n")?;
        // No toasts: the checkpoint's would paint over the bar.
        let mut options = Options {
            checkpoints: Some(Store::open(&dir.0.join(".state"))?),
            ..Options::for_test(dir.0.clone())
        };
        options.jump.toast = std::time::Duration::ZERO;
        let mut app = App::new(Workspace::discover(&dir.0)?, 100, 30, options);
        app.open(Path::new("a.md"));
        app.settle_status();
        assert!(!app.text_bar_shown(), "no diff, no thread: no bar");

        press(&mut app, " dd");
        assert!(app.text_bar_shown(), "a diff is something to say");
        assert_eq!(
            bar(&app)?,
            "b base · t target · D next diff · w whitespace · Esc close",
            "no paging on a HEAD base"
        );
        let rows = screen(&app)?;
        let header = rows
            .iter()
            .find(|row| row.contains("HEAD · now"))
            .ok_or_else(|| anyhow::anyhow!("no header: {rows:?}"))?;
        assert!(
            !header.contains("base"),
            "the header is its words alone: {header}"
        );

        app.checkpoint_file();
        app.leave_diff();
        press(&mut app, "D");
        press(&mut app, "D");
        assert!(app.view().diff().is_some_and(DiffView::on_checkpoint));
        assert!(
            bar(&app)?.starts_with("h/l page · b base"),
            "paging on a checkpoint base: {}",
            bar(&app)?
        );

        app.toggle_tree_focus();
        assert_eq!(bar(&app)?, "click or Space w l to focus");
        Ok(())
    }

    #[test]
    fn deleted_file_uses_git_snapshots_and_cycles_its_layers() -> anyhow::Result<()> {
        let dir = TempDir::new("diff-deleted-layers")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("a.md", "from HEAD\n")])?;
        git::stage(&dir.0, &[("a.md", "from index\n")])?;
        let mut app = App::new(
            Workspace::discover(&dir.0)?,
            100,
            30,
            Options::for_test(dir.0.clone()),
        );
        app.settle_status();
        app.open(Path::new("a.md"));

        assert_eq!(app.view().text(), "from index\n");
        assert_eq!(app.banner(), Some("deleted from worktree · showing INDEX"));
        press(&mut app, "/index");
        press_key(&mut app, KeyCode::Enter);
        assert_eq!(
            app.view().matches().len(),
            1,
            "tombstone source is searchable"
        );

        press(&mut app, "D");
        assert_eq!(header(&app), "INDEX · WORKTREE (missing)");
        assert_eq!(
            app.view().diff().map(|diff| diff.badge.as_str()),
            Some("DIFF unstaged")
        );
        press(&mut app, "D");
        assert_eq!(header(&app), "HEAD · INDEX");
        assert_eq!(
            app.view().diff().map(|diff| diff.badge.as_str()),
            Some("DIFF staged")
        );
        press(&mut app, "D");
        assert!(!app.view().diff_view());

        git::stage(&dir.0, &[])?;
        app.on_events(vec![crate::app::watch::Event::Change(
            dir.0.join(".git/index"),
        )]);
        app.settle_status();
        assert_eq!(app.view().text(), "from HEAD\n");
        assert_eq!(app.banner(), Some("staged deletion · showing HEAD"));
        press(&mut app, "D");
        assert_eq!(header(&app), "HEAD · INDEX (missing)");
        Ok(())
    }
}
