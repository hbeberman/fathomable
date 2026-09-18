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
//! engine from `app::threads::summary` and remain factual (ADR 0086).
//! Every header row draws on `ui.header`.
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

use crate::app::draw::counts::passive_count_hints;
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
    /// A shorter form of `key` used only after count words have dropped.
    compact: Option<String>,
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
            compact: None,
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
            compact: None,
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
            compact: None,
            what: String::new(),
            word: String::new(),
            actions: Vec::new(),
            tone: Some(tone),
            faint: false,
            gap: false,
            number: false,
        }
    }

    /// A passive word that shortens only when its full form no longer fits.
    fn responsive_word(text: &'static str, compact: &'static str, tone: Tone) -> Self {
        Self {
            key: text.to_owned(),
            compact: Some(compact.to_owned()),
            what: String::new(),
            word: String::new(),
            actions: Vec::new(),
            tone: Some(tone),
            faint: true,
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

    /// The columns the hint takes in `form`: key, action, their gap,
    /// and the word when worded.
    fn width(&self, form: Form) -> usize {
        display_width(self.key_in(form))
            + display_width(&self.what)
            + usize::from(self.gap && !self.key.is_empty() && !self.what.is_empty())
            + usize::from(form == Form::Worded) * self.word_width()
    }

    fn key_in(&self, form: Form) -> &str {
        if form == Form::Compact {
            self.compact.as_deref().unwrap_or(&self.key)
        } else {
            &self.key
        }
    }
}

/// Whether a header's hints are drawn with their words after the
/// counts (ADR 0075): worded when every hint fits that way, else bare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Worded,
    Bare,
    Compact,
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
    /// Hints retained after `hints` while optional hints are dropped.
    tail: Vec<HintOf>,
    align: Align,
    /// What joins the hints: ` · ` for keys, a space for counts.
    sep: &'static str,
    /// Key bars read as action then hotkey; headers keep key then fact.
    action_first: bool,
}

impl Header {
    /// Words on the left, hints against the right edge.
    pub(super) fn new(left: Vec<(String, Tone)>, hints: Vec<HintOf>) -> Self {
        Self {
            left,
            hints,
            tail: Vec::new(),
            align: Align::Right,
            sep: " · ",
            action_first: false,
        }
    }

    /// A key bar: no words, action then hotkey from the left edge.
    pub(super) fn bar(hints: Vec<HintOf>) -> Self {
        Self {
            left: Vec::new(),
            hints,
            tail: Vec::new(),
            align: Align::Left,
            sep: " · ",
            action_first: true,
        }
    }

    /// Words, then counts against the right edge, a space apart (ADR
    /// 0066).
    pub(super) fn counted(left: Vec<(String, Tone)>, counts: Vec<HintOf>, align: Align) -> Self {
        Self {
            left,
            hints: counts,
            tail: Vec::new(),
            align,
            sep: " ",
            action_first: false,
        }
    }

    /// Words and optional state, then a required tail against the right edge.
    fn counted_with_tail(left: Vec<(String, Tone)>, hints: Vec<HintOf>, tail: Vec<HintOf>) -> Self {
        Self {
            left,
            hints,
            tail,
            align: Align::Right,
            sep: " ",
            action_first: false,
        }
    }

    /// The cells the left part takes.
    pub(crate) fn left_width(&self) -> usize {
        self.left.iter().map(|(text, _)| display_width(text)).sum()
    }

    /// The cells occupied by the clickable title at the start of the row.
    pub(crate) fn title_width(&self) -> usize {
        self.left.first().map_or(0, |(text, _)| display_width(text))
    }

