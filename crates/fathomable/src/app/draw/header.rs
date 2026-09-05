// @okf-doc: /decisions/0059-headers-and-the-key-bar.md
//! Pane headers and their key hints (ADR 0050, ADR 0059).
//!
//! A [`Header`] is the words on a pane's chrome row and the hints after
//! them, built once so the drawing and the mouse agree on where each
//! hint is. The review list's header carries its counts with the sort
//! word at the right edge; its keys sit on a bar along the list's
//! bottom row, left-aligned, built by [`review_footer`]. Every header
//! row draws on `ui.header`.

use fathomable_core::layout::display_width;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::app::draw::{Theme, mark_style};
use crate::app::input::bindings::{self, Action, Where};
use crate::app::threads::list::Entry;
use crate::app::threads::words::{Words, label};
use crate::app::threads::{Compose, ComposeTarget, ThreadState};
use crate::app::{App, Focus};

/// How `action` is spelled on `place`, for a hint; a binding the table
/// lacks shows as nothing rather than a made-up key.
fn key_of(place: Where, action: Action) -> String {
    bindings::hint(place, action).unwrap_or_default()
}

/// Two actions' keys as `a/b`, the way paired hints read.
fn pair(place: Where, a: Action, b: Action) -> String {
    format!("{}/{}", key_of(place, a), key_of(place, b))
}

/// The colour a word of a header's left part takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Key,
    Info,
    Mark(ThreadState),
}

/// A header hint with what a click on it runs (ADR 0050): nothing for
/// words alone, two actions for an `a/b` pair split at the slash.
#[derive(Debug, Clone)]
pub(crate) struct HintOf {
    key: String,
    what: &'static str,
    actions: Vec<Action>,
}

impl HintOf {
    fn new(key: impl Into<String>, what: &'static str, actions: &[Action]) -> Self {
        Self {
            key: key.into(),
            what,
            actions: actions.to_vec(),
        }
    }

    fn keyed(place: Where, action: Action, what: &'static str) -> Self {
        Self::new(key_of(place, action), what, &[action])
    }

    fn paired(place: Where, a: Action, b: Action, what: &'static str) -> Self {
        Self::new(pair(place, a, b), what, &[a, b])
    }

    /// The columns the hint takes: key, action, and the space between
    /// when both are present.
    fn width(&self) -> usize {
        display_width(&self.key)
            + display_width(self.what)
            + usize::from(!self.key.is_empty() && !self.what.is_empty())
    }
}

/// Where a header's hints sit: against the right edge after the words,
/// or from the left edge along a key bar (ADR 0059).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Right,
    Left,
}

/// Cells kept clear around the hints: one before, one after, and one
/// more so a full row never touches the words.
const HINT_MARGIN: usize = 3;

/// A pane header: the words on the left and the hints after them,
/// built once so the drawing and the mouse agree on where each hint is
/// (ADR 0050).
#[derive(Debug, Clone)]
pub(crate) struct Header {
    left: Vec<(String, Tone)>,
    hints: Vec<HintOf>,
    align: Align,
}

impl Header {
    /// Words on the left, hints against the right edge.
    pub(super) fn new(left: Vec<(String, Tone)>, hints: Vec<HintOf>) -> Self {
        Self {
            left,
            hints,
            align: Align::Right,
        }
    }

    /// A key bar: no words, the hints from the left edge (ADR 0059).
    fn bar(hints: Vec<HintOf>) -> Self {
        Self {
            left: Vec::new(),
            hints,
            align: Align::Left,
        }
    }

    /// The cells the left part takes.
    pub(crate) fn left_width(&self) -> usize {
        self.left.iter().map(|(text, _)| display_width(text)).sum()
    }

    /// The hints that fit after the left part on `width` cells, losing
    /// items from the end until they do, and the column the first
    /// starts at.
    fn shown(&self, width: usize) -> Option<(usize, &[HintOf])> {
        let used = self.left_width();
        let free = width.saturating_sub(used);
        let shown = (1..=self.hints.len())
            .rev()
            .map(|n| &self.hints[..n])
            .find(|shown| free >= hints_width(shown) + HINT_MARGIN)?;
        let start = match self.align {
            Align::Right => used + (free - hints_width(shown) - 1),
            Align::Left => used + 1,
        };
        Some((start, shown))
    }

    /// The action a click at `column` on a `width`-cell header runs: the
    /// hint under the pointer, its left or right half for a pair.
    pub(crate) fn action_at(&self, width: usize, column: usize) -> Option<Action> {
        let (mut at, shown) = self.shown(width)?;
        for (i, hint) in shown.iter().enumerate() {
            if i > 0 {
                at += 3;
            }
            let end = at + hint.width();
            if column >= at && column < end {
                let offset = column - at;
                let slash = hint
                    .key
                    .find('/')
                    .map(|byte| display_width(&hint.key[..byte]));
                return match (hint.actions.as_slice(), slash) {
                    ([first, second], Some(slash)) => {
                        Some(if offset <= slash { *first } else { *second })
                    }
                    (actions, _) => actions.first().copied(),
                };
            }
            at = end;
        }
        None
    }

