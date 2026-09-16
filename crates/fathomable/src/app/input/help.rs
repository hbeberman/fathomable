// @okf-doc: /decisions/0078-all-keys-stays-reachable.md
//! The compact, searchable, scrollable `Space ?` keymap.
//!
//! The binding table remains the source of every action and spelling.
//! This module only groups, filters, wraps, and places those rows for the
//! current terminal, and owns the transient query and scroll position.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::layout::display_width;

use super::bindings::{self, BINDINGS, Binding};
use crate::app::view::Effect;
use crate::app::{App, Popup};

const TWO_COLUMN_MIN_WIDTH: usize = 72;
const MAX_KEY_WIDTH: usize = 14;
const COLUMN_GAP: usize = 2;

/// The transient state of the keymap.
#[derive(Debug, Default)]
pub(crate) struct Help {
    query: String,
    filtering: bool,
    scroll: usize,
}

impl Help {
    #[must_use]
    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub(crate) fn filtering(&self) -> bool {
        self.filtering
    }

    #[must_use]
    pub(crate) fn layout(&self, width: usize, height: usize) -> Layout {
        Layout::new(self, width, height)
    }

    fn requery(&mut self) {
        self.scroll = 0;
    }

    fn scroll_by(&mut self, delta: isize, width: usize, height: usize) {
        let layout = self.layout(width, height);
        self.scroll = layout
            .scroll
            .saturating_add_signed(delta)
            .min(layout.max_scroll);
    }

    fn page_by(&mut self, pages: isize, width: usize, height: usize) {
        let rows = self.layout(width, height).body_rows.max(1);
        let rows = isize::try_from(rows).unwrap_or(isize::MAX);
        self.scroll_by(pages.saturating_mul(rows), width, height);
    }

    fn scroll_end(&mut self, width: usize, height: usize) {
        self.scroll = self.layout(width, height).max_scroll;
    }

    pub(crate) fn clamp(&mut self, width: usize, height: usize) {
        self.scroll = self.layout(width, height).scroll;
    }
}

/// One displayed line in one help column.
#[derive(Debug, Clone)]
pub(crate) enum Cell {
    Heading(String),
    Entry {
        binding: usize,
        key: String,
        label: String,
        key_width: usize,
    },
    Message(String),
}

impl Cell {
    #[must_use]
    pub(crate) fn binding(&self) -> Option<usize> {
        match self {
            Self::Entry { binding, .. } => Some(*binding),
            Self::Heading(_) | Self::Message(_) => None,
        }
    }
}

/// A visible row, with one cell per help column.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub(crate) cells: Vec<Option<Cell>>,
}

/// Geometry and visible content derived from the current screen and help
/// state. Drawing and mouse hit-testing both construct this value, so a
/// resize cannot leave stale click targets behind.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) columns: usize,
    pub(crate) column_width: usize,
    pub(crate) column_gap: usize,
    pub(crate) body_y: usize,
    pub(crate) body_rows: usize,
    pub(crate) rows: Vec<Row>,
    pub(crate) footer: Vec<String>,
    pub(crate) shown_bindings: usize,
    pub(crate) total_bindings: usize,
    pub(crate) scroll: usize,
    pub(crate) max_scroll: usize,
    pub(crate) more_above: bool,
    pub(crate) more_below: bool,
}

