// @okf-doc: /decisions/0010-viewer-ux.md
//! Viewer state: cursor, scrolling, selection, search, and reload anchoring.
//!
//! Everything here is plain data so the ADR 0010 conventions can be tested
//! without a terminal; `ui` draws it and `keys` drives it.

use std::fmt;

use fathomable_core::layout::{Layout, LineIndex, display_width};
use regex::Regex;

/// Rendered lines kept visible above and below the cursor.
const SCROLLOFF: usize = 3;

/// Editing-style mode shown in the status pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Select,
    Command,
    Search { backward: bool },
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "NOR",
            Self::Select => "SEL",
            Self::Command => "CMD",
            Self::Search { .. } => "SRCH",
        })
    }
}

/// A position in rendered coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// A selection between an anchor and a moving head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cursor,
    pub head: Cursor,
    /// Whole lines (`V`) rather than columns (mouse drag).
    pub linewise: bool,
}

impl Selection {
    /// The selection ordered top-left to bottom-right.
    pub fn ordered(&self) -> (Cursor, Cursor) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether rendered cell `(row, col)` is inside the selection.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let (start, end) = self.ordered();
        if row < start.row || row > end.row {
            return false;
        }
        if self.linewise {
            return true;
        }
        let after_start = row > start.row || col >= start.col;
        let before_end = row < end.row || col <= end.col;
        after_start && before_end
    }
}

/// A search match in rendered coordinates: row and column span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub row: usize,
    pub start: usize,
    pub end: usize,
}

/// What the event loop must do after handling input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
    Copy(String),
}

#[derive(Debug)]
pub struct View {
    text: String,
    layout: Layout,
    width: usize,
    height: usize,
    show_source: bool,
    cursor: Cursor,
    want_col: usize,
    scroll: usize,
    mode: Mode,
    input: String,
    selection: Option<Selection>,
    pattern: Option<Regex>,
    backward: bool,
    matches: Vec<Match>,
    message: Option<String>,
    changed: bool,
    pending: Option<char>,
}