    /// The header as one drawn row on `ui.header`: the words, then the
    /// hints joined by ` · `, the key dim and its action dimmer still,
    /// padded to `width` so the surface reaches the right edge.
    pub(super) fn line(&self, theme: &Theme, width: usize) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = self
            .left
            .iter()
            .map(|(text, tone)| {
                let style = match tone {
                    Tone::Key => theme.popup_key,
                    Tone::Info => theme.info,
                    Tone::Mark(state) => mark_style(theme, *state),
                };
                Span::styled(text.clone(), style)
            })
            .collect();
        let mut at = self.left_width();
        if let Some((start, shown)) = self.shown(width) {
            spans.push(Span::raw(" ".repeat(start - at)));
            at = start;
            let faint = theme.info.add_modifier(Modifier::DIM);
            for (i, hint) in shown.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" · ", faint));
                    at += 3;
                }
                if !hint.key.is_empty() {
                    spans.push(Span::styled(hint.key.clone(), theme.info));
                }
                if !hint.key.is_empty() && !hint.what.is_empty() {
                    spans.push(Span::styled(" ", faint));
                }
                if !hint.what.is_empty() {
                    spans.push(Span::styled(hint.what, faint));
                }
                at += hint.width();
            }
        }
        spans.push(Span::raw(" ".repeat(width.saturating_sub(at))));
        Line::from(spans).style(theme.header)
    }
}

/// The cells `shown` hints take with ` · ` between them.
fn hints_width(shown: &[HintOf]) -> usize {
    shown.iter().map(HintOf::width).sum::<usize>() + 3 * shown.len().saturating_sub(1)
}

/// A diff's header (ADR 0049, ADR 0060): the pair's names, then the
/// paging, side, whitespace, and close keys.
pub(crate) fn diff_header(text: &str) -> Header {
    Header::new(
        vec![(format!(" {text}"), Tone::Key)],
        vec![
            HintOf::paired(Where::View, Action::MoveLeft, Action::MoveRight, "page"),
            HintOf::keyed(Where::View, Action::DiffBase, "base"),
            HintOf::keyed(Where::View, Action::DiffTarget, "target"),
            HintOf::keyed(Where::View, Action::DiffWhitespace, "whitespace"),
            HintOf::keyed(Where::View, Action::Escape, "close"),
        ],
    )
}

/// An expanded thread's header row in the text (ADR 0049): the state,
/// the placement, who watches it, and the thread keys.
pub(crate) fn expanded_header(app: &App, thread: &fathomable_core::annotations::Thread) -> Header {
    let mark = app.marks().iter().find(|mark| mark.id() == thread.id());
    let words = Words::of(mark.map(crate::app::threads::Mark::placement), thread);
    let tone = Tone::Mark(words.state());
    let mut left = vec![(" ● ".to_owned(), tone)];
    if let Some(placement) = words.placement() {
        left.push((placement.to_owned(), tone));
        left.push((" · ".to_owned(), Tone::Info));
    }
    left.push((label(words.state()).to_owned(), tone));
    if words.proposed() {
        left.push((" · proposed".to_owned(), tone));
    }
    let watchers = app.watchers_of(thread.id());
    if !watchers.is_empty() {
        left.push((format!(" · watched by {}", watchers.join(", ")), Tone::Info));
    }
    let resolve = if words.is_resolved() {
        "reopen"
    } else {
        "resolve"
    };
    Header::new(
        left,
        vec![
            HintOf::keyed(Where::View, Action::Reply, "reply"),
            HintOf::keyed(Where::View, Action::EditMessage, "edit"),
            HintOf::keyed(Where::View, Action::ToggleResolved, resolve),
            HintOf::keyed(Where::View, Action::Comment, "fold"),
        ],
    )
}

/// The review list's header (ADR 0025, ADR 0049, ADR 0059): the counts
/// joined by dots, the path while the list is one file's, and the sort
/// word at the right edge, where a click switches the sort as `s` does.
pub(crate) fn review_header(app: &App, entries: &[Entry]) -> Header {
    let review = app.review();
    let open = entries
        .iter()
        .filter(|entry| matches!(entry.kind(), ThreadState::Open | ThreadState::Waiting))
        .count();
    let resolved = entries.len() - open;
    let proposed = entries.iter().filter(|entry| entry.proposed()).count();
    let mut left = vec![
        (" review ".to_owned(), Tone::Key),
        (format!(" {open} open"), Tone::Info),
    ];
    if proposed > 0 {
        left.push((format!(" · {proposed} proposed"), Tone::Info));
    }
    left.push((
        if review.resolved {
            format!(" · {resolved} resolved")
        } else {
            " · resolved hidden".to_owned()
        },
        Tone::Info,
    ));
    if review.file_only {
        left.push((format!(" · {}", app.current_path().display()), Tone::Info));
    }
    Header::new(
        left,
        vec![HintOf::new(review.sort.label(), "", &[Action::ReviewSort])],
    )
}

