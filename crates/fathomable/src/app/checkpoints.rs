//! Checkpoints (ADR 0049): `Space v c` marks the current file's content on
//! its timeline, `Space v C` marks every non-ignored file whose content
//! moved since its last checkpoint. Both go through the one store in
//! [`fathomable_core::checkpoints`]; a toast counts what was stored.
//!
//! `Space v r` opens the checkpoint diff: a unified diff between two
//! [`Side`]s of the current file, opened on the newest pair (latest
//! checkpoint to the working file). `h`/`l` page along the timeline, `b`
//! and `t` pick either side from the file's checkpoints, the commits that
//! touched it, `HEAD`, and the working file; `Space v g` picks a commit
//! for the base with the working file as the target.

use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::checkpoints::Origin;
use fathomable_core::content;
use fathomable_core::workspace::Filter;

use super::draw::format_age;
use super::{App, PickerKind};
use fathomable_core::clock::now;

/// Commits touching the file the pickers list, at most (ADR 0049).
const HISTORY_LIMIT: usize = 50;

/// One side of the checkpoint diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Side {
    /// The file's checkpoint at this index on its timeline, oldest first.
    Checkpoint(usize),
    /// The file as committed at this commit (full hex).
    Commit(String),
    /// The file as committed at `HEAD`.
    Head,
    /// The working file, as the view shows it.
    Working,
}

/// What the checkpoint view lays out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckBody {
    /// A unified diff from `base` to `target`; `None` is the working text.
    Diff {
        base: String,
        target: Option<String>,
    },
    /// One line saying why there is no diff.
    Notice(String),
}

/// The checkpoint diff a view shows (ADR 0049).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckDiff {
    pub(crate) base: Side,
    pub(crate) target: Side,
    /// `checkpoint 2/3  5m ago · now`, or the two labels when the sides
    /// are not a neighbouring pair of the timeline.
    pub(crate) header: String,
    pub(crate) body: CheckBody,
}

/// One entry of the strip along the view's bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StripEntry {
    pub(crate) label: String,
    pub(crate) workspace: bool,
    /// Whether the checkpoint is one of the two sides shown.
    pub(crate) shown: bool,
}

impl App {
    /// `Space v r`: the checkpoint diff of the current file on its newest
    /// pair, or back to the rendered view when it is shown.
    pub(crate) fn toggle_checkpoint_view(&mut self) {
        if self.view().checkpoint_view() {
            self.view_mut().leave_checkpoint();
            self.relayout();
            return;
        }
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        self.show_newest_pair();
    }

    /// The latest checkpoint against the working file, or the notice that
    /// there is none.
    fn show_newest_pair(&mut self) {
        let count = self.timeline_len();
        if count == 0 {
            self.show_checkpoint(Side::Working, Side::Working);
        } else {
            self.show_checkpoint(Side::Checkpoint(count - 1), Side::Working);
        }
    }

    /// `h`/`l` in the checkpoint view: the earlier or later neighbouring
    /// pair along the timeline.
    pub(crate) fn checkpoint_page(&mut self, delta: isize) {
        let Some(check) = self.view().checkpoint() else {
            return;
        };
        let count = self.timeline_len();
        let Side::Checkpoint(index) = check.base else {
            self.notice(if count == 0 {
                "no checkpoint of this file yet; Space v c makes one"
            } else {
                "pick a checkpoint as the base (b) to page the timeline"
            });
            return;
        };
        let Some(next) = index.checked_add_signed(delta).filter(|&i| i < count) else {
            self.notice(if delta < 0 {
                "at the first checkpoint"
            } else {
                "at the newest pair"
            });
            return;
        };
        let target = if next + 1 < count {
            Side::Checkpoint(next + 1)
        } else {
            Side::Working
        };
        self.show_checkpoint(Side::Checkpoint(next), target);
    }

    /// `b` / `t` in the checkpoint view: a picker for the base or the
    /// target side.
    pub(crate) fn pick_checkpoint_side(&mut self, target: bool) {
        if !self.view().checkpoint_view() {
            self.notice("not in the checkpoint view; Space v r opens it");
            return;
        }
        self.check_target_next = None;
        self.open_picker(if target {
            PickerKind::CheckTarget
        } else {
            PickerKind::CheckBase
        });
    }

    /// `Space v g`: pick a commit (or anything else) as the base, with the
    /// working file as the target.
    pub(crate) fn checkpoint_against_commit(&mut self) {
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        self.check_target_next = Some(Side::Working);
        self.open_picker(PickerKind::CheckBase);
    }