impl Layout {
    fn new(help: &Help, screen_width: usize, screen_height: usize) -> Self {
        let x = usize::from(screen_width > 2);
        let y = usize::from(screen_height > 2);
        let width = screen_width.saturating_sub(x * 2).max(1);
        let height = screen_height.saturating_sub(y * 2).max(1);
        let inner_width = width.saturating_sub(2).max(1);
        let columns: usize = if inner_width >= TWO_COLUMN_MIN_WIDTH {
            2
        } else {
            1
        };
        let column_width =
            inner_width.saturating_sub(COLUMN_GAP * columns.saturating_sub(1)) / columns;
        let mut footer = footer_lines(help, inner_width);
        footer.truncate(height.saturating_sub(2));
        let body_y = y + 1;
        let body_rows = height.saturating_sub(2 + footer.len());
        let (lanes, shown_bindings) = content(help, column_width.max(1), columns);
        let max_scroll = lanes
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .saturating_sub(body_rows);
        let scroll = help.scroll.min(max_scroll);
        let mut rows = Vec::with_capacity(body_rows);
        for row in 0..body_rows {
            let cells = (0..columns)
                .map(|column| {
                    lanes
                        .get(column)
                        .and_then(|lane| lane.get(scroll + row))
                        .cloned()
                })
                .collect();
            rows.push(Row { cells });
        }
        Self {
            x,
            y,
            width,
            height,
            columns,
            column_width,
            column_gap: COLUMN_GAP,
            body_y,
            body_rows,
            rows,
            footer,
            shown_bindings,
            total_bindings: BINDINGS.len(),
            scroll,
            max_scroll,
            more_above: scroll > 0,
            more_below: lanes.iter().any(|lane| scroll + body_rows < lane.len()),
        }
    }

    /// The stable binding-table index whose visible wrapped row contains
    /// this screen cell.
    #[must_use]
    pub(crate) fn binding_index_at(&self, column: usize, row: usize) -> Option<usize> {
        let body_row = row.checked_sub(self.body_y)?;
        let visible = self.rows.get(body_row)?;
        let inner_column = column.checked_sub(self.x + 1)?;
        let stride = self.column_width + self.column_gap;
        let help_column = inner_column / stride;
        if help_column >= self.columns || inner_column % stride >= self.column_width {
            return None;
        }
        visible.cells.get(help_column)?.as_ref()?.binding()
    }

    /// The binding whose visible wrapped row contains this screen cell.
    #[must_use]
    pub(crate) fn binding_at(&self, column: usize, row: usize) -> Option<&'static Binding> {
        self.binding_index_at(column, row)
            .and_then(|index| BINDINGS.get(index))
    }

    #[must_use]
    pub(crate) fn footer_y(&self) -> usize {
        self.y + self.height - 1 - self.footer.len()
    }
}

fn footer_lines(help: &Help, width: usize) -> Vec<String> {
    let text = if help.filtering {
        "type to filter  Backspace erase  Enter apply  Esc clear"
    } else if help.query.is_empty() {
        "/ filter  Up/Down scroll  PgUp/PgDn page  Esc close"
    } else {
        "/ edit filter  Up/Down scroll  PgUp/PgDn page  Esc clear"
    };
    wrap_phrases(text, width)
}

