// @okf-doc: /decisions/0064-hints-you-can-press.md
//! Pane chrome plus shared inline and review thread summaries.
//!
//! A [`Header`] is the words on a pane's chrome row and the hints after
//! them, built once so the drawing and the mouse agree on where each
//! hint is. The review list's and the threads pane's headers carry
//! their counts by colour (ADR 0066) as hints, so the resolved count
//! takes a click; a count's word (ADR 0075, [`super::counts`]) is drawn
//! when every count's fits and dropped with the others when not. Keys
//! live on a bar along a focused pane's bottom row,
//! left-aligned: [`review_footer`], [`threads_pane_footer`], and the
//! text's in [`super::bar`] (ADR 0067). Thread rows use the summary
//! engine from `app::threads::summary` and remain factual (ADR 0086).
//! Every header row draws on `ui.header`.
//!
//! A key hint is drawn only where pressing that key now, with the focus
//! and cursor as they are, runs the action it names (ADR 0064). An inactive
//! pane gives its footer row back to content. The binding table says what a
//! key is called; each builder here says whether it works.

use std::path::Path;

use fathomable_core::config::DiffMode;
use fathomable_core::layout::display_width;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::app::draw::counts::passive_count_hints;
use crate::app::draw::nest::NEST;
use crate::app::draw::{Theme, mark_style, on_surface};
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
    /// A pane name with a reserved leading focus-marker cell.
    Pane,
    Mark(ThreadState),
    /// The diff colours: the files pane header's `+n` and `-m`.
    Added,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    DiffMode,
}

#[derive(Debug, Clone, Copy)]
enum Hover {
    None,
    Title,
    Action(Action),
    Control(Control),
}

/// A header hint with what a click on it runs (ADR 0050): nothing for
/// words alone, two actions for an `a/b` pair split at the slash, or
/// explicit targets in a grouped key legend. A count (ADR 0066) is a
/// circle in a state's colour with its number against it and a word
/// after the number (ADR 0075) that the header draws only when every
/// count's fits.
#[derive(Debug, Clone)]
pub(crate) struct HintOf {
    key: String,
    /// A shorter form of `key` used only after count words have dropped.
    compact: Option<String>,
    what: String,
    /// The word after `what`, a space between; empty for most hints.
    word: String,
    actions: Vec<Action>,
    key_targets: Vec<KeyTarget>,
    control: Option<Control>,
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

#[derive(Debug, Clone)]
struct KeyTarget {
    start: usize,
    end: usize,
    action: Action,
}

impl HintOf {
    pub(super) fn new(key: impl Into<String>, what: impl Into<String>, actions: &[Action]) -> Self {
        Self {
            key: key.into(),
            compact: None,
            what: what.into(),
            word: String::new(),
            actions: actions.to_vec(),
            key_targets: Vec::new(),
            control: None,
            tone: None,
            faint: false,
            gap: true,
            number: false,
        }
    }

    pub(crate) fn keyed(place: Where, action: Action, what: &'static str) -> Self {
        Self::new(key_of(place, action), what, &[action])
    }

    fn control(key: &'static str, what: &'static str, action: Action) -> Self {
        let mut control = Self::new(key, what, &[action]);
        control.tone = Some(Tone::Key);
        control
    }

    pub(crate) fn paired(place: Where, a: Action, b: Action, what: &'static str) -> Self {
        Self::new(pair(place, a, b), what, &[a, b])
    }

