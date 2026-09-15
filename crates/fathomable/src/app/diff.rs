// @okf-doc: /decisions/0060-one-diff-two-sides.md
//! The diff view (ADR 0060): one unified diff between two [`Side`]s of
//! the current file, with a header naming the pair and a badge naming
//! the base.
//!
//! `Space d d` shows the net `HEAD · now` diff, `Space d D` shows
//! `last seen · now`, and `Space d r` the newest checkpoint pair; each
//! closes the diff when its own pair is on screen. `b` and `t` pick
//! either side from the file's checkpoints, the working file, the
//! index, the last-seen snapshot, `HEAD`, and commits that touched it;
//! `Space d g` picks a commit for the base with the working file as the
//! target; `h` / `l` page the checkpoint timeline; `w` ignores
//! whitespace; `Esc` leaves. The checkpoint marks and the strip live in
//! [`super::checkpoints`].

use std::path::Path;

use fathomable_core::clock::now;
use fathomable_core::diff::Whitespace;

use super::draw::format_age;
use super::{App, PickerKind};

/// Commits touching the file the pickers list, at most (ADR 0049).
const HISTORY_LIMIT: usize = 50;

/// One side of the diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Side {
    /// The file's checkpoint at this index on its timeline, oldest first.
    Checkpoint(usize),
    /// The file as committed at this commit (full hex).
    Commit(String),
    /// The file as committed at `HEAD`.
    Head,
    /// The file as staged in the index.
    Index,
    /// The last-seen snapshot of the file (ADR 0015).
    Seen,
    /// The working file, as the view shows it.
    Working,
}

/// Where the layout reads a side's text: one of the view's own copies,
/// or a text fetched for the side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Text {
    Working,
    Index,
    Head,
    Seen,
    Owned(String),
}

/// What the diff view lays out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffBody {
    /// A unified diff from `base` to `target`.
    Diff { base: Text, target: Text },
    /// One line saying why there is no diff.
    Notice(String),
}

/// The diff a view shows (ADR 0060).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffView {
    pub(crate) base: Side,
    pub(crate) target: Side,
    /// `checkpoint 2/3  5m ago · now`, `HEAD · INDEX`, or the two labels.
    pub(crate) header: String,
    /// The status badge: the Git layer or base (`DIFF unstaged`,
    /// `DIFF staged`, `DIFF net`, `DIFF seen`, `DIFF cp 2/3`).
    pub(crate) badge: String,
    pub(crate) body: DiffBody,
}

impl DiffView {
    /// The net `HEAD -> worktree` pair `Space d d` shows.
    #[cfg(test)]
    pub(crate) fn head(worktree_missing: bool) -> Self {
        Self {
            base: Side::Head,
            target: Side::Working,
            header: format!(
                "HEAD · {}",
                if worktree_missing {
                    "WORKTREE (missing)"
                } else {
                    "now"
                }
            ),
            badge: "DIFF net".to_owned(),
            body: DiffBody::Diff {
                base: Text::Head,
                target: Text::Working,
            },
        }
    }

    /// `last seen · now`, the pair `Space d D` shows.
    #[cfg(test)]
    pub(crate) fn seen(worktree_missing: bool) -> Self {
        Self {
            base: Side::Seen,
            target: Side::Working,
            header: format!(
                "last seen · {}",
                if worktree_missing {
                    "WORKTREE (missing)"
                } else {
                    "now"
                }
            ),
            badge: "DIFF seen".to_owned(),
            body: DiffBody::Diff {
                base: Text::Seen,
                target: Text::Working,
            },
        }
    }

    /// Whether this is the diff from `base` to `target`.
    pub(crate) fn is_pair(&self, base: &Side, target: &Side) -> bool {
        self.base == *base && self.target == *target
    }

    /// Whether the base is a checkpoint, so `h` / `l` page the timeline
    /// and `Space d r` closes the diff.
    pub(crate) fn on_checkpoint(&self) -> bool {
        matches!(self.base, Side::Checkpoint(_))
    }
}