impl View {
    /// Lay `text` out for a text area of `width` by `height` cells.
    pub fn new(text: String, width: usize, height: usize) -> Self {
        let layout = Layout::render(&text, width);
        Self {
            text,
            layout,
            width,
            height: height.max(1),
            show_source: false,
            cursor: Cursor::default(),
            want_col: 0,
            scroll: 0,
            mode: Mode::Normal,
            input: String::new(),
            selection: None,
            pattern: None,
            backward: false,
            matches: Vec::new(),
            message: None,
            changed: false,
            pending: None,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn index(&self) -> &LineIndex {
        self.layout.index()
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn pending(&self) -> Option<char> {
        self.pending
    }

    pub fn changed(&self) -> bool {
        self.changed
    }

    pub fn source_view(&self) -> bool {
        self.show_source
    }

    /// Progress through the document as a percentage of rendered lines.
    pub fn percent(&self) -> usize {
        let last = self.layout.lines().len().saturating_sub(1);
        (self.cursor.row * 100).checked_div(last).unwrap_or(100)
    }

    /// Cursor position in source coordinates: 1-based line and 0-based column.
    pub fn source_position(&self) -> (usize, usize) {
        let offset = self.cursor_offset().unwrap_or(0);
        let index = self.layout.index();
        (index.line_of(offset), index.column_of(&self.text, offset))
    }

    /// The source byte offset under the cursor.
    pub fn cursor_offset(&self) -> Option<usize> {
        self.layout
            .lines()
            .get(self.cursor.row)
            .and_then(|line| line.source_at(self.cursor.col))
    }

    /// Re-lay out for a new pane size, keeping the cursor on the same source.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height.max(1);
        self.relayout();
    }

    /// Replace the document text after a change on disk (ADR 0010 reload).
    pub fn reload(&mut self, text: String) {
        self.text = text;
        self.changed = true;
        self.relayout();
    }

    /// Switch between rendered Markdown and the raw source.
    pub fn toggle_source_view(&mut self) {
        self.show_source = !self.show_source;
        self.relayout();
    }

    fn relayout(&mut self) {
        // Anchor by source line and column, not byte offset, so an insertion
        // above the cursor does not drag it onto unrelated text (ADR 0010).
        let (line, column) = self.source_position();
        let screen_row = self.cursor.row.saturating_sub(self.scroll);
        self.layout = if self.show_source {
            Layout::source(&self.text, self.width)
        } else {
            Layout::render(&self.text, self.width)
        };
        let index = self.layout.index();
        let line = line.min(index.line_count());
        let offset = index.offset_at(&self.text, line, column);
        let row = offset
            .and_then(|offset| self.layout.line_at_offset(offset))
            .unwrap_or(self.cursor.row);
        self.cursor.row = row.min(self.last_row());
        self.scroll = self.cursor.row.saturating_sub(screen_row);
        self.clamp_col();
        self.selection = None;
        self.rescan();
        self.ensure_visible();
    }

    fn last_row(&self) -> usize {
        self.layout.lines().len().saturating_sub(1)
    }

    fn columns(&self, row: usize) -> Vec<usize> {
        self.layout
            .lines()
            .get(row)
            .map(fathomable_core::layout::Line::columns)
            .unwrap_or_default()
    }

    /// Snap the cursor to a grapheme start no further right than `want_col`.
    fn clamp_col(&mut self) {
        let columns = self.columns(self.cursor.row);
        self.cursor.col = columns
            .iter()
            .rev()
            .find(|&&col| col <= self.want_col)
            .copied()
            .unwrap_or(0);
    }

    fn ensure_visible(&mut self) {
        let height = self.height;
        let off = SCROLLOFF.min(height.saturating_sub(1) / 2);
        if self.cursor.row < self.scroll + off {
            self.scroll = self.cursor.row.saturating_sub(off);
        }
        let bottom = self.scroll + height;
        if self.cursor.row + off >= bottom {
            self.scroll = (self.cursor.row + off + 1).saturating_sub(height);
        }
        let max_scroll = self.layout.lines().len().saturating_sub(height);
        self.scroll = self.scroll.min(max_scroll);
    }

    fn set_row(&mut self, row: usize) {
        self.cursor.row = row.min(self.last_row());
        self.clamp_col();
        self.extend_selection();
        self.ensure_visible();
    }

    fn extend_selection(&mut self) {
        if let Some(selection) = self.selection.as_mut()
            && self.mode == Mode::Select
        {
            selection.head = self.cursor;
        }
    }

    pub fn move_down(&mut self, n: usize) {
        self.set_row(self.cursor.row.saturating_add(n));
    }

    pub fn move_up(&mut self, n: usize) {
        self.set_row(self.cursor.row.saturating_sub(n));
    }

    pub fn half_page_down(&mut self) {
        self.move_down((self.height / 2).max(1));
    }

    pub fn half_page_up(&mut self) {
        self.move_up((self.height / 2).max(1));
    }

    pub fn goto_top(&mut self) {
        self.set_row(0);
    }

    pub fn goto_bottom(&mut self) {
        self.set_row(self.last_row());
    }

    pub fn move_left(&mut self) {
        let columns = self.columns(self.cursor.row);
        if let Some(&col) = columns.iter().rev().find(|&&c| c < self.cursor.col) {
            self.cursor.col = col;
        }
        self.want_col = self.cursor.col;
        self.extend_selection();
    }

    pub fn move_right(&mut self) {
        let columns = self.columns(self.cursor.row);
        if let Some(&col) = columns.iter().find(|&&c| c > self.cursor.col) {
            self.cursor.col = col;
        }
        self.want_col = self.cursor.col;
        self.extend_selection();
    }

    pub fn line_start(&mut self) {
        self.cursor.col = 0;
        self.want_col = 0;
        self.extend_selection();
    }

    pub fn line_end(&mut self) {
        self.cursor.col = self.columns(self.cursor.row).last().copied().unwrap_or(0);
        self.want_col = usize::MAX;
        self.extend_selection();
    }

    /// Scroll the viewport without a cursor jump unless the cursor leaves it.
    pub fn scroll_by(&mut self, delta: isize) {
        let max_scroll = self.layout.lines().len().saturating_sub(self.height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max_scroll);
        let top = self.scroll;
        let bottom = self.scroll + self.height - 1;
        if self.cursor.row < top {
            self.cursor.row = top;
            self.clamp_col();
        } else if self.cursor.row > bottom {
            self.cursor.row = bottom.min(self.last_row());
            self.clamp_col();
        }
    }

    /// Place the cursor at a screen position (mouse click).
    pub fn click(&mut self, screen_row: usize, col: usize) {
        self.selection = None;
        self.mode = Mode::Normal;
        self.cursor.row = (self.scroll + screen_row).min(self.last_row());
        self.want_col = col;
        self.clamp_col();
    }

    /// Extend a mouse selection to a screen position (drag).
    pub fn drag(&mut self, screen_row: usize, col: usize) {
        let anchor = self.cursor;
        let row = (self.scroll + screen_row).min(self.last_row());
        let col = self
            .columns(row)
            .iter()
            .rev()
            .find(|&&c| c <= col)
            .copied()
            .unwrap_or(0);
        let selection = self.selection.get_or_insert(Selection {
            anchor,
            head: anchor,
            linewise: false,
        });
        selection.head = Cursor { row, col };
        self.mode = Mode::Select;
    }

    /// Finish a mouse selection: copy it (ADR 0010) and return to normal mode.
    pub fn release(&mut self) -> Effect {
        let effect = self.copy_selection();
        self.mode = Mode::Normal;
        effect
    }

    /// Start a keyboard line selection (`V`).
    pub fn select_lines(&mut self) {
        self.selection = Some(Selection {
            anchor: self.cursor,
            head: self.cursor,
            linewise: true,
        });
        self.mode = Mode::Select;
    }

    /// Copy the selection (`y`) and leave select mode.
    pub fn yank(&mut self) -> Effect {
        let effect = self.copy_selection();
        self.mode = Mode::Normal;
        effect
    }

    fn copy_selection(&mut self) -> Effect {
        let Some(text) = self.selected_source() else {
            return Effect::None;
        };
        let lines = text.lines().count().max(1);
        self.message = Some(format!(
            "copied {lines} line{}",
            if lines == 1 { "" } else { "s" }
        ));
        Effect::Copy(text)
    }

    /// The source Markdown behind the selection, if any.
    pub fn selected_source(&self) -> Option<String> {
        let selection = self.selection?;
        let (start, end) = selection.ordered();
        let lines = self.layout.lines();
        let (from, to) = if selection.linewise {
            // Whole source lines, including list markers and other syntax
            // the rendered spans do not cover.
            let index = self.layout.index();
            let from = lines[start.row..=end.row]
                .iter()
                .find_map(fathomable_core::layout::Line::source)
                .and_then(|range| index.range_of(index.line_of(range.start)))
                .map(|range| range.start)?;
            let to = lines[start.row..=end.row]
                .iter()
                .rev()
                .find_map(fathomable_core::layout::Line::source)
                .and_then(|range| index.range_of(index.line_of(range.end.max(range.start + 1) - 1)))
                .map(|range| range.end)?;
            (from, to)
        } else {
            let from = lines.get(start.row)?.source_at(start.col)?;
            let last = lines.get(end.row)?.source_at(end.col)?;
            let to = last
                + self
                    .text
                    .get(last..)
                    .and_then(|rest| rest.chars().next())
                    .map_or(0, char::len_utf8);
            (from, to)
        };
        if from >= to {
            return None;
        }
        let from = floor_char(&self.text, from);
        let to = ceil_char(&self.text, to);
        self.text.get(from..to).map(str::to_owned)
    }

    /// Esc: clear input, pending keys, selection, then search highlights.
    pub fn escape(&mut self) {
        self.message = None;
        if matches!(self.mode, Mode::Command | Mode::Search { .. }) {
            self.mode = Mode::Normal;
            self.input.clear();
        } else if self.pending.is_some() {
            self.pending = None;
        } else if self.selection.is_some() {
            self.selection = None;
            self.mode = Mode::Normal;
        } else {
            self.clear_highlight();
        }
    }

    pub fn clear_highlight(&mut self) {
        self.pattern = None;
        self.matches.clear();
    }

    pub fn set_pending(&mut self, key: Option<char>) {
        self.pending = key;
    }

    pub fn clear_message(&mut self) {
        self.message = None;
    }

    pub fn start_command(&mut self) {
        self.mode = Mode::Command;
        self.input.clear();
        self.message = None;
    }

    pub fn start_search(&mut self, backward: bool) {
        self.mode = Mode::Search { backward };
        self.backward = backward;
        self.input.clear();
        self.message = None;
    }

    /// Type into the `:` or `/` line; searches update incrementally.
    pub fn input_char(&mut self, ch: char) {
        self.input.push(ch);
        self.incremental();
    }

    pub fn input_backspace(&mut self) {
        self.input.pop();
        if self.input.is_empty() && matches!(self.mode, Mode::Command | Mode::Search { .. }) {
            self.mode = Mode::Normal;
            return;
        }
        self.incremental();
    }

    fn incremental(&mut self) {
        if let Mode::Search { .. } = self.mode {
            match compile(&self.input) {
                Ok(pattern) => {
                    self.pattern = Some(pattern);
                    self.rescan();
                    self.message = None;
                    self.jump_from(self.cursor, self.backward, false);
                }
                Err(error) => {
                    self.matches.clear();
                    self.message = Some(error);
                }
            }
        }
    }

    /// Enter on the input line.
    pub fn confirm(&mut self) -> Effect {
        match self.mode {
            Mode::Command => {
                let command = std::mem::take(&mut self.input);
                self.mode = Mode::Normal;
                self.execute(command.trim())
            }
            Mode::Search { .. } => {
                self.mode = Mode::Normal;
                if self.input.is_empty() {
                    self.search_next(false);
                } else {
                    self.incremental();
                    self.input.clear();
                }
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn execute(&mut self, command: &str) -> Effect {
        match command {
            "q" | "q!" | "quit" => Effect::Quit,
            "noh" | "nohlsearch" => {
                self.clear_highlight();
                Effect::None
            }
            "source" => {
                self.toggle_source_view();
                Effect::None
            }
            "" => Effect::None,
            number if number.chars().all(|c| c.is_ascii_digit()) => {
                if let Ok(line) = number.parse::<usize>() {
                    self.goto_source_line(line);
                }
                Effect::None
            }
            other => {
                self.message = Some(format!("not a command: {other}"));
                Effect::None
            }
        }
    }

    /// Move to the rendered line showing source line `line`.
    pub fn goto_source_line(&mut self, line: usize) {
        if let Some(range) = self.layout.index().range_of(line)
            && let Some(row) = self.layout.line_at_offset(range.start)
        {
            self.set_row(row);
        }
    }

    /// `n` / `N`: next match in the search direction, flipped by `reverse`.
    pub fn search_next(&mut self, reverse: bool) {
        if self.pattern.is_none() {
            self.message = Some("no previous search".to_owned());
            return;
        }
        let backward = self.backward ^ reverse;
        self.jump_from(self.cursor, backward, true);
    }

    fn rescan(&mut self) {
        self.matches.clear();
        let Some(pattern) = &self.pattern else {
            return;
        };
        for (row, line) in self.layout.lines().iter().enumerate() {
            let text = line.text();
            for found in pattern.find_iter(&text) {
                let start = display_width(&text[..found.start()]);
                let end = start + display_width(found.as_str()).max(1);
                self.matches.push(Match { row, start, end });
            }
        }
    }

    /// Jump to the match after (or before) `from`; `skip_current` excludes a
    /// match sitting exactly on the cursor, as `n` must.
    fn jump_from(&mut self, from: Cursor, backward: bool, skip_current: bool) {
        if self.matches.is_empty() {
            if self.pattern.is_some() {
                self.message = Some(format!("pattern not found: {}", self.input));
            }
            return;
        }
        let at = |m: &Match| Cursor {
            row: m.row,
            col: m.start,
        };
        let target = if backward {
            let before = self.matches.iter().rev().find(|m| at(m) < from);
            before.or_else(|| {
                self.message = Some("search hit TOP, continuing at BOTTOM".to_owned());
                self.matches.last()
            })
        } else {
            let after = self.matches.iter().find(|m| {
                if skip_current {
                    at(m) > from
                } else {
                    at(m) >= from
                }
            });
            after.or_else(|| {
                self.message = Some("search hit BOTTOM, continuing at TOP".to_owned());
                self.matches.first()
            })
        };
        if let Some(target) = target.copied() {
            self.cursor = at(&target);
            self.want_col = self.cursor.col;
            self.ensure_visible();
        }
    }
}

/// Compile a search pattern with smart case (ADR 0010).
fn compile(input: &str) -> Result<Regex, String> {
    let smart = if input.chars().any(char::is_uppercase) {
        String::new()
    } else {
        "(?i)".to_owned()
    };
    Regex::new(&format!("{smart}{input}")).map_err(|error| match error {
        regex::Error::Syntax(msg) => msg.lines().last().unwrap_or("bad pattern").to_owned(),
        other => other.to_string(),
    })
}

fn floor_char(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn ceil_char(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::{Cursor, Effect, Mode, View};

    const DOC: &str = "# Title\n\nalpha beta\n\n- one\n- two\n- three\n\nlast *word* here\n";

    fn view() -> View {
        View::new(DOC.to_owned(), 40, 5)
    }

    #[test]
    fn cursor_moves_by_rendered_line_and_keeps_scrolloff() {
        let mut v = view();
        assert_eq!(v.layout().lines().len(), 9);
        v.move_down(3);
        assert_eq!(v.cursor().row, 3);
        // With height 5 and scrolloff 2 (half of height-1), row 3 scrolls.
        assert_eq!(v.scroll(), 1);
        v.goto_bottom();
        assert_eq!(v.cursor().row, 8);
        assert_eq!(v.scroll(), 4);
        v.goto_top();
        assert_eq!(v.scroll(), 0);
        v.half_page_down();
        assert_eq!(v.cursor().row, 2);
    }

    #[test]
    fn horizontal_motion_steps_graphemes_and_remembers_column() {
        let mut v = View::new("日本語\n\nab\n\n日本語\n".to_owned(), 40, 10);
        v.move_right();
        v.move_right();
        assert_eq!(v.cursor().col, 4);
        v.move_down(2);
        assert_eq!(v.cursor().col, 1, "clamped to a grapheme start on 'ab'");
        v.move_down(2);
        assert_eq!(v.cursor().col, 4, "wanted column restored");
        v.line_end();
        v.move_left();
        assert_eq!(v.cursor().col, 2);
    }

    #[test]
    fn source_position_is_in_source_coordinates() {
        let mut v = view();
        v.move_down(2);
        v.move_right();
        assert_eq!(v.source_position(), (3, 1));
        v.goto_bottom();
        assert_eq!(v.source_position().0, 9);
    }

    #[test]
    fn reload_reanchors_cursor_to_same_source_line() {
        let mut v = view();
        v.move_down(4);
        assert_eq!(v.source_position().0, 5);
        let screen_row = v.cursor().row - v.scroll();
        assert_eq!(screen_row, 2, "cursor sits mid-screen before the reload");
        let text = "# Title\n\nnew paragraph inserted\n\nalpha beta\n\n- one\n- two\n- three\n\nlast *word* here\n";
        v.reload(text.to_owned());
        assert!(v.changed());
        // ADR 0010: anchor by source line number, so line 5 is now "alpha beta".
        assert_eq!(v.source_position().0, 5);
        assert_eq!(v.layout().lines()[v.cursor().row].text(), "alpha beta");
        assert_eq!(v.cursor().row - v.scroll(), screen_row);
        // Deleted line: nearest surviving line wins.
        v.reload("# Title\n\nalpha\n".to_owned());
        assert_eq!(v.cursor().row, 2);
    }

    #[test]
    fn line_selection_copies_source_markdown() {
        let mut v = view();
        v.move_down(4);
        v.select_lines();
        assert_eq!(v.mode(), Mode::Select);
        v.move_down(1);
        assert_eq!(v.yank(), Effect::Copy("- one\n- two".to_owned()));
        assert_eq!(v.mode(), Mode::Normal);
        assert_eq!(v.message(), Some("copied 2 lines"));
    }

    #[test]
    fn mouse_drag_copies_source_slice_on_release() {
        let mut v = view();
        // Click on "beta" (row 2, col 6) then drag to the end of "one".
        v.click(2, 6);
        v.drag(4, 4);
        assert_eq!(v.mode(), Mode::Select);
        assert_eq!(v.release(), Effect::Copy("beta\n\n- one".to_owned()));
        // Dragging inside emphasis maps back to the raw markup.
        v.click(8, 5);
        v.drag(8, 8);
        assert_eq!(v.release(), Effect::Copy("word".to_owned()));
    }

    #[test]
    fn search_is_incremental_smart_case_and_wraps() {
        let mut v = view();
        v.start_search(false);
        for ch in "T".chars() {
            v.input_char(ch);
        }
        assert_eq!(v.matches().len(), 1, "uppercase pattern is case-sensitive");
        v.escape();
        v.start_search(false);
        for ch in "t".chars() {
            v.input_char(ch);
        }
        assert!(v.matches().len() > 3, "lowercase pattern ignores case");
        assert_eq!(v.confirm(), Effect::None);
        assert_eq!(v.mode(), Mode::Normal);
        v.goto_bottom();
        v.line_end();
        v.search_next(false);
        assert_eq!(v.cursor(), Cursor { row: 0, col: 0 });
        assert_eq!(v.message(), Some("search hit BOTTOM, continuing at TOP"));
        v.search_next(true);
        assert_eq!(v.message(), Some("search hit TOP, continuing at BOTTOM"));
    }

    #[test]
    fn bad_regex_reports_and_stays_put() {
        let mut v = view();
        v.move_down(2);
        v.start_search(false);
        v.input_char('(');
        assert!(v.message().is_some());
        assert_eq!(v.cursor().row, 2);
        assert!(v.matches().is_empty());
    }

    #[test]
    fn commands_quit_goto_and_toggle_source() {
        let mut v = view();
        v.start_command();
        for ch in "9".chars() {
            v.input_char(ch);
        }
        assert_eq!(v.confirm(), Effect::None);
        assert_eq!(v.source_position().0, 9);
        v.start_command();
        v.input_char('q');
        assert_eq!(v.confirm(), Effect::Quit);
        v.start_command();
        for ch in "source".chars() {
            v.input_char(ch);
        }
        v.confirm();
        assert!(v.source_view());
        assert_eq!(v.layout().lines()[0].text(), "# Title");
        assert_eq!(v.source_position().0, 9, "toggle keeps the source line");
    }

    #[test]
    fn escape_clears_in_order_and_never_quits() {
        let mut v = view();
        v.start_search(false);
        v.input_char('a');
        v.confirm();
        assert!(!v.matches().is_empty());
        v.select_lines();
        v.set_pending(Some('g'));
        v.escape();
        assert_eq!(v.pending(), None);
        assert!(v.selection().is_some());
        v.escape();
        assert!(v.selection().is_none());
        assert!(!v.matches().is_empty());
        v.escape();
        assert!(v.matches().is_empty());
        v.escape();
        assert_eq!(v.mode(), Mode::Normal);
    }

    #[test]
    fn wheel_scroll_drags_cursor_along() {
        let mut v = view();
        v.scroll_by(3);
        assert_eq!(v.scroll(), 3);
        assert_eq!(v.cursor().row, 3);
        v.scroll_by(-10);
        assert_eq!(v.scroll(), 0);
    }
}
