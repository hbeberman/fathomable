// @okf-doc: /decisions/0064-hints-you-can-press.md
//! Pane chrome plus shared inline and review thread summaries.
//!
//! A [`Header`] is the words on a pane's chrome row and the hints after
//! them, built once so the drawing and the mouse agree on where each
//! hint is. The review list's and the threads pane's headers carry
//! their counts by colour (ADR 0066) as hints, so the resolved count
//! takes a click; a count's word (ADR 0075, [`super::counts`]) is drawn
//! when every count's fits and dropped with the others when not. Keys
//! live on a bar along a pane's bottom row,
//! left-aligned: [`review_footer`], [`threads_pane_footer`], and the
//! text's in [`super::bar`] (ADR 0067). Thread rows use the summary
//! engine from `app::threads::summary`, including direct actions and
//! exact hit regions (ADR 0086). Every header row draws on `ui.header`.
//!
//! A key hint is drawn only where pressing that key now, with the focus
//! and cursor as they are, runs the action it names (ADR 0064): a bar
//! whose keys would not work here says how to focus the pane instead.
//! The binding table says what a key is called; each builder here says
//! whether it works.

use std::path::Path;

use fathomable_core::layout::display_width;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::app::draw::counts::count_hints;
use crate::app::draw::nest::NEST;
use crate::app::draw::{Theme, mark_style};
use crate::app::input::bindings::{self, Action, Where};
use crate::app::threads::list::{Entry, ReviewView};
use crate::app::threads::summary::{
    SummaryLayout, SummaryLayoutOptions, SummarySpan, SummaryTone, ThreadSummary, layout,
};
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
    /// The sidebar's directory colour, bold: the threads pane's title.
    Dir,
    Mark(ThreadState),
    /// The diff colours: the files pane header's `+n` and `-m`.
    Added,
    Removed,
}

/// A header hint with what a click on it runs (ADR 0050): nothing for
/// words alone, two actions for an `a/b` pair split at the slash. A
/// count (ADR 0066) is a circle in a state's colour with its number
/// against it and a word after the number (ADR 0075) that the header
/// draws only when every count's fits.
#[derive(Debug, Clone)]
pub(crate) struct HintOf {
    key: String,
    what: String,
    /// The word after `what`, a space between; empty for most hints.
    word: String,
    actions: Vec<Action>,
    /// The key's colour when it is not the info colour.
    tone: Option<Tone>,
    /// The key reads dim: a count of what is hidden.
    faint: bool,
    /// A space between the key and its word.
    gap: bool,
    /// `what` is a count's number: it reads in the text colour rather
    /// than dim (ADR 0075).
    number: bool,
}

impl HintOf {
    pub(super) fn new(key: impl Into<String>, what: impl Into<String>, actions: &[Action]) -> Self {
        Self {
            key: key.into(),
            what: what.into(),
            word: String::new(),
            actions: actions.to_vec(),
            tone: None,
            faint: false,
            gap: true,
            number: false,
        }
    }

    pub(crate) fn keyed(place: Where, action: Action, what: &'static str) -> Self {
        Self::new(key_of(place, action), what, &[action])
    }

    pub(crate) fn paired(place: Where, a: Action, b: Action, what: &'static str) -> Self {
        Self::new(pair(place, a, b), what, &[a, b])
    }

    /// `● 2 user`: the circle in `state`'s colour, the count after a
    /// space in the text colour, `word` after that when the header has
    /// room (ADR 0075), all dim when it counts what is hidden (ADR
    /// 0066).
    pub(super) fn count(
        glyph: &'static str,
        word: &'static str,
        state: ThreadState,
        n: usize,
        faint: bool,
        actions: &[Action],
    ) -> Self {
        Self {
            key: glyph.to_owned(),
            what: n.to_string(),
            word: word.to_owned(),
            actions: actions.to_vec(),
            tone: Some(Tone::Mark(state)),
            faint,
            gap: true,
            number: true,
        }
    }