    /// The hints that fit after the left part on `width` cells, the
    /// column the first starts at, and their form (ADR 0075): every
    /// hint with its word when that fits, then without count words, then
    /// with responsive words shortened, dropping optional hints before the
    /// retained tail.
    fn shown(&self, width: usize) -> Option<(usize, &[HintOf], &[HintOf], Form)> {
        let used = self.left_width();
        let free = width.saturating_sub(used);
        let sep = display_width(self.sep);
        let margin = if self.left.is_empty() {
            BAR_MARGIN
        } else {
            HINT_MARGIN
        };
        let fits = |hints: &[HintOf], tail: &[HintOf], form: Form| {
            free >= hints_width(hints, tail, sep, form) + margin
        };
        let has_word = self
            .hints
            .iter()
            .chain(&self.tail)
            .any(|hint| !hint.word.is_empty());
        let has_compact = self
            .hints
            .iter()
            .chain(&self.tail)
            .any(|hint| hint.compact.is_some());
        let has_any = !self.hints.is_empty() || !self.tail.is_empty();
        let (hints, tail, form) = if has_word && fits(&self.hints, &self.tail, Form::Worded) {
            (self.hints.as_slice(), self.tail.as_slice(), Form::Worded)
        } else if has_any && fits(&self.hints, &self.tail, Form::Bare) {
            (self.hints.as_slice(), self.tail.as_slice(), Form::Bare)
        } else if has_compact && fits(&self.hints, &self.tail, Form::Compact) {
            (self.hints.as_slice(), self.tail.as_slice(), Form::Compact)
        } else {
            let form = if has_compact {
                Form::Compact
            } else {
                Form::Bare
            };
            let hints = (0..=self.hints.len())
                .rev()
                .map(|n| &self.hints[..n])
                .find(|hints| {
                    (!hints.is_empty() || !self.tail.is_empty()) && fits(hints, &self.tail, form)
                });
            if let Some(hints) = hints {
                (hints, self.tail.as_slice(), form)
            } else {
                let tail = (1..=self.tail.len())
                    .rev()
                    .map(|n| &self.tail[..n])
                    .find(|tail| fits(&[], tail, form))?;
                (&[][..], tail, form)
            }
        };
        let start = match self.align {
            Align::Right => used + (free - hints_width(hints, tail, sep, form) - 1),
            Align::Left => used + 1,
        };
        Some((start, hints, tail, form))
    }

    /// The action a click at `column` on a `width`-cell header runs: the
    /// hint under the pointer, its left or right half for a pair.
    pub(crate) fn action_at(&self, width: usize, column: usize) -> Option<Action> {
        let (mut at, hints, tail, form) = self.shown(width)?;
        let sep = display_width(self.sep);
        for (i, hint) in hints.iter().chain(tail).enumerate() {
            if i > 0 {
                at += sep;
            }
            let end = at + hint.width(form);
            if column >= at && column < end {
                let offset = column - at;
                let key = hint.key_in(form);
                let slash = key.find('/').map(|byte| display_width(&key[..byte]));
                return match (hint.actions.as_slice(), slash) {
                    ([first, second], Some(slash)) => {
                        let key_start = if self.action_first {
                            display_width(&hint.what)
                                + usize::from(
                                    hint.gap && !hint.key.is_empty() && !hint.what.is_empty(),
                                )
                                + usize::from(form == Form::Worded) * hint.word_width()
                        } else {
                            0
                        };
                        Some(if offset < key_start || offset - key_start <= slash {
                            *first
                        } else {
                            *second
                        })
                    }
                    (actions, _) => actions.first().copied(),
                };
            }
            at = end;
        }
        None
    }