    /// A grouped key legend whose named segments retain distinct click targets.
    pub(crate) fn mapped(
        key: &'static str,
        what: &'static str,
        targets: &[(&'static str, Action)],
    ) -> Self {
        let mut hint = Self::new(key, what, &[]);
        for (segment, action) in targets {
            let Some(byte) = key.find(segment) else {
                continue;
            };
            let start = display_width(&key[..byte]);
            hint.key_targets.push(KeyTarget {
                start,
                end: start + display_width(segment),
                action: *action,
            });
            if !hint.actions.contains(action) {
                hint.actions.push(*action);
            }
        }
        hint
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
            key_targets: Vec::new(),
            control: None,
            tone: Some(Tone::Mark(state)),
            faint,
            gap: true,
            number: true,
        }
    }

    /// Text in `tone` with nothing after it and no click: a count or a
    /// compact state marker on the files pane's header (ADR 0068).
    fn word(text: String, tone: Tone) -> Self {
        Self {
            key: text,
            compact: None,
            what: String::new(),
            word: String::new(),
            actions: Vec::new(),
            key_targets: Vec::new(),
            control: None,
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
            key_targets: Vec::new(),
            control: None,
            tone: Some(tone),
            faint: true,
            gap: false,
            number: false,
        }
    }

    fn diff_mode(mode: DiffMode) -> Self {
        Self {
            key: format!("Diff: {mode}"),
            compact: None,
            what: String::new(),
            word: String::new(),
            actions: Vec::new(),
            key_targets: Vec::new(),
            control: Some(Control::DiffMode),
            tone: Some(Tone::Info),
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

/// A pane header: the words on the left and the hints after them,
/// built once so the drawing and the mouse agree on where each hint is
/// (ADR 0050).
#[derive(Debug, Clone)]
pub(crate) struct Header {
    left: Vec<(String, Tone)>,
    /// Leading left spans that remain when passive left context must yield.
    required_left: usize,
    hints: Vec<HintOf>,
    /// Hints retained after `hints` while optional hints are dropped.
    tail: Vec<HintOf>,
    align: Align,
    /// What joins the hints: ` · ` for keys, a space for counts.
    sep: &'static str,
    /// Key bars read as action then hotkey; headers keep key then fact.
    action_first: bool,
    /// Empty cells before left-aligned controls.
    left_pad: usize,
}

type Shown<'a> = (usize, usize, &'a [HintOf], &'a [HintOf], Form);

impl Header {
    /// Words on the left, hints against the right edge.
    pub(super) fn new(left: Vec<(String, Tone)>, hints: Vec<HintOf>) -> Self {
        let required_left = left.len();
        Self {
            left,
            required_left,
            hints,
            tail: Vec::new(),
            align: Align::Right,
            sep: " · ",
            action_first: false,
            left_pad: 0,
        }
    }

    /// A key bar: no words, action then hotkey from the left edge.
    pub(super) fn bar(hints: Vec<HintOf>) -> Self {
        Self {
            left: Vec::new(),
            required_left: 0,
            hints,
            tail: Vec::new(),
            align: Align::Left,
            sep: " · ",
            action_first: true,
            left_pad: 1,
        }
    }

    /// Words, then counts against an edge, used by count layout tests.
    #[cfg(test)]
    pub(super) fn counted(left: Vec<(String, Tone)>, counts: Vec<HintOf>, align: Align) -> Self {
        let required_left = left.len();
        Self {
            left,
            required_left,
            hints: counts,
            tail: Vec::new(),
            align,
            sep: " ",
            action_first: false,
            left_pad: usize::from(align == Align::Left),
        }
    }

    /// Words and optional state, then a required tail against the right edge.
    fn counted_with_tail(left: Vec<(String, Tone)>, hints: Vec<HintOf>, tail: Vec<HintOf>) -> Self {
        let required_left = left.len();
        Self {
            left,
            required_left,
            hints,
            tail,
            align: Align::Right,
            sep: " ",
            action_first: false,
            left_pad: 0,
        }
    }

    /// The cells the left part takes.
    pub(crate) fn left_width(&self) -> usize {
        self.left.iter().map(|(text, _)| display_width(text)).sum()
    }

    fn left_width_for(&self, count: usize) -> usize {
        self.left
            .iter()
            .take(count)
            .map(|(text, _)| display_width(text))
            .sum()
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
    fn shown(&self, width: usize) -> Option<Shown<'_>> {
        let full = self.shown_after(width, self.left_width());
        if self.required_left < self.left.len()
            && full
                .as_ref()
                .is_none_or(|(_, _, tail, _)| tail.len() < self.tail.len())
            && let Some((start, hints, tail, form)) =
                self.shown_after(width, self.left_width_for(self.required_left))
        {
            return Some((self.required_left, start, hints, tail, form));
        }
        full.map(|(start, hints, tail, form)| (self.left.len(), start, hints, tail, form))
    }

    fn shown_after(
        &self,
        width: usize,
        used: usize,
    ) -> Option<(usize, &[HintOf], &[HintOf], Form)> {
        let free = width.saturating_sub(used);
        let sep = display_width(self.sep);
        let margin = if self.left.is_empty() {
            self.left_pad + 1
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
            Align::Left => used + self.left_pad,
        };
        Some((start, hints, tail, form))
    }

    /// The action a click at `column` on a `width`-cell header runs: the
    /// hint under the pointer, its left or right half for a pair.
    pub(crate) fn action_at(&self, width: usize, column: usize) -> Option<Action> {
        let (_, mut at, hints, tail, form) = self.shown(width)?;
        let sep = display_width(self.sep);
        for (i, hint) in hints.iter().chain(tail).enumerate() {
            if i > 0 {
                at += sep;
            }
            let end = at + hint.width(form);
            if column >= at && column < end {
                let offset = column - at;
                let key = hint.key_in(form);
                let key_start = if self.action_first {
                    display_width(&hint.what)
                        + usize::from(hint.gap && !hint.key.is_empty() && !hint.what.is_empty())
                        + usize::from(form == Form::Worded) * hint.word_width()
                } else {
                    0
                };
                if !hint.key_targets.is_empty() {
                    let key_offset = offset.checked_sub(key_start);
                    return key_offset.and_then(|offset| {
                        hint.key_targets
                            .iter()
                            .find(|target| offset >= target.start && offset < target.end)
                            .map(|target| target.action)
                    });
                }
                let slash = key.find('/').map(|byte| display_width(&key[..byte]));
                return match (hint.actions.as_slice(), slash) {
                    ([first, second], Some(slash)) => {
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

    /// The header control under `column`, when it was retained at `width`.
    pub(crate) fn control_at(&self, width: usize, column: usize) -> Option<Control> {
        let (_, mut at, hints, tail, form) = self.shown(width)?;
        let sep = display_width(self.sep);
        for (i, hint) in hints.iter().chain(tail).enumerate() {
            if i > 0 {
                at += sep;
            }
            let end = at + hint.width(form);
            if column >= at && column < end {
                return hint.control;
            }
            at = end;
        }
        None
    }

    /// The retained control's half-open column range.
    pub(crate) fn control_bounds(&self, width: usize, control: Control) -> Option<(usize, usize)> {
        let (_, mut at, hints, tail, form) = self.shown(width)?;
        let sep = display_width(self.sep);
        for (i, hint) in hints.iter().chain(tail).enumerate() {
            if i > 0 {
                at += sep;
            }
            let end = at + hint.width(form);
            if hint.control == Some(control) {
                return Some((at, end));
            }
            at = end;
        }
        None
    }

    /// The header as one drawn row on `ui.header`: the words, then the
    /// hints joined by the separator, padded to `width` so the surface
    /// reaches the right edge.
    pub(super) fn line(&self, theme: &Theme, width: usize) -> Line<'static> {
        self.line_with_hovers(theme, width, Hover::None, false, theme.header)
    }

    /// Draw the header with hover behind its clickable left label.
    pub(super) fn line_with_left_hover(
        &self,
        theme: &Theme,
        width: usize,
        left_hovered: bool,
        focused: bool,
    ) -> Line<'static> {
        self.line_with_hovers(
            theme,
            width,
            if left_hovered {
                Hover::Title
            } else {
                Hover::None
            },
            focused,
            theme.header,
        )
    }

    /// Draw the clickable title and mode control with shared hover treatment.
    pub(super) fn line_with_header_hovers(
        &self,
        theme: &Theme,
        width: usize,
        left_hovered: bool,
        control_hovered: Option<Control>,
        focused: bool,
    ) -> Line<'static> {
        self.line_with_hovers(
            theme,
            width,
            control_hovered.map_or(
                if left_hovered {
                    Hover::Title
                } else {
                    Hover::None
                },
                Hover::Control,
            ),
            focused,
            theme.header,
        )
    }

    /// Draw a key bar with hover behind the action under the pointer.
    pub(super) fn line_with_action_hover(
        &self,
        theme: &Theme,
        width: usize,
        hovered: Option<Action>,
    ) -> Line<'static> {
        self.line_with_hovers(
            theme,
            width,
            hovered.map_or(Hover::None, Hover::Action),
            false,
            theme.header,
        )
    }

    fn line_with_hovers(
        &self,
        theme: &Theme,
        width: usize,
        hovered: Hover,
        focused: bool,
        base: ratatui::style::Style,
    ) -> Line<'static> {
        let shown = self.shown(width);
        let left_count = shown
            .as_ref()
            .map_or(self.required_left, |(left_count, _, _, _, _)| *left_count);
        let mut spans =
            self.left_spans(theme, left_count, matches!(hovered, Hover::Title), focused);
        let mut at = self.left_width_for(left_count);
        if at > width {
            at = clip_spans(&mut spans, width);
        }
        if let Some((_, start, hints, tail, form)) = shown {
            spans.push(Span::raw(" ".repeat(start - at)));
            at = start;
            let faint = theme.info.add_modifier(Modifier::DIM);
            let sep = display_width(self.sep);
            for (i, hint) in hints.iter().chain(tail).enumerate() {
                if i > 0 {
                    spans.push(Span::styled(self.sep, faint));
                    at += sep;
                }
                let hint_hovered = match hovered {
                    Hover::Action(action) => hint.actions.contains(&action),
                    Hover::Control(control) => hint.control == Some(control),
                    Hover::None | Hover::Title => false,
                };
                let surface = if hint_hovered {
                    base.patch(theme.list_hover)
                } else {
                    base
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
        Line::from(spans).style(base)
    }

    fn left_spans(
        &self,
        theme: &Theme,
        count: usize,
        hovered: bool,
        focused: bool,
    ) -> Vec<Span<'static>> {
        let mut spans = Vec::with_capacity(count + 1);
        for (index, (text, tone)) in self.left.iter().take(count).enumerate() {
            if *tone == Tone::Pane {
                let surface = if hovered && index == 0 {
                    theme.header.patch(theme.list_hover)
                } else {
                    theme.header
                };
                let title = text.strip_prefix(' ').unwrap_or(text);
                let marker = if focused {
                    theme.pane_focus
                } else {
                    theme.popup_key
                };
                spans.push(Span::styled(
                    if focused { "▎" } else { " " },
                    on_surface(surface, marker),
                ));
                spans.push(Span::styled(
                    title.to_owned(),
                    on_surface(surface, theme.popup_key),
                ));
                continue;
            }
            let style = if hovered && index == 0 {
                tone_style(theme, *tone).patch(theme.list_hover)
            } else {
                tone_style(theme, *tone)
            };
            spans.push(Span::styled(text.clone(), style));
        }
        spans
    }
}

fn clip_spans(spans: &mut Vec<Span<'_>>, width: usize) -> usize {
    let mut remaining = width;
    let mut clipped = false;
    spans.retain_mut(|span| {
        if clipped {
            return false;
        }
        let (prefix, used) = super::fitting_prefix(&span.content, remaining);
        clipped = prefix.len() < span.content.len();
        span.content = prefix.to_owned().into();
        remaining -= used;
        !span.content.is_empty()
    });
    width - remaining
}

/// The shared action-first controls used by destructive confirmations.
#[derive(Debug, Clone)]
pub(crate) struct ConfirmationControls {
    bar: Header,
}

impl ConfirmationControls {
    /// Build affirmative-then-cancel controls for `verb`.
    pub(crate) fn new(verb: &'static str) -> Self {
        Self::with_key("Enter", verb)
    }

    /// Build controls with an explicit affirmative key.
    pub(crate) fn with_key(key: &'static str, verb: &'static str) -> Self {
        let mut bar = Header::bar(vec![
            HintOf::control(key, verb, Action::Confirm),
            HintOf::control("Esc", "cancel", Action::Escape),
        ]);
        bar.left_pad = 0;
        Self { bar }
    }

    /// Return the control under `column`, if that control is visible.
    pub(crate) fn action_at(&self, width: usize, column: usize) -> Option<Action> {
        self.bar.action_at(width, column)
    }

    /// Draw controls on the popup surface with whole-control hover.
    pub(crate) fn line(
        &self,
        theme: &Theme,
        width: usize,
        hovered: Option<Action>,
    ) -> Line<'static> {
        self.bar.line_with_hovers(
            theme,
            width,
            hovered.map_or(Hover::None, Hover::Action),
            false,
            theme.popup,
        )
    }
}

fn tone_style(theme: &Theme, tone: Tone) -> ratatui::style::Style {
    match tone {
        Tone::Key | Tone::Pane => theme.popup_key,
        Tone::Info => theme.info,
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
/// The normal board draws a clickable `Threads` title, then passive scope
/// and lifecycle counts against the right edge. History views keep Threads
/// as the pane name and add their specific title as neutral context.
pub(crate) fn review_header(app: &App) -> Header {
    let review = app.review();
    let mut counts = passive_count_hints(
        app.review_counts(review.file_only),
        review.view != ReviewView::Board || review.resolved,
    );
    if review.view == ReviewView::Board {
        let (scope, compact) = if review.file_only {
            ("file", "f")
        } else {
            ("workspace", "w")
        };
        counts.insert(0, HintOf::responsive_word(scope, compact, Tone::Info));
        if review.all_threads {
            counts.insert(1, HintOf::word("all history".to_owned(), Tone::Info));
        }
        Header::counted_with_tail(
            vec![(" Threads ".to_owned(), Tone::Pane)],
            counts,
            vec![HintOf::diff_mode(app.diff_mode())],
        )
    } else {
        Header::counted_with_tail(
            vec![
                (" Threads ".to_owned(), Tone::Pane),
                (format!(" {}", review.view.title()), Tone::Info),
            ],
            counts,
            vec![HintOf::diff_mode(app.diff_mode())],
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
    let mut header = Header::counted_with_tail(
        vec![
            (" File ".to_owned(), Tone::Pane),
            (format!(" {filename}"), Tone::Info),
        ],
        counts,
        vec![HintOf::diff_mode(app.diff_mode())],
    );
    header.required_left = 1;
    header
}

/// The review list's key bar on its bottom row while it owns navigation.
pub(crate) fn review_footer(app: &App, entries: &[Entry]) -> Header {
    let place = Where::Review;
    let hints = if app.pane_has_navigation(Focus::Review) {
        let view = app.review().view;
        let mut hints = Vec::new();
        if view != ReviewView::Archived {
            hints.push(HintOf::keyed(place, Action::Reply, "reply"));
            if app.thread_message_editable() {
                hints.push(HintOf::keyed(place, Action::EditMessage, "edit"));
            }
        }
        let cursor = app.review_thread_cursor();
        let cursor_thread = cursor.thread().and_then(|id| app.thread(id));
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
        if !entries.is_empty() {
            hints.push(HintOf::paired(
                place,
                Action::MoveUp,
                Action::MoveDown,
                "move",
            ));
        }
        hints.push(HintOf::keyed(place, Action::Escape, ""));
        hints
    } else {
        Vec::new()
    };
    Header::bar(hints)
}

/// The thread list's header (ADR 0066, ADR 0075): `Thread list` at the left,
/// then passive scope and lifecycle counts against the right edge. Scope
/// shortens to `f` or `w` after count words drop and before it disappears.
pub(crate) fn threads_pane_header(app: &App) -> Header {
    let scope = app.sidebar_scope();
    let file_only = scope == crate::app::threads::pane::PaneScope::File;
    let mut filters = vec![HintOf::responsive_word(
        scope.word(),
        scope.short_word(),
        Tone::Info,
    )];
    if app.all_threads() {
        filters.push(HintOf::word("all history".to_owned(), Tone::Info));
    }
    Header::counted_with_tail(
        vec![(" Thread list ".to_owned(), Tone::Pane)],
        filters,
        passive_count_hints(
            app.review_counts(file_only),
            app.review().view != ReviewView::Board || app.review().resolved,
        ),
    )
}

/// The files pane's header (ADR 0017, ADR 0068): `File list` at the left,
/// then the compact active-filter marker before the `+n -m` totals.
pub(crate) fn files_pane_header(app: &App) -> Header {
    let marker = app.files_shown_marker();
    let filters = if marker.is_empty() {
        Vec::new()
    } else {
        vec![HintOf::word(marker, Tone::Info)]
    };
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
    Header::counted_with_tail(
        vec![(" File list ".to_owned(), Tone::Pane)],
        filters,
        counts,
    )
}

/// The Thread-list footer: local actions while the pane owns navigation.
pub(crate) fn threads_pane_footer(app: &App) -> Header {
    if !app.pane_has_navigation(Focus::ThreadsPane) {
        return Header::bar(Vec::new());
    }
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
    use crate::app::testing;

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
            HintOf::new("k/j", "move", &[Action::MoveUp, Action::MoveDown]),
        ])
    }

    #[test]
    fn a_bar_reads_from_the_left_and_fills_the_row() -> anyhow::Result<()> {
        let line = bar().line(&theme()?, 40);
        let text = text(&line);
        assert_eq!(display_width(&text), 40);
        assert_eq!(text.trim_end(), " file s · resolved x · move k/j");
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
        let movement = display_width(" file s · resolved x · ");
        assert_eq!(bar.action_at(40, movement), Some(Action::MoveUp));
        assert_eq!(
            bar.action_at(40, movement + display_width("move k/")),
            Some(Action::MoveDown)
        );
        assert_eq!(bar.action_at(40, 0), None, "the margin runs nothing");
        assert_eq!(
            bar.action_at(24, movement),
            None,
            "a dropped hint is not there"
        );
    }

    #[test]
    fn file_and_every_review_header_retain_the_mode_control_last() -> anyhow::Result<()> {
        let dir = testing::workspace("header-diff-control", testing::README)?;
        let mut app = testing::app(&dir)?;
        let theme = theme()?;

        let mut file = file_header(&app);
        file.left[1].0 = format!("  {}", "x".repeat(25));
        let narrow = text(&file.line(&theme, 40));
        assert!(narrow.contains("File"), "{narrow}");
        assert!(narrow.contains("Diff: normal"), "{narrow}");
        assert!(!narrow.contains("xxxxxxxx"), "{narrow}");
        let minimum = file.title_width() + display_width("Diff: normal") + HINT_MARGIN;
        assert!(file.control_bounds(minimum, Control::DiffMode).is_some());
        assert!(
            file.control_bounds(minimum.saturating_sub(1), Control::DiffMode)
                .is_none()
        );

        let assert_control = |header: Header, title: &str| {
            let wide = text(&header.line(&theme, 100));
            assert!(wide.contains(title), "{wide}");
            assert!(wide.contains("Diff: normal"), "{wide}");
            let minimum = header.left_width() + display_width("Diff: normal") + HINT_MARGIN;
            assert!(
                header.control_bounds(minimum, Control::DiffMode).is_some(),
                "{title} should retain Diff after optional state is dropped"
            );
            assert!(
                header
                    .control_bounds(minimum.saturating_sub(1), Control::DiffMode)
                    .is_none(),
                "{title} should drop Diff only when it cannot coexist with the title"
            );
        };

        for (view, title) in [
            (ReviewView::Board, "Threads"),
            (ReviewView::RecentlyResolved, "Recently resolved"),
            (ReviewView::Archived, "Archived"),
        ] {
            app.open_review_view(view);
            assert_control(review_header(&app), title);
        }
        Ok(())
    }

    #[test]
    fn normal_thread_headers_show_all_history_only_while_enabled() -> anyhow::Result<()> {
        let dir = testing::workspace("header-all-history", testing::README)?;
        let mut app = testing::app(&dir)?;
        let theme = theme()?;
        app.open_review();

        assert!(!text(&review_header(&app).line(&theme, 100)).contains("all history"));
        assert!(!text(&threads_pane_header(&app).line(&theme, 100)).contains("all history"));

        app.toggle_all_threads();
        assert!(text(&review_header(&app).line(&theme, 100)).contains("all history"));
        assert!(text(&threads_pane_header(&app).line(&theme, 100)).contains("all history"));

        app.open_review_view(ReviewView::RecentlyResolved);
        assert!(!text(&review_header(&app).line(&theme, 100)).contains("all history"));
        assert!(text(&threads_pane_header(&app).line(&theme, 100)).contains("all history"));
        Ok(())
    }

    #[test]
    fn pane_titles_fit_even_when_the_title_is_wider_than_the_pane() -> anyhow::Result<()> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        for title in [" File list ", " Thread list ", " 界界 "] {
            let header = Header::new(vec![(title.to_owned(), Tone::Pane)], Vec::new());
            for width in 0..=16 {
                for focused in [false, true] {
                    let line = header.line_with_header_hovers(&theme, width, false, None, focused);
                    let rendered = text(&line);
                    assert_eq!(display_width(&rendered), width, "{title} at {width}");
                    if width > 0 {
                        assert!(rendered.starts_with(if focused { '▎' } else { ' ' }));
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn pane_titles_use_the_button_accent_without_moving_focus_content() -> anyhow::Result<()> {
        for name in fathomable_core::theme::BUILTIN_NAMES {
            let core = fathomable_core::theme::Theme::resolve(name, |_| Ok(None))?;
            let theme = Theme::from_core(&core);
            let header = Header::counted_with_tail(
                vec![
                    (" File ".to_owned(), Tone::Pane),
                    (" README.md".to_owned(), Tone::Info),
                ],
                vec![HintOf::word("+3".to_owned(), Tone::Added)],
                vec![HintOf::diff_mode(DiffMode::Normal)],
            );
            let inactive = header.line_with_header_hovers(&theme, 48, false, None, false);
            let focused = header.line_with_header_hovers(&theme, 48, false, None, true);

            let inactive_text = text(&inactive);
            let focused_text = text(&focused);
            assert_eq!(display_width(&inactive_text), display_width(&focused_text));
            assert_eq!(
                inactive_text.strip_prefix(' '),
                focused_text.strip_prefix('▎')
            );
            assert_eq!(inactive.spans[0].content, " ");
            assert_eq!(focused.spans[0].content, "▎");
            assert_eq!(focused.spans[0].style.fg, theme.pane_focus.fg);
            assert_eq!(focused.spans[1].content, "File ");
            assert_eq!(inactive.spans[1].style.fg, theme.popup_key.fg);
            assert_eq!(focused.spans[1].style.fg, theme.popup_key.fg);
            assert_ne!(focused.spans[2].style.fg, theme.popup_key.fg);
        }
        Ok(())
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