    /// A word in `tone` with nothing after it and no click: a count or a
    /// state word on the files pane's header (ADR 0068).
    fn word(text: String, tone: Tone) -> Self {
        Self {
            key: text,
            what: String::new(),
            word: String::new(),
            actions: Vec::new(),
            tone: Some(tone),
            faint: false,
            gap: false,
            number: false,
        }
    }

    /// The cells the word takes after `what` in the worded form: a
    /// space and the word, or none when the hint has no word.
    fn word_width(&self) -> usize {
        if self.word.is_empty() {
            0
        } else {
            1 + display_width(&self.word)
        }
    }

    /// The columns the hint takes in `form`: key, action, the space
    /// between when both are present, and the word when worded.
    fn width(&self, form: Form) -> usize {
        display_width(&self.key)
            + display_width(&self.what)
            + usize::from(self.gap && !self.key.is_empty() && !self.what.is_empty())
            + match form {
                Form::Worded => self.word_width(),
                Form::Bare => 0,
            }
    }
}

/// Whether a header's hints are drawn with their words after the
/// counts (ADR 0075): worded when every hint fits that way, else bare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Worded,
    Bare,
}

/// Where a header's hints sit: against the right edge after the words,
/// or from the left edge along a key bar (ADR 0059), or straight after
/// the words (ADR 0066).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Right,
    Left,
}

/// Cells kept clear around the hints: one before, one after, and one
/// more so a full row never touches the words.
const HINT_MARGIN: usize = 3;
/// A bar has no words to keep clear of: one cell before and one after.
const BAR_MARGIN: usize = 2;

/// A pane header: the words on the left and the hints after them,
/// built once so the drawing and the mouse agree on where each hint is
/// (ADR 0050).
#[derive(Debug, Clone)]
pub(crate) struct Header {
    left: Vec<(String, Tone)>,
    hints: Vec<HintOf>,
    align: Align,
    /// What joins the hints: ` · ` for keys, a space for counts.
    sep: &'static str,
}

impl Header {
    /// Words on the left, hints against the right edge.
    pub(super) fn new(left: Vec<(String, Tone)>, hints: Vec<HintOf>) -> Self {
        Self {
            left,
            hints,
            align: Align::Right,
            sep: " · ",
        }
    }

    /// A key bar: no words, the hints from the left edge (ADR 0059).
    pub(super) fn bar(hints: Vec<HintOf>) -> Self {
        Self {
            left: Vec::new(),
            hints,
            align: Align::Left,
            sep: " · ",
        }
    }

    /// Words, then counts against the right edge, a space apart (ADR
    /// 0066).
    pub(super) fn counted(left: Vec<(String, Tone)>, counts: Vec<HintOf>, align: Align) -> Self {
        Self {
            left,
            hints: counts,
            align,
            sep: " ",
        }
    }

    /// The cells the left part takes.
    pub(crate) fn left_width(&self) -> usize {
        self.left.iter().map(|(text, _)| display_width(text)).sum()
    }

    /// The hints that fit after the left part on `width` cells, the
    /// column the first starts at, and their form (ADR 0075): every
    /// hint with its word when that fits, else bare, losing items from
    /// the end until they do.
    fn shown(&self, width: usize) -> Option<(usize, &[HintOf], Form)> {
        let used = self.left_width();
        let free = width.saturating_sub(used);
        let sep = display_width(self.sep);
        let margin = if self.left.is_empty() {
            BAR_MARGIN
        } else {
            HINT_MARGIN
        };
        let fits = |shown: &[HintOf], form: Form| free >= hints_width(shown, sep, form) + margin;
        let worded =
            self.hints.iter().any(|hint| !hint.word.is_empty()) && fits(&self.hints, Form::Worded);
        let (shown, form) = if worded {
            (self.hints.as_slice(), Form::Worded)
        } else {
            let shown = (1..=self.hints.len())
                .rev()
                .map(|n| &self.hints[..n])
                .find(|shown| fits(shown, Form::Bare))?;
            (shown, Form::Bare)
        };
        let start = match self.align {
            Align::Right => used + (free - hints_width(shown, sep, form) - 1),
            Align::Left => used + 1,
        };
        Some((start, shown, form))
    }