fn content(help: &Help, column_width: usize, columns: usize) -> (Vec<Vec<Cell>>, usize) {
    struct Section {
        group: &'static str,
        order: usize,
        bindings: Vec<(usize, &'static Binding)>,
    }

    let query = help.query.trim().to_lowercase();
    let mut sections: Vec<Section> = Vec::new();
    for (index, binding) in BINDINGS.iter().enumerate() {
        if let Some(section) = sections
            .iter_mut()
            .find(|section| section.group == binding.group)
        {
            section.bindings.push((index, binding));
        } else {
            sections.push(Section {
                group: binding.group,
                order: sections.len(),
                bindings: vec![(index, binding)],
            });
        }
    }
    sections.sort_by_key(|section| (section_rank(section.group), section.order));

    let mut blocks = Vec::new();
    let mut shown = 0;
    for section in sections {
        let group = section.group;
        let title = section_title(group);
        let group_match =
            contains_case_insensitive(group, &query) || contains_case_insensitive(title, &query);
        let matches: Vec<_> = section
            .bindings
            .into_iter()
            .filter(|(_, binding)| {
                query.is_empty() || group_match || binding_matches(binding, &query)
            })
            .collect();
        if matches.is_empty() {
            continue;
        }
        let mut cells = vec![Cell::Heading(title.to_owned())];
        shown += matches.len();
        for (index, binding) in matches {
            let keys = binding
                .keys
                .iter()
                .map(|keys| bindings::spell(keys))
                .collect::<Vec<_>>()
                .join(" / ");
            cells.extend(entry_lines(index, &keys, binding.label, column_width));
        }
        blocks.push((group, cells));
    }
    if blocks.is_empty() {
        return (
            vec![vec![Cell::Message(format!(
                "No keys match “{}”",
                help.query
            ))]],
            shown,
        );
    }
    if columns == 1 {
        return (
            vec![blocks.into_iter().flat_map(|(_, cells)| cells).collect()],
            shown,
        );
    }

    let mut lanes = vec![Vec::new(), Vec::new()];
    for (lane, wanted) in [
        (0, &["Move", "Search", "Select"][..]),
        (1, &["Threads", "Display"][..]),
    ] {
        for group in wanted {
            if let Some(index) = blocks.iter().position(|(name, _)| *name == *group) {
                lanes[lane].extend(blocks.remove(index).1);
            }
        }
    }
    for (_, cells) in blocks {
        let lane = usize::from(lanes[1].len() < lanes[0].len());
        lanes[lane].extend(cells);
    }
    (lanes, shown)
}

fn section_rank(group: &str) -> usize {
    match group {
        "Move" => 0,
        "Search" => 1,
        "Select" => 2,
        "Threads" => 3,
        "Display" => 4,
        "Links" => 5,
        _ => 6,
    }
}

fn section_title(group: &str) -> &str {
    match group {
        "Select" => "Select & copy",
        "Display" => "View & diff",
        _ => group,
    }
}

fn binding_matches(binding: &Binding, query: &str) -> bool {
    contains_case_insensitive(binding.label, query)
        || binding
            .keys
            .iter()
            .map(|keys| bindings::spell(keys))
            .any(|keys| contains_case_insensitive(&keys, query))
}

fn contains_case_insensitive(text: &str, query: &str) -> bool {
    query.is_empty() || text.to_lowercase().contains(query)
}

fn entry_lines(binding: usize, keys: &str, label: &str, column_width: usize) -> Vec<Cell> {
    let key_width = display_width(keys)
        .min(MAX_KEY_WIDTH)
        .min(column_width.saturating_sub(4).max(1));
    let label_width = column_width.saturating_sub(key_width + 2).max(1);
    let key_lines = wrap_hard(keys, key_width);
    let label_lines = wrap_words(label, label_width);
    let lines = key_lines.len().max(label_lines.len());
    (0..lines)
        .map(|line| Cell::Entry {
            binding,
            key: key_lines.get(line).cloned().unwrap_or_default(),
            label: label_lines.get(line).cloned().unwrap_or_default(),
            key_width,
        })
        .collect()
}

fn wrap_phrases(text: &str, width: usize) -> Vec<String> {
    if display_width(text) <= width {
        return vec![text.to_owned()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for phrase in text.split("  ") {
        let candidate = if line.is_empty() {
            phrase.to_owned()
        } else {
            format!("{line}  {phrase}")
        };
        if !line.is_empty() && display_width(&candidate) > width {
            lines.push(std::mem::take(&mut line));
            phrase.clone_into(&mut line);
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
        .into_iter()
        .flat_map(|line| wrap_words(&line, width))
        .collect()
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if display_width(word) > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let mut parts = wrap_hard(word, width);
            if let Some(last) = parts.pop() {
                lines.extend(parts);
                line = last;
            }
            continue;
        }
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if display_width(&candidate) > width {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

fn wrap_hard(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let char_width = display_width(&ch.to_string());
        if used > 0 && used + char_width > width {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push(ch);
        used += char_width;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[must_use]
pub(crate) fn state(app: &App) -> Option<&Help> {
    match app.popup() {
        Some(Popup::Help(help)) => Some(help),
        _ => None,
    }
}

fn state_mut(app: &mut App) -> Option<&mut Help> {
    match app.popup.as_mut() {
        Some(Popup::Help(help)) => Some(help),
        _ => None,
    }
}

/// A key while help is open. Help navigation and filter text are consumed
/// here and never dispatched to the document beneath it.
pub(crate) fn key(app: &mut App, event: KeyEvent) -> Effect {
    let (width, height) = app.size();
    let filtering = state(app).is_some_and(Help::filtering);
    let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
    match event.code {
        KeyCode::Esc => {
            let has_filter =
                state(app).is_some_and(|help| help.filtering || !help.query.is_empty());
            if has_filter {
                if let Some(help) = state_mut(app) {
                    help.query.clear();
                    help.filtering = false;
                    help.requery();
                }
            } else {
                app.close_popup();
            }
        }
        KeyCode::Char('/') if plain && !filtering => {
            if let Some(help) = state_mut(app) {
                help.filtering = true;
            }
        }
        KeyCode::Enter if filtering => {
            if let Some(help) = state_mut(app) {
                help.filtering = false;
            }
        }
        KeyCode::Backspace if filtering => {
            if let Some(help) = state_mut(app) {
                help.query.pop();
                help.requery();
            }
        }
        KeyCode::Down | KeyCode::Char('j') if !filtering => {
            if let Some(help) = state_mut(app) {
                help.scroll_by(1, width, height);
            }
        }
        KeyCode::Up | KeyCode::Char('k') if !filtering => {
            if let Some(help) = state_mut(app) {
                help.scroll_by(-1, width, height);
            }
        }
        KeyCode::PageDown if !filtering => {
            if let Some(help) = state_mut(app) {
                help.page_by(1, width, height);
            }
        }
        KeyCode::PageUp if !filtering => {
            if let Some(help) = state_mut(app) {
                help.page_by(-1, width, height);
            }
        }
        KeyCode::Char('d') if !filtering && event.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(help) = state_mut(app) {
                help.page_by(1, width, height);
            }
        }
        KeyCode::Char('u') if !filtering && event.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(help) = state_mut(app) {
                help.page_by(-1, width, height);
            }
        }
        KeyCode::Home | KeyCode::Char('g') if !filtering => {
            if let Some(help) = state_mut(app) {
                help.scroll = 0;
            }
        }
        KeyCode::End | KeyCode::Char('G') if !filtering => {
            if let Some(help) = state_mut(app) {
                help.scroll_end(width, height);
            }
        }
        KeyCode::Char(ch) if filtering && plain => {
            if let Some(help) = state_mut(app) {
                help.query.push(ch);
                help.requery();
            }
        }
        _ => {}
    }
    Effect::None
}

/// Move help by mouse-wheel lines without touching the document.
pub(crate) fn wheel(app: &mut App, delta: isize) -> Effect {
    let (width, height) = app.size();
    if let Some(help) = state_mut(app) {
        help.scroll_by(delta, width, height);
    }
    Effect::None
}

/// Keep help's offset valid after a terminal resize.
pub(crate) fn resize(app: &mut App) {
    let (width, height) = app.size();
    if let Some(help) = state_mut(app) {
        help.clamp(width, height);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier, Style};

    use anyhow::Context as _;
    use fathomable_core::layout::display_width;

    use super::{Cell, Help, state};
    use crate::app::Popup;
    use crate::app::draw;
    use crate::app::input::bindings::{Action, BINDINGS};
    use crate::app::input::{keys, mouse};
    use crate::app::testing::{self, AppBuilder, press};

    fn app(
        name: &str,
        width: usize,
        height: usize,
    ) -> anyhow::Result<(fathomable_testing::TempDir, crate::app::App)> {
        let dir = testing::workspace(&format!("help-{name}"), testing::README)?;
        let mut app = AppBuilder::new(&dir).source_view().width(width).build()?;
        app.resize(width, height);
        Ok((dir, app))
    }

    fn key(app: &mut crate::app::App, code: KeyCode) {
        keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn ctrl(app: &mut crate::app::App, ch: char) {
        keys::handle_key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL));
    }

    fn wheel(app: &mut crate::app::App, down: bool) {
        mouse::handle_mouse(
            app,
            MouseEvent {
                kind: if down {
                    MouseEventKind::ScrollDown
                } else {
                    MouseEventKind::ScrollUp
                },
                column: 2,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
    }

    fn click(app: &mut crate::app::App, column: usize, row: usize) {
        mouse::handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: u16::try_from(column).unwrap_or(u16::MAX),
                row: u16::try_from(row).unwrap_or(u16::MAX),
                modifiers: KeyModifiers::NONE,
            },
        );
    }

    fn move_pointer(app: &mut crate::app::App, column: usize, row: usize) {
        mouse::handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: u16::try_from(column).unwrap_or(u16::MAX),
                row: u16::try_from(row).unwrap_or(u16::MAX),
                modifiers: KeyModifiers::NONE,
            },
        );
    }

    fn heading_column(layout: &super::Layout, wanted: &str) -> Option<usize> {
        layout.rows.iter().find_map(|row| {
            row.cells.iter().enumerate().find_map(|(column, cell)| {
                matches!(cell, Some(Cell::Heading(text)) if text == wanted).then_some(column)
            })
        })
    }

    fn binding_cells(layout: &super::Layout, action: Action) -> Vec<(usize, usize, usize)> {
        layout
            .rows
            .iter()
            .enumerate()
            .flat_map(|(row, visible)| {
                visible
                    .cells
                    .iter()
                    .enumerate()
                    .filter_map(move |(column, cell)| {
                        let index = cell.as_ref()?.binding()?;
                        (BINDINGS[index].action == action).then_some((row, column, index))
                    })
            })
            .collect()
    }

    #[test]
    fn normal_help_is_compact_and_every_binding_is_reachable() {
        let mut help = Help::default();
        let layout = help.layout(80, 24);
        assert_eq!(layout.columns, 2);
        assert_eq!(heading_column(&layout, "Move"), Some(0));
        assert_eq!(heading_column(&layout, "Search"), Some(0));
        assert_eq!(heading_column(&layout, "Select & copy"), Some(0));
        assert_eq!(heading_column(&layout, "Threads"), Some(1));
        let headings: Vec<_> = super::content(&help, layout.column_width, layout.columns)
            .0
            .iter()
            .enumerate()
            .flat_map(|(column, lane)| {
                lane.iter()
                    .enumerate()
                    .filter_map(move |(row, cell)| match cell {
                        Cell::Heading(title) => Some((column, row, title.clone())),
                        Cell::Entry { .. } | Cell::Message(_) => None,
                    })
            })
            .collect();
        assert_eq!(
            heading_column(&layout, "View & diff"),
            Some(1),
            "{headings:?}"
        );
        assert!(layout.more_below);
        assert_eq!(
            layout.footer,
            ["/ filter  Up/Down scroll  PgUp/PgDn page  Esc close"]
        );

        let narrow = help.layout(60, 24);
        assert_eq!(narrow.columns, 1);

        let mut reached = HashSet::new();
        for scroll in 0..=layout.max_scroll {
            help.scroll = scroll;
            for row in help.layout(80, 24).rows {
                for cell in row.cells.into_iter().flatten() {
                    if let Some(binding) = cell.binding() {
                        reached.insert(binding);
                    }
                    match cell {
                        Cell::Entry {
                            key,
                            label,
                            key_width,
                            ..
                        } => {
                            assert!(display_width(&key) <= key_width);
                            assert!(
                                display_width(&label)
                                    <= layout.column_width.saturating_sub(key_width + 2)
                            );
                        }
                        Cell::Heading(_) | Cell::Message(_) => {}
                    }
                }
            }
        }
        assert_eq!(reached.len(), BINDINGS.len());
        help.scroll_end(80, 24);
        let bottom = help.layout(80, 24);
        assert!(!bottom.more_below);
        assert!(bottom.more_above);
    }

    #[test]
    fn help_keys_filter_scroll_and_restore_the_document() -> anyhow::Result<()> {
        let (_dir, mut app) = app("keys", 80, 24)?;
        app.view_mut().goto_row(3);
        press(&mut app, "v");
        let before_cursor = app.view().cursor();
        let before_focus = app.focus();
        let before_selection = app.view().selected_source();
        app.open_help();

        key(&mut app, KeyCode::Down);
        let after_line = state(&app).context("help")?.scroll;
        assert!(after_line > 0);
        ctrl(&mut app, 'd');
        assert!(state(&app).context("help")?.scroll > after_line);
        key(&mut app, KeyCode::PageUp);

        key(&mut app, KeyCode::Char('/'));
        press(&mut app, "no-such-j");
        let help = state(&app).context("help")?;
        assert!(help.filtering());
        assert_eq!(help.query(), "no-such-j");
        assert_eq!(help.layout(80, 24).shown_bindings, 0);
        assert!(
            help.layout(80, 24)
                .rows
                .iter()
                .flat_map(|row| row.cells.iter().flatten())
                .any(|cell| matches!(cell, Cell::Message(text) if text.contains("No keys match")))
        );
        key(&mut app, KeyCode::Backspace);
        assert_eq!(state(&app).context("help")?.query(), "no-such-");
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.popup(), Some(Popup::Help(_))));
        assert_eq!(state(&app).context("help")?.query(), "");
        assert!(!state(&app).context("help")?.filtering());
        key(&mut app, KeyCode::Esc);

        assert!(app.popup().is_none());
        assert_eq!(app.focus(), before_focus);
        assert_eq!(app.view().cursor(), before_cursor);
        assert_eq!(app.view().selected_source(), before_selection);
        Ok(())
    }

    #[test]
    fn wheel_resize_filter_and_wrapped_click_share_current_geometry() -> anyhow::Result<()> {
        let (_dir, mut app) = app("mouse", 80, 24)?;
        app.view_mut().goto_row(3);
        press(&mut app, "ll");
        app.open_help();
        wheel(&mut app, true);
        let wheel_down = state(&app).context("help")?.scroll;
        assert!(wheel_down > 0);
        wheel(&mut app, false);
        assert!(state(&app).context("help")?.scroll < wheel_down);

        key(&mut app, KeyCode::Char('/'));
        press(&mut app, "thread");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::PageDown);
        assert!(state(&app).context("help")?.scroll > 0);
        app.resize(60, 20);
        let resized = state(&app).context("help")?.layout(60, 20);
        assert_eq!(resized.columns, 1);
        assert!(state(&app).context("help")?.scroll <= resized.max_scroll);
        let (hover_row, hover_column, _) = resized
            .rows
            .iter()
            .enumerate()
            .find_map(|(row, visible)| {
                visible.cells.iter().enumerate().find_map(|(column, cell)| {
                    cell.as_ref()?.binding().map(|index| (row, column, index))
                })
            })
            .context("a filtered binding is visible")?;
        let hover_x = resized.x + 1 + hover_column * (resized.column_width + resized.column_gap);
        let hover_y = resized.body_y + hover_row;
        move_pointer(&mut app, hover_x, hover_y);
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let mut theme = draw::Theme::from_core(&core);
        let hover_bg = Color::Rgb(38, 51, 66);
        theme.list_hover = Style::default().bg(hover_bg);
        let mut terminal = Terminal::new(TestBackend::new(60, 20))?;
        terminal.draw(|frame| draw::draw(frame, &app, &theme))?;
        assert_eq!(
            terminal.backend().buffer()[(u16::try_from(hover_x)?, u16::try_from(hover_y)?,)].bg,
            hover_bg,
            "a real mouse-move highlights the current filtered, scrolled layout"
        );
        key(&mut app, KeyCode::Esc);

        key(&mut app, KeyCode::Char('/'));
        press(&mut app, "wraps to row below");
        key(&mut app, KeyCode::Enter);
        app.resize(32, 18);
        let layout = state(&app).context("help")?.layout(32, 18);
        assert_eq!(layout.columns, 1);
        assert!(state(&app).context("help")?.scroll <= layout.max_scroll);
        let cells = binding_cells(&layout, Action::MoveRight);
        assert!(cells.len() > 1, "the long description wraps at 32 columns");
        let (row, column, _) = cells[1];
        let screen_column = layout.x + 1 + column * (layout.column_width + layout.column_gap);
        let screen_row = layout.body_y + row;
        let before = app.view().cursor().col;
        click(&mut app, screen_column, screen_row);
        assert!(app.popup().is_none());
        assert_eq!(app.view().cursor().col, before + 1);
        Ok(())
    }

    #[test]
    fn help_uses_popup_and_key_styles_without_forcing_bold() -> anyhow::Result<()> {
        let (_dir, mut app) = app("theme", 80, 24)?;
        app.open_help();
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let mut theme = draw::Theme::from_core(&core);
        theme.popup = Style::default()
            .fg(Color::Gray)
            .bg(Color::Rgb(9, 18, 30))
            .add_modifier(Modifier::ITALIC);
        theme.menu = Style::default().bg(Color::LightYellow);
        theme.popup_key = Style::default().fg(Color::LightBlue);
        let layout = state(&app).context("help")?.layout(80, 24);
        let (row, column, _) = binding_cells(&layout, Action::MoveDown)[0];
        let x = layout.x + 1 + column * (layout.column_width + layout.column_gap);
        let y = layout.body_y + row;
        let mut terminal = Terminal::new(TestBackend::new(80, 24))?;
        terminal.draw(|frame| draw::draw(frame, &app, &theme))?;
        let cell = &terminal.backend().buffer()[(
            u16::try_from(x).unwrap_or(u16::MAX),
            u16::try_from(y).unwrap_or(u16::MAX),
        )];
        assert_eq!(cell.fg, Color::LightBlue);
        assert_eq!(cell.bg, Color::Rgb(9, 18, 30));
        assert!(cell.modifier.contains(Modifier::ITALIC));
        assert!(!cell.modifier.contains(Modifier::BOLD));
        let heading_row = layout
            .rows
            .iter()
            .position(|row| matches!(&row.cells[0], Some(Cell::Heading(text)) if text == "Move"))
            .context("Move heading")?;
        let heading = &terminal.backend().buffer()[(
            u16::try_from(layout.x + 1)?,
            u16::try_from(layout.body_y + heading_row)?,
        )];
        assert_eq!(heading.fg, Color::Gray);
        assert_eq!(heading.bg, Color::Rgb(9, 18, 30));
        assert!(heading.modifier.contains(Modifier::BOLD));
        assert!(heading.modifier.contains(Modifier::ITALIC));
        Ok(())
    }

    #[test]
    fn help_info_keeps_opaque_and_transparent_popup_backgrounds() -> anyhow::Result<()> {
        let (_dir, mut app) = app("info-background", 80, 24)?;
        app.open_help();
        let layout = state(&app).context("help")?.layout(80, 24);
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let mut theme = draw::Theme::from_core(&core);
        let popup_bg = Color::Rgb(17, 34, 51);
        let intrusive_bg = Color::Rgb(238, 238, 238);
        let subdued = Color::DarkGray;
        theme.popup = Style::default().fg(Color::Gray).bg(popup_bg);
        theme.info = Style::default().fg(subdued).bg(intrusive_bg);

        let render = |theme: &draw::Theme| -> anyhow::Result<ratatui::buffer::Buffer> {
            let mut terminal = Terminal::new(TestBackend::new(80, 24))?;
            terminal.draw(|frame| draw::draw(frame, &app, theme))?;
            Ok(terminal.backend().buffer().clone())
        };
        let footer = (
            u16::try_from(layout.x + 1)?,
            u16::try_from(layout.footer_y())?,
        );
        let count = (
            u16::try_from(layout.x + layout.width - 2)?,
            u16::try_from(layout.y)?,
        );
        let buffer = render(&theme)?;
        for at in [footer, count] {
            assert_eq!(buffer[at].fg, subdued);
            assert_eq!(buffer[at].bg, popup_bg);
            assert_ne!(buffer[at].bg, intrusive_bg);
        }

        theme.popup = Style::default().fg(Color::Gray);
        let buffer = render(&theme)?;
        for at in [footer, count] {
            assert_eq!(buffer[at].fg, subdued);
            assert_eq!(
                buffer[at].bg,
                Color::Reset,
                "an info background must not create an opaque stripe"
            );
        }
        Ok(())
    }
}