    /// The header as one drawn row on `ui.header`: the words, then the
    /// hints joined by the separator, padded to `width` so the surface
    /// reaches the right edge.
    pub(super) fn line(&self, theme: &Theme, width: usize) -> Line<'static> {
        self.line_with_hovers(theme, width, false, None)
    }

    /// Draw the header with hover behind its clickable left label.
    pub(super) fn line_with_left_hover(
        &self,
        theme: &Theme,
        width: usize,
        left_hovered: bool,
    ) -> Line<'static> {
        self.line_with_hovers(theme, width, left_hovered, None)
    }

    /// Draw a key bar with hover behind the action under the pointer.
    pub(super) fn line_with_action_hover(
        &self,
        theme: &Theme,
        width: usize,
        hovered: Option<Action>,
    ) -> Line<'static> {
        self.line_with_hovers(theme, width, false, hovered)
    }

    fn line_with_hovers(
        &self,
        theme: &Theme,
        width: usize,
        left_hovered: bool,
        hovered: Option<Action>,
    ) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = self
            .left
            .iter()
            .enumerate()
            .map(|(index, (text, tone))| {
                let style = if left_hovered && index == 0 {
                    tone_style(theme, *tone).patch(theme.list_hover)
                } else {
                    tone_style(theme, *tone)
                };
                Span::styled(text.clone(), style)
            })
            .collect();
        let mut at = self.left_width();
        if let Some((start, hints, tail, form)) = self.shown(width) {
            spans.push(Span::raw(" ".repeat(start - at)));
            at = start;
            let faint = theme.info.add_modifier(Modifier::DIM);
            let sep = display_width(self.sep);
            for (i, hint) in hints.iter().chain(tail).enumerate() {
                if i > 0 {
                    spans.push(Span::styled(self.sep, faint));
                    at += sep;
                }
                let hint_hovered = hovered.is_some_and(|action| hint.actions.contains(&action));
                let surface = if hint_hovered {
                    theme.header.patch(theme.list_hover)
                } else {
                    theme.header
                };
                let on_surface = |accent: ratatui::style::Style| {
                    if !hint_hovered {
                        return accent;
                    }
                    let mut style = surface.patch(accent);
                    style.bg = surface.bg;
                    style
                };
                let key = hint.key_in(form);
                let key_span = || {
                    let mut style = hint.tone.map_or(theme.info, |tone| tone_style(theme, tone));
                    if hint.faint {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    Span::styled(key.to_owned(), on_surface(style))
                };
                let what_span = || {
                    // A count's number reads in the text colour, dim
                    // only when the count is of what is hidden.
                    let style = match (hint.number, hint.faint) {
                        (true, false) => theme.text,
                        (true, true) => theme.text.add_modifier(Modifier::DIM),
                        (false, _) => faint,
                    };
                    Span::styled(hint.what.clone(), on_surface(style))
                };
                let gap = hint.gap && !key.is_empty() && !hint.what.is_empty();
                if self.action_first {
                    if !hint.what.is_empty() {
                        spans.push(what_span());
                    }
                    if form == Form::Worded && !hint.word.is_empty() {
                        spans.push(Span::styled(format!(" {}", hint.word), on_surface(faint)));
                    }
                    if gap {
                        spans.push(Span::styled(
                            " ",
                            if hint_hovered { surface } else { faint },
                        ));
                    }
                    if !key.is_empty() {
                        spans.push(key_span());
                    }
                } else {
                    if !key.is_empty() {
                        spans.push(key_span());
                    }
                    if gap {
                        spans.push(Span::styled(
                            " ",
                            if hint_hovered { surface } else { faint },
                        ));
                    }
                    if !hint.what.is_empty() {
                        spans.push(what_span());
                    }
                    if form == Form::Worded && !hint.word.is_empty() {
                        spans.push(Span::styled(format!(" {}", hint.word), on_surface(faint)));
                    }
                }
                at += hint.width(form);
            }
        }
        spans.push(Span::raw(" ".repeat(width.saturating_sub(at))));
        Line::from(spans).style(theme.header)
    }
}

fn tone_style(theme: &Theme, tone: Tone) -> ratatui::style::Style {
    match tone {
        Tone::Key => theme.popup_key,
        Tone::Info => theme.info,
        Tone::Dir => theme.sidebar_dir.add_modifier(Modifier::BOLD),
        Tone::Mark(state) => mark_style(theme, state),
        Tone::Added => theme.diff_plus,
        Tone::Removed => theme.diff_minus,
    }
}