    /// The action a click at `column` on a `width`-cell header runs: the
    /// hint under the pointer, its left or right half for a pair.
    pub(crate) fn action_at(&self, width: usize, column: usize) -> Option<Action> {
        let (mut at, shown, form) = self.shown(width)?;
        let sep = display_width(self.sep);
        for (i, hint) in shown.iter().enumerate() {
            if i > 0 {
                at += sep;
            }
            let end = at + hint.width(form);
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
    /// hints joined by the separator, the key dim and its action dimmer
    /// still, padded to `width` so the surface reaches the right edge.
    pub(super) fn line(&self, theme: &Theme, width: usize) -> Line<'static> {
        let tone_style = |tone: Tone| match tone {
            Tone::Key => theme.popup_key,
            Tone::Info => theme.info,
            Tone::Dir => theme.sidebar_dir.add_modifier(Modifier::BOLD),
            Tone::Mark(state) => mark_style(theme, state),
            Tone::Added => theme.diff_plus,
            Tone::Removed => theme.diff_minus,
        };
        let mut spans: Vec<Span<'static>> = self
            .left
            .iter()
            .map(|(text, tone)| Span::styled(text.clone(), tone_style(*tone)))
            .collect();
        let mut at = self.left_width();
        if let Some((start, shown, form)) = self.shown(width) {
            spans.push(Span::raw(" ".repeat(start - at)));
            at = start;
            let faint = theme.info.add_modifier(Modifier::DIM);
            let sep = display_width(self.sep);
            for (i, hint) in shown.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(self.sep, faint));
                    at += sep;
                }
                if !hint.key.is_empty() {
                    let mut style = hint.tone.map_or(theme.info, tone_style);
                    if hint.faint {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    spans.push(Span::styled(hint.key.clone(), style));
                }
                if hint.gap && !hint.key.is_empty() && !hint.what.is_empty() {
                    spans.push(Span::styled(" ", faint));
                }
                if !hint.what.is_empty() {
                    // A count's number reads in the text colour, dim
                    // only when the count is of what is hidden.
                    let style = match (hint.number, hint.faint) {
                        (true, false) => theme.text,
                        (true, true) => theme.text.add_modifier(Modifier::DIM),
                        (false, _) => faint,
                    };
                    spans.push(Span::styled(hint.what.clone(), style));
                }
                if form == Form::Worded && !hint.word.is_empty() {
                    spans.push(Span::styled(format!(" {}", hint.word), faint));
                }
                at += hint.width(form);
            }
        }
        spans.push(Span::raw(" ".repeat(width.saturating_sub(at))));
        Line::from(spans).style(theme.header)
    }
}

/// The cells `shown` hints take in `form` with `sep`-wide separators
/// between them.
fn hints_width(shown: &[HintOf], sep: usize, form: Form) -> usize {
    shown.iter().map(|hint| hint.width(form)).sum::<usize>() + sep * shown.len().saturating_sub(1)
}

/// A diff's header (ADR 0049, ADR 0060): the pair's names and nothing
/// more; its keys are on the text's bar (ADR 0069).
pub(crate) fn diff_header(text: &str) -> Header {
    Header::new(vec![(format!(" {text}"), Tone::Key)], Vec::new())
}

/// An inline thread header laid out by the shared summary engine.
pub(crate) fn expanded_header(
    app: &App,
    thread: &fathomable_core::annotations::Thread,
    marked: bool,
    expanded: bool,
    width: usize,
) -> SummaryLayout {
    let mark = app.marks().iter().find(|mark| mark.id() == thread.id());
    let summary = ThreadSummary::new(
        thread,
        mark.map(crate::app::threads::Mark::placement),
        app.user_name(),
        None,
    );
    layout(
        &summary,
        SummaryLayoutOptions {
            width,
            leading: 1,
            expanded,
            cursor: marked,
        },
        fathomable_core::clock::now(),
    )
}