impl App {
    /// `Space d d` / `:diff`: the diff against `HEAD`, or back
    /// to the file when that pair is shown.
    pub(crate) fn toggle_head_diff(&mut self) {
        if self
            .view()
            .diff()
            .is_some_and(|diff| diff.is_pair(&Side::Head, &Side::Working))
        {
            self.leave_diff();
        } else if self.view().has_head() {
            self.show_diff(Side::Head, Side::Working);
        } else {
            self.notice("no diff base: not in a git repository");
        }
    }

    /// `Space d D` / `:diff seen`: the diff against the last-seen
    /// snapshot, or back to the file when that pair is shown.
    pub(crate) fn toggle_seen_diff(&mut self) {
        if self
            .view()
            .diff()
            .is_some_and(|diff| diff.is_pair(&Side::Seen, &Side::Working))
        {
            self.leave_diff();
        } else if self.view().has_seen() {
            self.show_diff(Side::Seen, Side::Working);
        } else {
            self.notice("no last-seen snapshot of this file yet");
        }
    }

    /// `Space d r`: the diff of the current file on its newest checkpoint
    /// pair, or back to the file when a checkpoint diff is shown.
    pub(crate) fn toggle_checkpoint_diff(&mut self) {
        if self.view().diff().is_some_and(DiffView::on_checkpoint) {
            self.leave_diff();
            return;
        }
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        self.show_newest_pair();
    }

    /// Back to the file's own display.
    pub(crate) fn leave_diff(&mut self) {
        self.view_mut().leave_diff();
        self.relayout();
    }

    /// `Esc` in the text: clear the input, selection, or highlight, and
    /// with none of those to clear, leave the diff (ADR 0060).
    pub(crate) fn escape_view(&mut self) {
        if !self.view_mut().escape() && self.view().diff_view() {
            self.leave_diff();
        }
    }

    /// The latest checkpoint against the working file, or the notice that
    /// there is none.
    pub(super) fn show_newest_pair(&mut self) {
        let count = self.timeline_len();
        if count == 0 {
            self.show_diff(Side::Working, Side::Working);
        } else {
            self.show_diff(Side::Checkpoint(count - 1), Side::Working);
        }
    }