/// The cells `hints` and `tail` take with `sep`-wide separators.
fn hints_width(hints: &[HintOf], tail: &[HintOf], sep: usize, form: Form) -> usize {
    let count = hints.len() + tail.len();
    hints
        .iter()
        .chain(tail)
        .map(|hint| hint.width(form))
        .sum::<usize>()
        + sep * count.saturating_sub(1)
}

/// An inline thread header laid out by the shared summary engine.
pub(crate) fn expanded_header(
    app: &App,
    thread: &fathomable_core::annotations::Thread,
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
        },
        fathomable_core::clock::now(),
    )
}

/// A review thread header laid out by the shared summary engine.
pub(crate) fn entry_header(
    summary: &ThreadSummary,
    now: u64,
    expanded: bool,
    width: usize,
) -> SummaryLayout {
    layout(
        summary,
        SummaryLayoutOptions {
            width,
            leading: 1 + NEST,
            expanded,
        },
        now,
    )
}

/// Draw a shared thread summary after surface-owned leading cells.
pub(crate) fn summary_line<'a>(
    theme: &Theme,
    layout: &SummaryLayout,
    mut leading: Vec<Span<'a>>,
) -> Line<'a> {
    let on = |surface: ratatui::style::Style, accent: ratatui::style::Style| {
        let mut style = surface.patch(accent);
        style.bg = surface.bg;
        style
    };
    leading.extend(layout.spans.iter().map(|SummarySpan { text, tone }| {
        let surface = theme.header;
        let style = match tone {
            SummaryTone::Surface => surface,
            SummaryTone::Info => on(surface, theme.info),
            SummaryTone::Status => on(surface, theme.info.add_modifier(Modifier::DIM)),
            SummaryTone::Lifecycle(lifecycle) => on(
                surface,
                mark_style(
                    theme,
                    match lifecycle {
                        fathomable_core::annotations::Lifecycle::Active => ThreadState::Active,
                        fathomable_core::annotations::Lifecycle::ResolutionProposed => {
                            ThreadState::Proposed
                        }
                        fathomable_core::annotations::Lifecycle::Resolved => ThreadState::Resolved,
                    },
                ),
            ),
            SummaryTone::Author { user: true } => on(surface, theme.thread_user),
            SummaryTone::Author { user: false } => on(surface, theme.thread_agent),
            SummaryTone::Preview => on(surface, theme.text),
            SummaryTone::Chevron => on(surface, theme.info).add_modifier(Modifier::BOLD),
        };
        Span::styled(text.clone(), style)
    }));
    Line::from(leading).style(theme.header)
}

/// The review list's header (ADR 0025, ADR 0066, ADR 0075).
///
/// The normal board draws a clickable `Reviews` title, then passive scope
/// and lifecycle counts against the right edge. History views keep their
/// specific title and passive counts.
pub(crate) fn review_header(app: &App) -> Header {
    let review = app.review();
    let counts = passive_count_hints(
        app.review_counts(review.file_only),
        review.view != ReviewView::Board || review.resolved,
    );
    if review.view == ReviewView::Board {
        let (scope, compact) = if review.file_only {
            ("file", "f")
        } else {
            ("workspace", "w")
        };
        Header::counted_with_tail(
            vec![(" Reviews".to_owned(), Tone::Dir)],
            vec![HintOf::responsive_word(scope, compact, Tone::Info)],
            counts,
        )
    } else {
        Header::counted(
            vec![(format!(" {}", review.view.title()), Tone::Key)],
            counts,
            Align::Right,
        )
    }
}