/// The review list's key bar on its bottom row (ADR 0059): the list
/// keys while it has focus, else how to focus it.
pub(crate) fn review_footer(app: &App, entries: &[Entry]) -> Header {
    let place = Where::Review;
    let hints = if app.focus() == Focus::Review {
        let mut hints = vec![
            HintOf::keyed(place, Action::ReviewSort, "sort"),
            HintOf::keyed(place, Action::ReviewResolved, "resolved"),
            HintOf::keyed(place, Action::FileOnly, "file"),
            HintOf::keyed(place, Action::Fold, "fold"),
            HintOf::keyed(place, Action::Confirm, "open"),
            HintOf::keyed(place, Action::Reply, "reply"),
        ];
        if app.thread_message_editable() {
            hints.push(HintOf::keyed(place, Action::EditMessage, "edit"));
        }
        hints.push(HintOf::keyed(place, Action::ToggleResolved, "resolve"));
        if entries.len() > 1 {
            hints.push(HintOf::paired(
                place,
                Action::ThreadPrev,
                Action::ThreadNext,
                "threads",
            ));
        }
        if app.cursor_message_count() > 1 {
            hints.push(HintOf::paired(
                place,
                Action::MoveDown,
                Action::MoveUp,
                "messages",
            ));
        }
        hints.push(HintOf::keyed(place, Action::Escape, ""));
        hints
    } else {
        vec![HintOf::new("", "click or Space w h to focus", &[])]
    };
    Header::bar(hints)
}

/// The draft's author row (ADR 0054): ` user  draft` as a message's
/// author row reads, then the draft keys, or the discard question.
pub(crate) fn draft_header(compose: &Compose) -> Header {
    let place = Where::Draft;
    let hints = if compose.confirming_discard() {
        vec![
            HintOf::keyed(place, Action::Escape, "again to discard"),
            HintOf::new("", "any key keeps the draft", &[]),
        ]
    } else {
        vec![
            HintOf::keyed(
                place,
                Action::Confirm,
                if matches!(compose.target(), ComposeTarget::Edit { .. }) {
                    "save"
                } else {
                    "submit"
                },
            ),
            HintOf::keyed(place, Action::Newline, "newline"),
            HintOf::paired(place, Action::ScrollUp, Action::ScrollDown, "scroll"),
            HintOf::keyed(place, Action::EditDraft, "$EDITOR"),
            HintOf::keyed(place, Action::Escape, ""),
        ]
    };
    Header::new(
        vec![
            (" user".to_owned(), Tone::Key),
            ("  draft".to_owned(), Tone::Info),
        ],
        hints,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> anyhow::Result<Theme> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        Ok(Theme::from_core(&core))
    }

    fn text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn bar() -> Header {
        Header::bar(vec![
            HintOf::new("s", "sort", &[Action::ReviewSort]),
            HintOf::new("x", "resolved", &[Action::ReviewResolved]),
            HintOf::new("k/j", "threads", &[Action::ThreadPrev, Action::ThreadNext]),
        ])
    }

    #[test]
    fn a_bar_reads_from_the_left_and_fills_the_row() -> anyhow::Result<()> {
        let line = bar().line(&theme()?, 40);
        let text = text(&line);
        assert_eq!(display_width(&text), 40);
        assert_eq!(text.trim_end(), " s sort · x resolved · k/j threads");
        Ok(())
    }

    #[test]
    fn a_bar_drops_hints_from_the_end_when_narrow() -> anyhow::Result<()> {
        let theme = theme()?;
        let line = bar().line(&theme, 24);
        assert_eq!(text(&line).trim_end(), " s sort · x resolved");
        let line = bar().line(&theme, 4);
        assert_eq!(text(&line), "    ", "no hint fits");
        Ok(())
    }

    #[test]
    fn a_click_on_a_bar_hint_runs_it_and_a_pair_splits_at_the_slash() {
        let bar = bar();
        assert_eq!(bar.action_at(40, 1), Some(Action::ReviewSort));
        assert_eq!(bar.action_at(40, 10), Some(Action::ReviewResolved));
        let threads = display_width(" s sort · x resolved · ");
        assert_eq!(bar.action_at(40, threads), Some(Action::ThreadPrev));
        assert_eq!(bar.action_at(40, threads + 2), Some(Action::ThreadNext));
        assert_eq!(bar.action_at(40, 0), None, "the margin runs nothing");
        assert_eq!(
            bar.action_at(24, threads),
            None,
            "a dropped hint is not there"
        );
    }

    #[test]
    fn a_header_keeps_its_hints_at_the_right_edge() -> anyhow::Result<()> {
        let header = Header::new(
            vec![(" review".to_owned(), Tone::Key)],
            vec![HintOf::new("by file", "", &[Action::ReviewSort])],
        );
        let line = header.line(&theme()?, 30);
        let text = text(&line);
        assert_eq!(display_width(&text), 30);
        assert_eq!(text.trim_end(), " review               by file");
        assert_eq!(header.action_at(30, 25), Some(Action::ReviewSort));
        Ok(())
    }
}