    /// The items of the side pickers, remembering which side each names.
    pub(super) fn checkpoint_choices(&mut self) -> Vec<String> {
        let relative = self.current_path().to_path_buf();
        let at = now();
        let mut choices = Vec::new();
        let timeline: Vec<(u64, bool)> = self
            .checkpoints
            .as_ref()
            .map(|store| {
                store
                    .timeline(&relative)
                    .iter()
                    .map(|c| (c.created(), c.is_workspace()))
                    .collect()
            })
            .unwrap_or_default();
        for (index, (created, workspace)) in timeline.iter().enumerate().rev() {
            let mark = if *workspace { " ◆" } else { "" };
            choices.push((
                format!(
                    "checkpoint {}  {}{mark}",
                    index + 1,
                    format_age(*created, at)
                ),
                Side::Checkpoint(index),
            ));
        }
        choices.push(("working file  now".to_owned(), Side::Working));
        if self.workspace.is_git() {
            choices.push(("HEAD".to_owned(), Side::Head));
            for commit in self.workspace.file_history(&relative, HISTORY_LIMIT) {
                choices.push((
                    format!(
                        "{}  {}  {}",
                        commit.short(),
                        format_age(commit.time(), at),
                        commit.subject()
                    ),
                    Side::Commit(commit.hex().to_owned()),
                ));
            }
        }
        let items = choices.iter().map(|(item, _)| item.clone()).collect();
        self.check_choices = choices;
        items
    }

    /// The picker's choice for a side: show the diff with it.
    pub(super) fn choose_checkpoint_side(&mut self, kind: PickerKind, item: &str) {
        let Some(side) = self
            .check_choices
            .iter()
            .find(|(text, _)| text == item)
            .map(|(_, side)| side.clone())
        else {
            return;
        };
        let current = self
            .view()
            .checkpoint()
            .map(|c| (c.base.clone(), c.target.clone()));
        if kind == PickerKind::CheckTarget {
            let base = current.map_or(Side::Working, |(base, _)| base);
            self.show_checkpoint(base, side);
        } else {
            let target = self
                .check_target_next
                .take()
                .or_else(|| current.map(|(_, target)| target))
                .unwrap_or(Side::Working);
            self.show_checkpoint(side, target);
        }
    }