/// The file surface's header: a clickable title, the current path, and
/// passive lifecycle counts for that file.
pub(crate) fn file_header(app: &App) -> Header {
    let directory = app.directory_path();
    let filename = directory.map_or_else(
        || {
            let path = app.current_path();
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        },
        |path| format!("{}/", path.display()),
    );
    let counts = directory.map_or_else(
        || passive_count_hints(app.review_counts(true), app.stubs_resolved()),
        |_| Vec::new(),
    );
    Header::counted(
        vec![
            (" File".to_owned(), Tone::Dir),
            (format!("  {filename}"), Tone::Info),
        ],
        counts,
        Align::Right,
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
        let cursor_thread = app.thread_cursor().thread().and_then(|id| app.thread(id));
        let resolved = cursor_thread.is_some_and(|thread| {
            thread.lifecycle() == fathomable_core::annotations::Lifecycle::Resolved
        });
        if view != ReviewView::Archived {
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
        if app.review_cursor_on_thread() {
            if cursor_thread.is_some_and(fathomable_core::annotations::Thread::is_archived) {
                hints.push(HintOf::keyed(place, Action::RestoreThread, "restore"));
            } else if resolved {
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

/// The threads pane's header (ADR 0066, ADR 0075): `Threads` at the left,
/// then passive scope and lifecycle counts against the right edge. Scope
/// shortens to `f` or `w` after count words drop and before it disappears.
pub(crate) fn threads_pane_header(app: &App) -> Header {
    let scope = app.sidebar_scope();
    let file_only = scope == crate::app::threads::pane::PaneScope::File;
    Header::counted_with_tail(
        vec![(" Threads".to_owned(), Tone::Dir)],
        vec![HintOf::responsive_word(
            scope.word(),
            scope.short_word(),
            Tone::Info,
        )],
        passive_count_hints(
            app.review_counts(file_only),
            app.review().view != ReviewView::Board || app.review().resolved,
        ),
    )
}

/// The files pane's header (ADR 0017, ADR 0068): `Files` at the left,
/// then the active filter words before the `+n -m` totals at the right.
/// Filter words drop before the totals as the pane narrows.
pub(crate) fn files_pane_header(app: &App) -> Header {
    let filters = app
        .files_shown_words()
        .into_iter()
        .map(|word| HintOf::word(word.to_owned(), Tone::Info))
        .collect();
    let mut counts = Vec::new();
    let comparison_status = app.comparison_status();
    if let Some(total) = comparison_status.summary_under(Path::new("")) {
        if total.added > 0 {
            counts.push(HintOf::word(format!("+{}", total.added), Tone::Added));
        }
        if total.removed > 0 {
            counts.push(HintOf::word(format!("-{}", total.removed), Tone::Removed));
        }
    }
    Header::counted_with_tail(vec![(" Files".to_owned(), Tone::Dir)], filters, counts)
}

/// The threads pane's key bar on its bottom row while it has the keys
/// (ADR 0066): `scope s · resolved x`, and the fold keys in workspace
/// scope, where they work (ADR 0064).
pub(crate) fn threads_pane_footer(app: &App) -> Header {
    let place = Where::ThreadsPane;
    let mut hints = Vec::new();
    if let Some(thread) = app.threads_pane_cursor_thread() {
        let resolved = thread.lifecycle() == fathomable_core::annotations::Lifecycle::Resolved;
        if thread.is_archived() {
            hints.push(HintOf::keyed(place, Action::RestoreThread, "restore"));
        } else if resolved {
            hints.push(HintOf::keyed(place, Action::ToggleResolved, "reopen"));
            if !thread.is_archived() {
                hints.push(HintOf::keyed(place, Action::ArchiveThread, "archive"));
            }
            hints.push(HintOf::keyed(place, Action::Reply, "reply"));
        } else {
            hints.push(HintOf::keyed(place, Action::Reply, "reply"));
            hints.push(HintOf::keyed(
                place,
                Action::ToggleAutoResolve,
                "auto-resolve",
            ));
            hints.push(HintOf::keyed(place, Action::ToggleResolved, "resolve"));
        }
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
            HintOf::new("s", "file", &[Action::FileOnly]),
            HintOf::new("x", "resolved", &[Action::ReviewResolved]),
            HintOf::new("k/j", "threads", &[Action::ThreadPrev, Action::ThreadNext]),
        ])
    }

    #[test]
    fn a_bar_reads_from_the_left_and_fills_the_row() -> anyhow::Result<()> {
        let line = bar().line(&theme()?, 40);
        let text = text(&line);
        assert_eq!(display_width(&text), 40);
        assert_eq!(text.trim_end(), " file s · resolved x · threads k/j");
        Ok(())
    }

    #[test]
    fn a_bar_drops_hints_from_the_end_when_narrow() -> anyhow::Result<()> {
        let theme = theme()?;
        let line = bar().line(&theme, 24);
        assert_eq!(text(&line).trim_end(), " file s · resolved x");
        let line = bar().line(&theme, 4);
        assert_eq!(text(&line), "    ", "no hint fits");
        Ok(())
    }

    #[test]
    fn a_click_on_a_bar_hint_runs_it_and_a_pair_splits_at_the_slash() {
        let bar = bar();
        assert_eq!(bar.action_at(40, 1), Some(Action::FileOnly));
        assert_eq!(bar.action_at(40, 10), Some(Action::ReviewResolved));
        let threads = display_width(" file s · resolved x · ");
        assert_eq!(bar.action_at(40, threads), Some(Action::ThreadPrev));
        assert_eq!(
            bar.action_at(40, threads + display_width("threads k/")),
            Some(Action::ThreadNext)
        );
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
    fn a_bar_action_reads_as_one_button_on_hover() -> anyhow::Result<()> {
        use ratatui::style::{Color, Style};

        let mut theme = theme()?;
        let hover = Color::Rgb(1, 2, 3);
        theme.list_hover = Style::default().bg(hover);
        let line = bar().line_with_action_hover(&theme, 40, Some(Action::FileOnly));
        for part in ["file", " ", "s"] {
            assert!(
                line.spans
                    .iter()
                    .any(|span| span.content == part && span.style.bg == Some(hover)),
                "{part:?} is part of the hovered button: {line:?}"
            );
        }
        let separator = line
            .spans
            .iter()
            .find(|span| span.content == " · ")
            .ok_or_else(|| anyhow::anyhow!("separator"))?;
        assert_ne!(separator.style.bg, Some(hover));
        Ok(())
    }

    #[test]
    fn thread_headers_show_facts_without_cleanup_controls() -> anyhow::Result<()> {
        use crate::app::draw::author::CURSOR_BAR;
        use crate::app::testing::{self, source_app};

        let dir = testing::workspace("header-action-style", testing::README)?;
        let mut app = source_app(&dir)?;
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        app.compose_insert("mine");
        app.compose_submit();
        let id = app.file_threads()[0].clone();
        app.goto_message(id.clone(), 0);
        app.thread_toggle_resolved();
        let thread = app.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
        let layout = expanded_header(&app, thread, true, 80);
        let theme = theme()?;
        let line = summary_line(
            &theme,
            &layout,
            vec![Span::styled(CURSOR_BAR, theme.thread_cursor)],
        );
        let rendered = text(&line);
        assert!(!rendered.contains("Archive"), "{rendered}");
        assert!(!rendered.contains("Restore"), "{rendered}");
        Ok(())
    }

    #[test]
    fn thread_lifecycle_status_is_dim_and_passive() -> anyhow::Result<()> {
        use crate::app::draw::author::CURSOR_BAR;
        use crate::app::testing::{self, source_app};

        let dir = testing::workspace("header-status-style", testing::README)?;
        let mut app = source_app(&dir)?;
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        app.compose_insert("mine");
        app.compose_submit();
        let id = app.file_threads()[0].clone();
        app.goto_message(id.clone(), 0);
        app.thread_toggle_auto_resolve();
        let thread = app.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
        let layout = expanded_header(&app, thread, true, 80);
        let line = summary_line(&theme()?, &layout, vec![Span::raw(CURSOR_BAR)]);
        let status = line
            .spans
            .iter()
            .find(|span| span.content == "autoresolve")
            .ok_or_else(|| anyhow::anyhow!("autoresolve status"))?;
        assert!(status.style.add_modifier.contains(Modifier::DIM));
        Ok(())
    }
}