    /// `h` / `l` in a diff: the earlier or later neighbouring pair along
    /// the checkpoint timeline.
    pub(crate) fn diff_page(&mut self, delta: isize) {
        let Some(diff) = self.view().diff() else {
            return;
        };
        let count = self.timeline_len();
        let Side::Checkpoint(index) = diff.base else {
            self.notice(if count == 0 {
                "no checkpoint of this file yet; Space d c makes one"
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
        self.show_diff(Side::Checkpoint(next), target);
    }

    /// `b` / `t`, `Space d b` / `Space d t`: a picker for the base or the
    /// target side. Outside a diff the other side is the working file.
    pub(crate) fn pick_diff_side(&mut self, target: bool) {
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        self.diff_target_next = None;
        self.open_picker(if target {
            PickerKind::DiffTarget
        } else {
            PickerKind::DiffBase
        });
    }

    /// `Space d g`: pick a commit (or anything else) as the base, with
    /// the working file as the target.
    pub(crate) fn diff_against_commit(&mut self) {
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        self.diff_target_next = Some(Side::Working);
        self.open_picker(PickerKind::DiffBase);
    }

    /// `w` / `Space d w`: compare lines with whitespace ignored, or
    /// exactly again; every open diff relays out.
    pub(crate) fn toggle_whitespace(&mut self) {
        self.compare.whitespace = match self.compare.whitespace {
            Whitespace::Exact => Whitespace::Ignore,
            Whitespace::Ignore => Whitespace::Exact,
        };
        let compare = self.compare;
        for doc in &mut self.docs {
            doc.view.set_compare(compare);
        }
        self.push_toast(match compare.whitespace {
            Whitespace::Ignore => "whitespace ignored".to_owned(),
            Whitespace::Exact => "whitespace compared".to_owned(),
        });
    }

    /// How diffs are compared this session (ADR 0060).
    #[cfg(test)]
    pub(crate) fn compare(&self) -> fathomable_core::diff::Compare {
        self.compare
    }

    /// The header row's text: the pair, then the whitespace rule while
    /// it is not the exact one.
    pub(crate) fn diff_header(&self) -> Option<String> {
        let diff = self.view().diff()?;
        Some(match self.compare.whitespace {
            Whitespace::Exact => diff.header.clone(),
            Whitespace::Ignore => format!("{} · whitespace ignored", diff.header),
        })
    }

    /// Rows the diff's chrome takes from the text: the header over it,
    /// and the strip of the file's checkpoints under it while the file
    /// has one.
    pub(crate) fn diff_chrome_rows(&self) -> usize {
        if !self.has_document()
            || self.directory_path().is_some()
            || self.review_list().is_open()
            || self.info().is_some()
            || !self.view().diff_view()
        {
            return 0;
        }
        1 + usize::from(self.timeline_len() > 0)
    }

    /// The items of the side pickers, remembering which side each names.
    pub(super) fn diff_choices(&mut self) -> Vec<String> {
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
        if self.view().has_seen() {
            choices.push(("last seen".to_owned(), Side::Seen));
        }
        if self.workspace.is_git() {
            choices.push(("HEAD".to_owned(), Side::Head));
            choices.push(("index".to_owned(), Side::Index));
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
        self.diff_choices = choices;
        items
    }

    /// The picker's choice for a side: show the diff with it.
    pub(super) fn choose_diff_side(&mut self, kind: PickerKind, item: &str) {
        let Some(side) = self
            .diff_choices
            .iter()
            .find(|(text, _)| text == item)
            .map(|(_, side)| side.clone())
        else {
            return;
        };
        let current = self
            .view()
            .diff()
            .map(|d| (d.base.clone(), d.target.clone()));
        if kind == PickerKind::DiffTarget {
            let base = current.map_or(Side::Working, |(base, _)| base);
            self.show_diff(base, side);
        } else {
            let target = self
                .diff_target_next
                .take()
                .or_else(|| current.map(|(_, target)| target))
                .unwrap_or(Side::Working);
            self.show_diff(side, target);
        }
    }

    /// Lay the diff between `base` and `target` in the current view.
    pub(super) fn show_diff(&mut self, base: Side, target: Side) {
        let relative = self.current_path().to_path_buf();
        let count = self.timeline_len();
        let layered_net_empty = base == Side::Head
            && target == Side::Working
            && self.view().net_diff_empty()
            && self.status().get(&relative).is_some_and(|entry| {
                entry.staged_state().is_some() && entry.unstaged_state().is_some()
            });
        let body = match (&base, &target) {
            (Side::Working, Side::Working) if count == 0 => DiffBody::Notice(
                "no checkpoint of this file yet; Space d c makes one, b picks another base"
                    .to_owned(),
            ),
            _ if layered_net_empty => {
                DiffBody::Notice("net diff is empty; staged and unstaged changes cancel".to_owned())
            }
            _ => match (
                self.side_text(&base, &relative),
                self.side_text(&target, &relative),
            ) {
                (Ok(base), Ok(target)) => DiffBody::Diff { base, target },
                (Err(error), _) | (_, Err(error)) => DiffBody::Notice(error),
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
        let badge = match (&base, &target) {
            (Side::Head, Side::Working) => "DIFF net".to_owned(),
            (Side::Head, Side::Index) => "DIFF staged".to_owned(),
            (Side::Index, Side::Working) => "DIFF unstaged".to_owned(),
            (Side::Working, _) => "DIFF now".to_owned(),
            (Side::Head, _) => "DIFF HEAD".to_owned(),
            (Side::Index, _) => "DIFF index".to_owned(),
            (Side::Seen, _) => "DIFF seen".to_owned(),
            (Side::Commit(hex), _) => format!("DIFF {}", &hex[..hex.len().min(7)]),
            (Side::Checkpoint(i), _) => format!("DIFF cp {}/{count}", i + 1),
        };
        self.view_mut().show_diff(DiffView {
            base,
            target,
            header,
            badge,
            body,
        });
        self.relayout();
    }

    /// Where the layout reads `side`'s text for `relative`.
    fn side_text(&self, side: &Side, relative: &Path) -> Result<Text, String> {
        match side {
            Side::Working => Ok(Text::Working),
            Side::Index => self
                .view()
                .has_index()
                .then_some(Text::Index)
                .ok_or_else(|| "no index: not in a git repository".to_owned()),
            Side::Head => self
                .view()
                .has_head()
                .then_some(Text::Head)
                .ok_or_else(|| "no HEAD: not in a git repository".to_owned()),
            Side::Seen => self
                .view()
                .has_seen()
                .then_some(Text::Seen)
                .ok_or_else(|| "no last-seen snapshot of this file yet".to_owned()),
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
                    .map(Text::Owned)
                    .map_err(|error| format!("cannot read the checkpoint: {error}"))
            }
            Side::Commit(hex) => self
                .workspace
                .text_at(hex, relative)
                .map_err(|error| error.to_string())
                .and_then(|text| text.ok_or_else(|| "not in a git repository".to_owned()))
                .map(Text::Owned),
        }
    }

    fn side_label(&self, side: &Side, relative: &Path) -> String {
        match side {
            Side::Working if self.view().worktree_missing() => "WORKTREE (missing)".to_owned(),
            Side::Working => "now".to_owned(),
            Side::Index if self.view().index_missing() => "INDEX (missing)".to_owned(),
            Side::Index => "INDEX".to_owned(),
            Side::Head => "HEAD".to_owned(),
            Side::Seen => "last seen".to_owned(),
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
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fathomable_core::checkpoints::Store;
    use fathomable_core::diff::Whitespace;
    use fathomable_core::workspace::Workspace;
    use fathomable_testing::{TempDir, git};

    use super::Side;
    use crate::app::testing::{press, press_key};
    use crate::app::{App, Options, PickerKind, Popup};
    use crossterm::event::KeyCode;

    fn header(app: &App) -> String {
        app.view()
            .diff()
            .map(|d| d.header.clone())
            .unwrap_or_default()
    }

    fn badge(app: &App) -> String {
        app.view()
            .diff()
            .map(|d| d.badge.clone())
            .unwrap_or_default()
    }

    fn shown(app: &App) -> Vec<String> {
        app.view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect()
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

    fn git_app(dir: &TempDir) -> anyhow::Result<App> {
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
        app.settle_status();
        Ok(app)
    }

    /// `Space d d` shows `HEAD · now` with its header and badge and one chrome
    /// row when the file has no checkpoint; again closes it; `Space d D`
    /// from the `HEAD` diff switches; `Esc` leaves; `:diff` is the same
    /// toggle through the command line (ADR 0060).
    #[test]
    fn head_and_seen_diffs_are_pairs_with_a_header() -> anyhow::Result<()> {
        let dir = TempDir::new("app-diff-pairs")?;
        let mut app = git_app(&dir)?;

        press(&mut app, " dd");
        assert_eq!(header(&app), "HEAD · now");
        assert_eq!(badge(&app), "DIFF net");
        assert_eq!(app.diff_chrome_rows(), 1, "a header, no strip");
        assert_eq!(app.text_rows(), 30 - 1 - 1);
        assert!(shown(&app).iter().any(|l| l.contains("-two")));
        press(&mut app, " dd");
        assert!(!app.view().diff_view(), "Space d d on its own pair closes");
        assert_eq!(app.text_rows(), 29);

        press(&mut app, " dD");
        assert!(!app.view().diff_view(), "never seen: no seen diff");
        assert!(app.message().is_some_and(|m| m.contains("last-seen")));
        app.view_mut()
            .set_bases(Some("one\n".to_owned()), None, Some("two\n".to_owned()));
        press(&mut app, " dD");
        assert_eq!(header(&app), "last seen · now");
        assert_eq!(badge(&app), "DIFF seen");
        press(&mut app, " dd");
        assert_eq!(
            header(&app),
            "HEAD · now",
            "Space d d from the seen diff switches"
        );
        app.command("diff seen");
        assert_eq!(
            header(&app),
            "last seen · now",
            ":diff seen is the same toggle"
        );
        app.command("diff");
        assert_eq!(header(&app), "HEAD · now");
        press(&mut app, "/two");
        press_key(&mut app, KeyCode::Enter);
        press_key(&mut app, KeyCode::Esc);
        assert!(app.view().diff_view(), "the first Esc clears the highlight");
        press_key(&mut app, KeyCode::Esc);
        assert!(!app.view().diff_view(), "the next Esc leaves the diff");
        assert!(
            !app.view().source_view(),
            "a Markdown file's home is the rendered view"
        );
        Ok(())
    }

    #[test]
    fn net_diff_explains_canceling_staged_and_unstaged_layers() -> anyhow::Result<()> {
        let dir = TempDir::new("app-diff-canceling-layers")?;
        git::init(&dir.0)?;
        git::stage(&dir.0, &[("new.md", "staged\n")])?;
        let mut app = App::new(
            Workspace::discover(&dir.0)?,
            100,
            30,
            Options::for_test(dir.0.clone()),
        );
        app.settle_status();
        app.open(Path::new("new.md"));
        assert_eq!(app.view().text(), "staged\n");
        assert_eq!(app.banner(), Some("deleted from worktree · showing INDEX"));

        press(&mut app, " dd");
        assert_eq!(header(&app), "HEAD · WORKTREE (missing)");
        assert_eq!(badge(&app), "DIFF net");
        let lines = shown(&app);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("staged and unstaged changes cancel")),
            "{lines:?}"
        );
        Ok(())
    }

    /// `b` outside a diff opens one against the working file, the picker
    /// listing the last-seen snapshot when the file has one; `w` ignores
    /// whitespace and says so in the header (ADR 0060).
    #[test]
    fn side_pickers_and_whitespace_work_in_any_diff() -> anyhow::Result<()> {
        let dir = TempDir::new("app-diff-sides")?;
        let mut app = git_app(&dir)?;
        app.view_mut()
            .set_bases(Some("one\n".to_owned()), None, Some("two\n".to_owned()));

        press(&mut app, "b");
        let items = picker_items(&app);
        assert_eq!(items[0], "working file  now");
        assert_eq!(items[1], "last seen");
        assert_eq!(items[2], "HEAD");
        for ch in "last seen".chars() {
            app.picker_char(ch);
        }
        app.picker_confirm();
        assert_eq!(header(&app), "last seen · now");
        assert_eq!(badge(&app), "DIFF seen");
        assert!(shown(&app).iter().any(|l| l.contains("-one")));

        fs::write(dir.0.join("a.md"), "  two  \n")?;
        app.on_changes(vec![dir.0.join("a.md")]);
        press(&mut app, " dd");
        assert_eq!(app.view().pair_counts(), Some((1, 1)));
        press(&mut app, "w");
        assert_eq!(app.compare().whitespace, Whitespace::Ignore);
        assert_eq!(
            app.diff_header().as_deref(),
            Some("HEAD · now · whitespace ignored")
        );
        assert_eq!(app.view().pair_counts(), Some((0, 0)));
        assert_eq!(
            app.view().diff_counts(),
            Some((1, 1)),
            "the gutter never ignores whitespace"
        );
        press(&mut app, " dw");
        assert_eq!(app.compare().whitespace, Whitespace::Exact);
        assert_eq!(app.diff_header().as_deref(), Some("HEAD · now"));
        Ok(())
    }

    /// `Space d r` opens the newest pair and `h`/`l` page the timeline;
    /// `b`/`t` pick any side, commits included; `Space d g` fixes the
    /// target at the working file; a record refreshes the open view.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one walk through the whole view")]
    fn checkpoint_diff_pages_and_picks_sides() -> anyhow::Result<()> {
        let dir = TempDir::new("app-checkpoint-view")?;
        let mut app = git_app(&dir)?;

        press(&mut app, " dr");
        assert!(app.view().diff_view());
        assert_eq!(header(&app), "now · now");
        assert!(
            shown(&app)[0].contains("no checkpoint of this file yet"),
            "{}",
            shown(&app)[0]
        );
        assert_eq!(app.text_rows(), 30 - 1 - 1, "the header takes a row");
        press(&mut app, " dc");
        assert_eq!(
            header(&app),
            "checkpoint 1/1  just now · now",
            "the first checkpoint moves the empty view to the new pair"
        );
        assert_eq!(badge(&app), "DIFF cp 1/1");
        assert_eq!(
            app.text_rows(),
            30 - 1 - 2,
            "header and strip take two rows"
        );
        press(&mut app, " dr");
        assert!(!app.view().diff_view());
        assert_eq!(app.text_rows(), 29);

        fs::write(dir.0.join("a.md"), "four\n")?;
        app.on_changes(vec![dir.0.join("a.md")]);
        press(&mut app, " dc");
        press(&mut app, " dr");
        assert_eq!(header(&app), "checkpoint 2/2  just now · now");
        assert_eq!(
            app.view().pair_counts(),
            Some((0, 0)),
            "the latest checkpoint is the working file"
        );
        press(&mut app, "h");
        assert_eq!(header(&app), "checkpoint 1/2  just now · just now");
        assert_eq!(app.view().pair_counts(), Some((1, 1)));
        let lines = shown(&app);
        assert!(lines.iter().any(|l| l.contains("-three")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("+four")), "{lines:?}");
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
            2 + 3 + 2,
            "two checkpoints, working, HEAD, index, two commits: {items:?}"
        );
        assert!(items[0].starts_with("checkpoint 2"));
        assert_eq!(items[2], "working file  now");
        assert_eq!(items[3], "HEAD");
        assert_eq!(items[4], "index");
        assert!(items[5].ends_with("  commit"), "{}", items[5]);
        for ch in "HEAD".chars() {
            app.picker_char(ch);
        }
        app.picker_confirm();
        assert_eq!(header(&app), "HEAD · now");
        assert_eq!(badge(&app), "DIFF net");
        assert!(shown(&app).iter().any(|l| l.contains("-two")));
        press(&mut app, "h");
        assert_eq!(
            app.message(),
            Some("pick a checkpoint as the base (b) to page the timeline")
        );
        press(&mut app, " dd");
        assert!(
            !app.view().diff_view(),
            "Space d d closes the HEAD pair the picker made"
        );
        press(&mut app, " dd");

        press(&mut app, "t");
        assert!(
            matches!(app.popup(), Some(Popup::Picker(p)) if p.kind() == PickerKind::DiffTarget)
        );
        for ch in "checkpoint 1".chars() {
            app.picker_char(ch);
        }
        app.picker_confirm();
        assert_eq!(header(&app), "HEAD · just now");
        assert_eq!(app.view().pair_counts(), Some((1, 1)), "two → three");

        // Space d g: a commit as the base, the working file as the target.
        press(&mut app, " dg");
        assert!(matches!(app.popup(), Some(Popup::Picker(p)) if p.kind() == PickerKind::DiffBase));
        let oldest = picker_items(&app).last().cloned().unwrap_or_default();
        for ch in oldest.chars().take(7) {
            app.picker_char(ch);
        }
        app.picker_confirm();
        assert_eq!(header(&app), format!("{} · now", &oldest[..7]));
        assert_eq!(badge(&app), format!("DIFF {}", &oldest[..7]));
        assert!(shown(&app).iter().any(|l| l.contains("-one")));
        assert!(matches!(
            app.view().diff().map(|d| &d.base),
            Some(Side::Commit(_))
        ));

        // A new checkpoint while the view is open re-reads the timeline.
        fs::write(dir.0.join("a.md"), "five\n")?;
        app.on_changes(vec![dir.0.join("a.md")]);
        press(&mut app, " dc");
        assert_eq!(app.checkpoint_strip().len(), 3);
        press(&mut app, " vs");
        assert!(!app.view().diff_view(), "Space v s leaves the diff");
        assert!(app.view().source_view());
        Ok(())
    }
}