    /// The strip along the checkpoint view's bottom: the file's
    /// checkpoints, oldest first.
    pub(crate) fn checkpoint_strip(&self) -> Vec<StripEntry> {
        let relative = self.current_path();
        let sides = self.view().checkpoint().map(|c| (&c.base, &c.target));
        let at = now();
        self.checkpoints
            .as_ref()
            .map(|store| store.timeline(relative))
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, c)| StripEntry {
                label: format_age(c.created(), at),
                workspace: c.is_workspace(),
                shown: sides.is_some_and(|(base, target)| {
                    *base == Side::Checkpoint(index) || *target == Side::Checkpoint(index)
                }),
            })
            .collect()
    }

    /// Whether the checkpoint view's header and strip take rows over and
    /// under the text.
    pub(crate) fn checkpoint_chrome(&self) -> bool {
        self.has_document()
            && !self.review_list().is_open()
            && self.info().is_none()
            && self.view().checkpoint_view()
    }

    fn timeline_len(&self) -> usize {
        let relative = self.current_path();
        self.checkpoints
            .as_ref()
            .map_or(0, |store| store.timeline(relative).len())
    }

    /// Lay the diff between `base` and `target` in the current view.
    fn show_checkpoint(&mut self, base: Side, target: Side) {
        let relative = self.current_path().to_path_buf();
        let count = self.timeline_len();
        let body = match (&base, &target) {
            (Side::Working, Side::Working) if count == 0 => CheckBody::Notice(
                "no checkpoint of this file yet; Space v c makes one, b picks another base"
                    .to_owned(),
            ),
            _ => match (
                self.side_text(&base, &relative),
                self.side_text(&target, &relative),
            ) {
                (Ok(base_text), Ok(target_text)) => CheckBody::Diff {
                    base: base_text.unwrap_or_else(|| self.view().text().to_owned()),
                    target: target_text,
                },
                (Err(error), _) | (_, Err(error)) => CheckBody::Notice(error),
            },
        };
        let base_label = self.side_label(&base, &relative);
        let target_label = self.side_label(&target, &relative);
        let header = match (&base, &target) {
            (Side::Checkpoint(i), Side::Checkpoint(j)) if *j == i + 1 => {
                format!(
                    "checkpoint {}/{count}  {base_label} · {target_label}",
                    i + 1
                )
            }
            (Side::Checkpoint(i), Side::Working) if i + 1 == count => {
                format!("checkpoint {count}/{count}  {base_label} · {target_label}")
            }
            _ => format!("{base_label} · {target_label}"),
        };
        let was_shown = self.view().checkpoint_view();
        self.view_mut().show_checkpoint(CheckDiff {
            base,
            target,
            header,
            body,
        });
        if !was_shown {
            self.relayout();
        }
    }

    /// The text of `side` for `relative`: `None` means the working text.
    fn side_text(&self, side: &Side, relative: &Path) -> Result<Option<String>, String> {
        match side {
            Side::Working => Ok(None),
            Side::Checkpoint(index) => {
                let store = self
                    .checkpoints
                    .as_ref()
                    .ok_or_else(|| "checkpoints unavailable".to_owned())?;
                let checkpoint = store
                    .timeline(relative)
                    .get(*index)
                    .ok_or_else(|| "that checkpoint is gone".to_owned())?;
                store
                    .text(checkpoint)
                    .map(Some)
                    .map_err(|error| format!("cannot read the checkpoint: {error}"))
            }
            Side::Head => self
                .workspace
                .head_text(relative)
                .map_err(|error| error.to_string())
                .and_then(|text| text.ok_or_else(|| "no HEAD: not in a git repository".to_owned()))
                .map(Some),
            Side::Commit(hex) => self
                .workspace
                .text_at(hex, relative)
                .map_err(|error| error.to_string())
                .and_then(|text| text.ok_or_else(|| "not in a git repository".to_owned()))
                .map(Some),
        }
    }

    fn side_label(&self, side: &Side, relative: &Path) -> String {
        match side {
            Side::Working => "now".to_owned(),
            Side::Head => "HEAD".to_owned(),
            Side::Commit(hex) => hex[..hex.len().min(7)].to_owned(),
            Side::Checkpoint(index) => self
                .checkpoints
                .as_ref()
                .and_then(|store| store.timeline(relative).get(*index))
                .map_or_else(
                    || "gone".to_owned(),
                    |c| {
                        let mark = if c.is_workspace() { " ◆" } else { "" };
                        format!("{}{mark}", format_age(c.created(), now()))
                    },
                ),
        }
    }

    /// `Space v c`: checkpoint the current file.
    pub(crate) fn checkpoint_file(&mut self) {
        let Some(doc) = self.current.and_then(|i| self.docs.get(i)) else {
            self.notice("no file open to checkpoint");
            return;
        };
        let Some(text) = doc.document.text().map(str::to_owned) else {
            self.notice("only text files are checkpointed");
            return;
        };
        let path = doc.relative.clone();
        self.record_checkpoint(Origin::File, &[(path, text)]);
    }

    /// `Space v C`: checkpoint every non-ignored text file whose content
    /// differs from its latest checkpoint, or has none.
    pub(crate) fn checkpoint_workspace(&mut self) {
        if self.checkpoints.is_none() {
            self.notice("checkpoints unavailable; see the log");
            return;
        }
        let max_bytes = self.viewer.max_file_bytes();
        let root = self.workspace.root().to_path_buf();
        let files: Vec<(PathBuf, String)> = self
            .workspace
            .walk_files(Filter::Visible)
            .into_iter()
            .map(PathBuf::from)
            .filter_map(|relative| {
                let bytes = fs::read(root.join(&relative)).ok()?;
                if u64::try_from(bytes.len()).is_ok_and(|len| len > max_bytes)
                    || content::is_binary(&bytes)
                {
                    return None;
                }
                let text = String::from_utf8(bytes).ok()?;
                Some((relative, text))
            })
            .collect();
        self.record_checkpoint(Origin::Workspace, &files);
    }

    fn record_checkpoint(&mut self, origin: Origin, files: &[(PathBuf, String)]) {
        let Some(store) = self.checkpoints.as_mut() else {
            self.notice("checkpoints unavailable; see the log");
            return;
        };
        let pairs = files
            .iter()
            .map(|(path, text)| (path.as_path(), text.as_str()));
        match store.record(origin, pairs) {
            Ok(0) => self.push_toast("checkpoint: nothing changed".to_owned()),
            Ok(stored) => {
                self.push_toast(if stored == 1 {
                    "checkpoint: 1 file".to_owned()
                } else {
                    format!("checkpoint: {stored} files")
                });
                // The open checkpoint view counts its timeline afresh; one
                // that had nothing to show moves to the new pair.
                if let Some((base, target)) = self
                    .view()
                    .checkpoint()
                    .map(|c| (c.base.clone(), c.target.clone()))
                {
                    if base == Side::Working && target == Side::Working {
                        self.show_newest_pair();
                    } else {
                        self.show_checkpoint(base, target);
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "cannot write a checkpoint");
                self.notice(format!("cannot checkpoint: {error}"));
            }
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

    use crate::app::testing::press;

    use crate::app::{App, Options, PickerKind, Popup};

    fn last_toast(app: &App) -> String {
        app.toasts()
            .last()
            .map(|t| t.text().to_owned())
            .unwrap_or_default()
    }

    /// `Space v C` stores every text file once, then only the files whose
    /// content moved; `Space v c` appends to the same timeline.
    #[test]
    fn workspace_checkpoints_skip_unchanged_files() -> anyhow::Result<()> {
        let dir = TempDir::new("app-checkpoints")?;
        fs::create_dir_all(dir.0.join("ws/docs"))?;
        fs::write(dir.0.join("ws/README.md"), "# Readme\n")?;
        fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n")?;
        fs::write(dir.0.join("ws/logo.bin"), b"\0\x01\x02")?;
        let store = || Store::open(&dir.0.join("state"));
        let options = Options {
            checkpoints: Some(store()?),
            ..Options::for_test(dir.0.join("ws"))
        };
        let mut app = App::new(Workspace::discover(dir.0.join("ws"))?, 100, 30, options);

        press(&mut app, " vC");
        assert_eq!(
            last_toast(&app),
            "checkpoint: 2 files",
            "binary files are skipped"
        );
        press(&mut app, " vC");
        assert_eq!(last_toast(&app), "checkpoint: nothing changed");

        fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n\nmore\n")?;
        press(&mut app, " vC");
        assert_eq!(last_toast(&app), "checkpoint: 1 file");

        app.open(Path::new("README.md"));
        press(&mut app, " vc");
        assert_eq!(
            last_toast(&app),
            "checkpoint: nothing changed",
            "the file checkpoint follows the same rule"
        );
        let readme = dir.0.join("ws/README.md");
        fs::write(&readme, "# Readme\n\nedited\n")?;
        app.on_changes(vec![readme]);
        press(&mut app, " vc");
        assert_eq!(last_toast(&app), "checkpoint: 1 file");

        let store = store()?;
        assert_eq!(store.events(), 3);
        let readme = store.timeline(Path::new("README.md"));
        assert_eq!(readme.len(), 2);
        assert!(readme[0].is_workspace());
        assert!(!readme[1].is_workspace());
        assert_eq!(store.text(&readme[1])?, "# Readme\n\nedited\n");
        assert_eq!(store.timeline(Path::new("docs/guide.md")).len(), 2);
        assert!(store.timeline(Path::new("logo.bin")).is_empty());
        Ok(())
    }

    fn header(app: &App) -> String {
        app.view()
            .checkpoint()
            .map(|c| c.header.clone())
            .unwrap_or_default()
    }

    fn picker_items(app: &App) -> Vec<String> {
        match app.popup() {
            Some(Popup::Picker(picker)) => picker
                .matches()
                .iter()
                .map(|m| picker.item(m).to_owned())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `Space v r` opens the newest pair and `h`/`l` page the timeline;
    /// `b`/`t` pick any side, commits included; `Space v g` fixes the
    /// target at the working file; a record refreshes the open view.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one walk through the whole view")]
    fn checkpoint_view_pages_and_picks_sides() -> anyhow::Result<()> {
        let dir = TempDir::new("app-checkpoint-view")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("a.md", "one\n"), ("b.md", "x\n")])?;
        git::commit_and_stage(&dir.0, &[("a.md", "two\n"), ("b.md", "x\n")])?;
        fs::write(dir.0.join("a.md"), "three\n")?;
        let options = Options {
            checkpoints: Some(Store::open(&dir.0.join(".state"))?),
            ..Options::for_test(dir.0.clone())
        };
        let mut app = App::new(Workspace::discover(&dir.0)?, 100, 30, options);
        app.open(Path::new("a.md"));

        press(&mut app, " vr");
        assert!(app.view().checkpoint_view());
        assert_eq!(header(&app), "now · now");
        assert!(
            app.view().layout().lines()[0]
                .text()
                .contains("no checkpoint of this file yet"),
            "{}",
            app.view().layout().lines()[0].text()
        );
        assert_eq!(
            app.text_rows(),
            30 - 1 - 2,
            "header and strip take two rows"
        );
        press(&mut app, " vc");
        assert_eq!(
            header(&app),
            "checkpoint 1/1  just now · now",
            "the first checkpoint moves the empty view to the new pair"
        );
        press(&mut app, " vr");
        assert!(!app.view().checkpoint_view());
        assert_eq!(app.text_rows(), 29);

        fs::write(dir.0.join("a.md"), "four\n")?;
        app.on_changes(vec![dir.0.join("a.md")]);
        press(&mut app, " vc");
        press(&mut app, " vr");
        assert_eq!(header(&app), "checkpoint 2/2  just now · now");
        assert_eq!(
            app.view().checkpoint_counts(),
            Some((0, 0)),
            "the latest checkpoint is the working file"
        );
        press(&mut app, "h");
        assert_eq!(header(&app), "checkpoint 1/2  just now · just now");
        assert_eq!(app.view().checkpoint_counts(), Some((1, 1)));
        let shown: Vec<String> = app
            .view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect();
        assert!(shown.iter().any(|l| l.contains("-three")), "{shown:?}");
        assert!(shown.iter().any(|l| l.contains("+four")), "{shown:?}");
        press(&mut app, "h");
        assert_eq!(app.message(), Some("at the first checkpoint"));
        press(&mut app, "ll");
        assert_eq!(app.message(), Some("at the newest pair"));
        assert_eq!(header(&app), "checkpoint 2/2  just now · now");
        let strip = app.checkpoint_strip();
        assert_eq!(strip.len(), 2);
        assert!(!strip[0].shown && strip[1].shown);

        press(&mut app, "b");
        let items = picker_items(&app);
        assert_eq!(
            items.len(),
            2 + 2 + 2,
            "two checkpoints, working, HEAD, two commits: {items:?}"
        );
        assert!(items[0].starts_with("checkpoint 2"));
        assert_eq!(items[2], "working file  now");
        assert_eq!(items[3], "HEAD");
        assert!(items[4].ends_with("  commit"), "{}", items[4]);
        for ch in "HEAD".chars() {
            app.picker_char(ch);
        }
        app.picker_confirm();
        assert_eq!(header(&app), "HEAD · now");
        let shown: Vec<String> = app
            .view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect();
        assert!(shown.iter().any(|l| l.contains("-two")), "{shown:?}");
        press(&mut app, "h");
        assert_eq!(
            app.message(),
            Some("pick a checkpoint as the base (b) to page the timeline")
        );

        press(&mut app, "t");
        assert!(
            matches!(app.popup(), Some(Popup::Picker(p)) if p.kind() == PickerKind::CheckTarget)
        );
        for ch in "checkpoint 1".chars() {
            app.picker_char(ch);
        }
        app.picker_confirm();
        assert_eq!(header(&app), "HEAD · just now");
        assert_eq!(app.view().checkpoint_counts(), Some((1, 1)), "two → three");

        // Space v g: a commit as the base, the working file as the target.
        press(&mut app, " vg");
        assert!(matches!(app.popup(), Some(Popup::Picker(p)) if p.kind() == PickerKind::CheckBase));
        let oldest = picker_items(&app).last().cloned().unwrap_or_default();
        for ch in oldest.chars().take(7) {
            app.picker_char(ch);
        }
        app.picker_confirm();
        assert_eq!(header(&app), format!("{} · now", &oldest[..7]));
        let shown: Vec<String> = app
            .view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect();
        assert!(shown.iter().any(|l| l.contains("-one")), "{shown:?}");

        // A new checkpoint while the view is open re-reads the timeline.
        fs::write(dir.0.join("a.md"), "five\n")?;
        app.on_changes(vec![dir.0.join("a.md")]);
        press(&mut app, " vc");
        assert_eq!(app.checkpoint_strip().len(), 3);
        press(&mut app, "gs");
        assert!(
            !app.view().checkpoint_view(),
            "gs leaves the checkpoint view"
        );
        Ok(())
    }
}
