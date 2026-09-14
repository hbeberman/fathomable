// @okf-doc: /decisions/0069-the-diffs-keys-on-the-bar.md
//! The diff's keys (ADR 0069): the hints the text's key bar carries
//! while a diff is on screen, and `D`, which steps the diff along the
//! file's row of pairs: the file itself, `HEAD · now`, `last seen ·
//! now`, the newest checkpoint against the working file, and the file
//! again. The diff header is its words alone; every key is on the bar.

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
        HintOf::keyed(place, Action::DiffNext, "next base"),
        HintOf::keyed(place, Action::DiffWhitespace, "whitespace"),
        HintOf::keyed(place, Action::Escape, "close"),
    ]);
    hints
}

impl App {
    /// `D`: the next pair along the file's row of diffs, `HEAD`, last
    /// seen, and the newest checkpoint, each against the working file
    /// and each skipped when the file cannot show it; after the last,
    /// the file. From a pair off the row (a commit base, a picked
    /// target) the file, so the next press starts at `HEAD`.
    pub(crate) fn diff_next(&mut self) {
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        let mut row = Vec::new();
        if self.view().has_head() {
            row.push(Side::Head);
        }
        if self.view().has_seen() {
            row.push(Side::Seen);
        }
        let count = self.timeline_len();
        if count > 0 {
            row.push(Side::Checkpoint(count - 1));
        }
        let shown = self
            .view()
            .diff()
            .map(|d| (d.base.clone(), d.target.clone()));
        let at = match shown {
            None => None,
            Some((base, Side::Working)) if row.contains(&base) => {
                row.iter().position(|s| *s == base)
            }
            Some(_) => {
                self.leave_diff();
                return;
            }
        };
        let next = at.map_or(0, |at| at + 1);
        match row.get(next).cloned() {
            Some(base) => self.show_diff(base, Side::Working),
            None if at.is_some() => self.leave_diff(),
            None => self.notice("no HEAD, last seen, or checkpoint to diff against"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fathomable_core::checkpoints::Store;
    use fathomable_core::workspace::Workspace;
    use fathomable_testing::{TempDir, git};

    use crate::app::diff::{DiffView, Side};
    use crate::app::testing::{press, screen};
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

    /// `D` walks the file, `HEAD · now`, `last seen · now`, the newest
    /// checkpoint, and the file again, skipping what the file lacks;
    /// from a pair off the row it returns to the file (ADR 0069).
    #[test]
    fn d_steps_the_diffs_and_skips_what_is_missing() -> anyhow::Result<()> {
        let dir = TempDir::new("diff-next")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("a.md", "one\n")])?;
        git::commit_and_stage(&dir.0, &[("a.md", "two\n")])?;
        fs::write(dir.0.join("a.md"), "three\n")?;
        let options = Options {
            checkpoints: Some(Store::open(&dir.0.join(".state"))?),
            ..Options::for_test(dir.0.clone())
        };
        let mut app = App::new(Workspace::discover(&dir.0)?, 100, 30, options);
        app.open(Path::new("a.md"));

        // No snapshot, no checkpoint: HEAD, then the file.
        press(&mut app, "D");
        assert_eq!(header(&app), "HEAD · now");
        press(&mut app, "D");
        assert!(!app.view().diff_view(), "after the last pair, the file");

        // A snapshot and a checkpoint join the row.
        app.view_mut()
            .set_bases(Some("one\n".to_owned()), None, Some("two\n".to_owned()));
        app.checkpoint_file();
        press(&mut app, "D");
        assert_eq!(header(&app), "HEAD · now");
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

        // From a pair off the row, the file; then HEAD again.
        app.show_diff(Side::Head, Side::Head);
        press(&mut app, "D");
        assert!(!app.view().diff_view(), "off the row: back to the file");
        press(&mut app, "D");
        assert_eq!(header(&app), "HEAD · now");
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
        assert!(!app.text_bar_shown(), "no diff, no thread: no bar");

        press(&mut app, " dd");
        assert!(app.text_bar_shown(), "a diff is something to say");
        assert_eq!(
            bar(&app)?,
            "b base · t target · D next base · w whitespace · Esc close",
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
}