/// A review thread header laid out by the shared summary engine.
pub(crate) fn entry_header(
    summary: &ThreadSummary,
    now: u64,
    cursor: bool,
    expanded: bool,
    width: usize,
) -> SummaryLayout {
    layout(
        summary,
        SummaryLayoutOptions {
            width,
            leading: 1 + NEST,
            expanded,
            cursor,
        },
        now,
    )
}

/// Draw a shared thread summary after surface-owned leading cells.
pub(crate) fn summary_line<'a>(
    theme: &Theme,
    layout: &SummaryLayout,
    mut leading: Vec<Span<'a>>,
    hovered: Option<Action>,
) -> Line<'a> {
    let on = |surface: ratatui::style::Style, accent: ratatui::style::Style| {
        let mut style = surface.patch(accent);
        style.bg = surface.bg;
        style
    };
    leading.extend(
        layout
            .spans
            .iter()
            .map(|SummarySpan { text, tone, action }| {
                let surface = if action.is_some_and(|action| Some(action) == hovered) {
                    theme.header.patch(theme.list_hover)
                } else {
                    theme.header
                };
                let style = match tone {
                    SummaryTone::Surface | SummaryTone::Action => surface,
                    SummaryTone::Info | SummaryTone::ActionKey => on(surface, theme.info),
                    SummaryTone::Lifecycle(lifecycle) => on(
                        surface,
                        mark_style(
                            theme,
                            match lifecycle {
                                fathomable_core::annotations::Lifecycle::Active => {
                                    ThreadState::Active
                                }
                                fathomable_core::annotations::Lifecycle::ResolutionProposed => {
                                    ThreadState::Proposed
                                }
                                fathomable_core::annotations::Lifecycle::Resolved => {
                                    ThreadState::Resolved
                                }
                            },
                        ),
                    ),
                    SummaryTone::Author { user: true } => on(surface, theme.thread_user),
                    SummaryTone::Author { user: false } => on(surface, theme.thread_agent),
                    SummaryTone::Preview => on(surface, theme.text),
                    SummaryTone::Chevron => on(surface, theme.info).add_modifier(Modifier::BOLD),
                };
                Span::styled(text.clone(), style)
            }),
    );
    Line::from(leading).style(theme.header)
}

/// Restore the exact hovered action after a surface applies row selection.
pub(crate) fn summary_rehover(
    theme: &Theme,
    layout: &SummaryLayout,
    line: &mut Line<'_>,
    leading_spans: usize,
    hovered: Option<Action>,
) {
    let Some(hovered) = hovered else {
        return;
    };
    let surface = theme.header.patch(theme.list_hover);
    for (span, semantic) in line.spans[leading_spans..]
        .iter_mut()
        .zip(layout.spans.iter())
    {
        if semantic.action == Some(hovered) {
            span.style = match semantic.tone {
                SummaryTone::ActionKey => {
                    let mut style = surface.patch(theme.info);
                    style.bg = surface.bg;
                    style
                }
                _ => surface,
            };
        }
    }
}

/// The review list's header (ADR 0025, ADR 0049, ADR 0066, ADR 0075):
/// `review threads`, the path while the list is one file's, then the
/// counts by colour with their words.
pub(crate) fn review_header(app: &App) -> Header {
    let review = app.review();
    let mut left = vec![(format!(" {}", review.view.title()), Tone::Key)];
    if review.view == ReviewView::Board && review.file_only {
        left.push((format!(" · {}", app.current_path().display()), Tone::Info));
    }
    left.push((" ".to_owned(), Tone::Info));
    Header::counted(
        left,
        count_hints(
            app.review_counts(review.file_only),
            review.view != ReviewView::Board || review.resolved,
        ),
        Align::Left,
    )
}

/// The review list's key bar on its bottom row (ADR 0059, ADR 0066):
/// the list keys while it has focus, else how to focus it.
pub(crate) fn review_footer(app: &App, entries: &[Entry]) -> Header {
    let place = Where::Review;
    let hints = if app.focus() == Focus::Review {
        let view = app.review().view;
        let mut hints = Vec::new();
        if view != ReviewView::Archived {
            hints.push(HintOf::keyed(place, Action::Reply, "reply"));
            if app.thread_message_editable() {
                hints.push(HintOf::keyed(place, Action::EditMessage, "edit"));
            }
        }
        if view != ReviewView::Archived && !app.review_thread_header_visible() {
            let resolved = app
                .thread_cursor()
                .thread()
                .and_then(|id| app.thread(id))
                .is_some_and(|thread| {
                    thread.lifecycle() == fathomable_core::annotations::Lifecycle::Resolved
                });
            if !resolved {
                hints.push(HintOf::keyed(place, Action::ToggleAutoResolve, "auto"));
            }
            hints.push(HintOf::keyed(
                place,
                Action::ToggleResolved,
                if resolved { "reopen" } else { "resolve" },
            ));
        }
        let resolved = app
            .thread_cursor()
            .thread()
            .and_then(|id| app.thread(id))
            .is_some_and(|thread| {
                thread.lifecycle() == fathomable_core::annotations::Lifecycle::Resolved
            });
        if !app.review_thread_header_visible() {
            if view == ReviewView::Archived {
                hints.push(HintOf::keyed(place, Action::RestoreThread, "restore"));
            } else if view == ReviewView::RecentlyResolved || resolved {
                hints.push(HintOf::keyed(place, Action::ArchiveThread, "archive"));
            }
        }
        // `z` and `Z` fold threads in file scope too (ADR 0076).
        hints.push(HintOf::keyed(place, Action::Fold, "fold"));
        hints.push(HintOf::keyed(place, Action::FoldAll, "fold all"));
        hints.push(HintOf::keyed(place, Action::Confirm, "open"));
        if view == ReviewView::Board {
            hints.push(HintOf::keyed(place, Action::ReviewResolved, "resolved"));
            hints.push(HintOf::keyed(place, Action::FileOnly, "file"));
        }
        if entries.len() > 1 {
            hints.push(HintOf::paired(
                place,
                Action::ThreadPrev,
                Action::ThreadNext,
                "threads",
            ));
        }
        // No messages to walk on a file row or a folded thread (ADR 0076).
        if app.cursor_message_count() > 1 && !app.review_cursor_folded() {
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
        vec![HintOf::new("", "click or Space w l to focus", &[])]
    };
    Header::bar(hints)
}

/// The threads pane's header (ADR 0066, ADR 0075): `threads ·
/// workspace`, then the counts by colour, with their words when the
/// row has room, against the right edge; a click on the words
/// switches the scope and one on the resolved count toggles `x`.
pub(crate) fn threads_pane_header(app: &App) -> Header {
    let scope = app.sidebar_scope();
    let left = vec![
        (" threads".to_owned(), Tone::Dir),
        (" · ".to_owned(), Tone::Info),
        (scope.word().to_owned(), Tone::Dir),
    ];
    let file_only = scope == crate::app::threads::pane::PaneScope::File;
    Header::counted(
        left,
        count_hints(
            app.review_counts(file_only),
            app.review().view != ReviewView::Board || app.review().resolved,
        ),
        Align::Right,
    )
}

/// The files pane's header (ADR 0017, ADR 0068): the repo's directory
/// name as `title`, then against the right edge its `+n -m` counts and
/// the words for the active filters, `· changed tracked ignored`, each
/// naming what is on screen. Items drop from the end as the column
/// narrows.
pub(crate) fn files_pane_header(app: &App, title: String) -> Header {
    let mut hints = Vec::new();
    let comparison_status = app.comparison_status();
    if let Some(total) = comparison_status.summary_under(Path::new("")) {
        if total.added > 0 {
            hints.push(HintOf::word(format!("+{}", total.added), Tone::Added));
        }
        if total.removed > 0 {
            hints.push(HintOf::word(format!("-{}", total.removed), Tone::Removed));
        }
    }
    for (i, word) in app.files_shown_words().into_iter().enumerate() {
        let text = if i == 0 {
            format!("· {word}")
        } else {
            word.to_owned()
        };
        hints.push(HintOf::word(text, Tone::Info));
    }
    Header::counted(vec![(title, Tone::Dir)], hints, Align::Right)
}

/// The threads pane's key bar on its bottom row while it has the keys
/// (ADR 0066): `s scope · x resolved`, and the fold keys in workspace
/// scope, where they work (ADR 0064).
pub(crate) fn threads_pane_footer(app: &App) -> Header {
    let place = Where::ThreadsPane;
    let mut hints = Vec::new();
    if let Some(thread) = app.thread_cursor().thread().and_then(|id| app.thread(id)) {
        hints.push(HintOf::keyed(place, Action::Reply, "reply"));
        let resolved = thread.lifecycle() == fathomable_core::annotations::Lifecycle::Resolved;
        if !resolved {
            hints.push(HintOf::keyed(
                place,
                Action::ToggleAutoResolve,
                "auto-resolve",
            ));
        }
        hints.push(HintOf::keyed(
            place,
            Action::ToggleResolved,
            if resolved { "reopen" } else { "resolve" },
        ));
    }
    if app.sidebar_scope() == crate::app::threads::pane::PaneScope::Workspace {
        hints.push(HintOf::keyed(place, Action::Fold, "fold"));
        hints.push(HintOf::keyed(place, Action::FoldAll, "fold all"));
    }
    hints.push(HintOf::keyed(place, Action::PaneScope, "scope"));
    hints.push(HintOf::keyed(place, Action::ReviewResolved, "resolved"));
    Header::bar(hints)
}

/// The draft's keys (ADR 0054), for the text's key bar (ADR 0067):
/// submit or save, newline, scroll, the editor, and Esc; or the
/// discard question after an Esc on a changed draft.
pub(super) fn draft_hints(compose: &Compose) -> Vec<HintOf> {
    let place = Where::Draft;
    if compose.confirming_reopen() {
        vec![
            HintOf::new("", "resolved while editing;", &[]),
            HintOf::keyed(place, Action::Confirm, "reopen and submit"),
            HintOf::keyed(place, Action::Escape, "keep editing"),
        ]
    } else if compose.confirming_discard() {
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
            HintOf::keyed(place, Action::SubmitAutoResolve, "submit + auto-resolve"),
            HintOf::keyed(place, Action::Newline, "newline"),
            HintOf::keyed(place, Action::Escape, ""),
            HintOf::paired(place, Action::ScrollUp, Action::ScrollDown, "scroll"),
            HintOf::keyed(place, Action::EditDraft, "$EDITOR"),
        ]
    }
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
            HintOf::new("f", "file", &[Action::FileOnly]),
            HintOf::new("x", "resolved", &[Action::ReviewResolved]),
            HintOf::new("k/j", "threads", &[Action::ThreadPrev, Action::ThreadNext]),
        ])
    }

    #[test]
    fn a_bar_reads_from_the_left_and_fills_the_row() -> anyhow::Result<()> {
        let line = bar().line(&theme()?, 40);
        let text = text(&line);
        assert_eq!(display_width(&text), 40);
        assert_eq!(text.trim_end(), " f file · x resolved · k/j threads");
        Ok(())
    }

    #[test]
    fn a_bar_drops_hints_from_the_end_when_narrow() -> anyhow::Result<()> {
        let theme = theme()?;
        let line = bar().line(&theme, 24);
        assert_eq!(text(&line).trim_end(), " f file · x resolved");
        let line = bar().line(&theme, 4);
        assert_eq!(text(&line), "    ", "no hint fits");
        Ok(())
    }

    #[test]
    fn a_click_on_a_bar_hint_runs_it_and_a_pair_splits_at_the_slash() {
        let bar = bar();
        assert_eq!(bar.action_at(40, 1), Some(Action::FileOnly));
        assert_eq!(bar.action_at(40, 10), Some(Action::ReviewResolved));
        let threads = display_width(" f file · x resolved · ");
        assert_eq!(bar.action_at(40, threads), Some(Action::ThreadPrev));
        assert_eq!(bar.action_at(40, threads + 2), Some(Action::ThreadNext));
        assert_eq!(bar.action_at(40, 0), None, "the margin runs nothing");
        assert_eq!(
            bar.action_at(24, threads),
            None,
            "a dropped hint is not there"
        );
    }

    /// A header row inside a thread block paints its gutter cells on
    /// `ui.header` too (ADR 0064): the first cell of the row and the
    /// first cell of the words share a background, and a message row's
    /// gutter stays on `thread.inline`.
    #[test]
    fn a_thread_header_reaches_the_left_edge() -> anyhow::Result<()> {
        use crate::app::testing::{self, source_app};

        let dir = testing::workspace("header-edge", testing::README)?;
        let mut app = source_app(&dir)?;
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        app.compose_insert("mine");
        app.compose_submit();
        let id = app.file_threads()[0].clone();
        app.goto_message(id, 0);
        let theme = theme()?;
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        let text_x = buffer.area.width - u16::try_from(app.view().layout().width())?;
        let header_y = (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("● ▾")
            })
            .ok_or_else(|| anyhow::anyhow!("the header row"))?;
        let gutter_x = text_x - 4;
        assert_eq!(
            buffer[(gutter_x, header_y)].bg,
            theme.header.bg.unwrap_or_default(),
            "the gutter cell is on ui.header"
        );
        assert_eq!(
            buffer[(gutter_x, header_y)].bg,
            buffer[(text_x, header_y)].bg
        );
        assert_eq!(
            buffer[(gutter_x, header_y + 1)].bg,
            theme.thread_inline.bg.unwrap_or_default(),
            "a message row's gutter stays on thread.inline"
        );
        assert_ne!(
            theme.header.bg, theme.thread_inline.bg,
            "the test can tell them apart"
        );
        Ok(())
    }

    #[test]
    fn a_header_keeps_its_hints_at_the_right_edge() -> anyhow::Result<()> {
        let header = Header::new(
            vec![(" review".to_owned(), Tone::Key)],
            vec![HintOf::new("x", "resolved", &[Action::ReviewResolved])],
        );
        let line = header.line(&theme()?, 30);
        let text = text(&line);
        assert_eq!(display_width(&text), 30);
        assert_eq!(text.trim_end(), " review            x resolved");
        assert_eq!(header.action_at(30, 25), Some(Action::ReviewResolved));
        Ok(())
    }

    #[test]
    fn thread_action_keys_trail_words_in_the_subdued_style() -> anyhow::Result<()> {
        use crate::app::draw::author::CURSOR_BAR;
        use crate::app::testing::{self, source_app};

        let dir = testing::workspace("header-action-style", testing::README)?;
        let mut app = source_app(&dir)?;
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        app.compose_insert("mine");
        app.compose_submit();
        let id = app.file_threads()[0].clone();
        let thread = app.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
        let layout = expanded_header(&app, thread, true, true, 80);
        let theme = theme()?;
        let line = summary_line(
            &theme,
            &layout,
            vec![Span::styled(CURSOR_BAR, theme.thread_cursor)],
            None,
        );
        let action = line
            .spans
            .iter()
            .position(|span| span.content == "Auto-resolve")
            .ok_or_else(|| anyhow::anyhow!("action word"))?;
        let key = line
            .spans
            .iter()
            .position(|span| span.content == "  R")
            .ok_or_else(|| anyhow::anyhow!("action key"))?;
        assert!(action < key);
        assert_eq!(line.spans[key].style.fg, theme.info.fg);
        assert_eq!(line.spans[key].style.bg, theme.header.bg);
        assert!(!text(&line).contains('['));
        Ok(())
    }
}
