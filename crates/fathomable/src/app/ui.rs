// @okf-doc: /decisions/0012-workspace-mode.md
//! Draw the app with ratatui: sidebar, gutter and text, popups, status line.

use fathomable_core::layout::{Face, Style as Face_, display_width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use fathomable_core::annotations::Status;

use super::threads::{Compose, ComposeTarget, MarkKind, ThreadPanel};
use super::view::{Mode, View};
use super::{App, Focus, HELP, PickerState, Popup, SPACE_MENU};

/// Most rows the comment box grows to before it scrolls.
const COMPOSE_MAX_ROWS: usize = 8;

/// Snippet lines quoted at the top of the thread panel.
const SNIPPET_ROWS: usize = 3;

/// Ratatui styles for the chrome and Markdown faces.
///
/// Built from a resolved [`fathomable_core::theme::Theme`] (ADR 0011) so the
/// draw code never touches theme keys directly.
#[derive(Debug, Clone)]
pub struct Theme {
    pub text: Style,
    pub heading: [Style; 6],
    pub code: Style,
    pub code_block: Style,
    pub link: Style,
    pub marker: Style,
    pub quote: Style,
    pub line_number: Style,
    pub cursorline: Style,
    pub selection: Style,
    pub search_match: Style,
    pub statusline: Style,
    pub info: Style,
    pub mode_normal: Style,
    pub mode_select: Style,
    pub mode_input: Style,
    pub sidebar: Style,
    pub sidebar_selected: Style,
    pub sidebar_dir: Style,
    pub popup: Style,
    pub popup_key: Style,
    pub picker_match: Style,
    pub picker_selected: Style,
    pub annotation_open: Style,
    pub annotation_resolved: Style,
    pub annotation_auto: Style,
    pub annotation_detached: Style,
    pub annotation_line: Style,
}

impl Theme {
    /// Convert a resolved core theme into ratatui styles.
    pub fn from_core(theme: &fathomable_core::theme::Theme) -> Self {
        use fathomable_core::theme::Key;
        let style = |key: Key| convert_style(theme.style(key));
        Self {
            text: style(Key::UiText),
            heading: std::array::from_fn(|i| {
                style(Key::MarkupHeadingLevel(u8::try_from(i + 1).unwrap_or(1)))
            }),
            code: style(Key::MarkupRawInline),
            code_block: style(Key::MarkupRawBlock),
            link: style(Key::MarkupLink),
            marker: style(Key::MarkupList),
            quote: style(Key::MarkupQuote),
            line_number: style(Key::UiLinenr),
            cursorline: style(Key::UiCursorline),
            selection: style(Key::UiSelection),
            search_match: style(Key::UiSearchMatch),
            statusline: style(Key::UiStatusline),
            info: style(Key::UiStatuslineInfo),
            mode_normal: style(Key::UiStatuslineNormal),
            mode_select: style(Key::UiStatuslineSelect),
            mode_input: style(Key::UiStatuslineInput),
            sidebar: style(Key::UiSidebar),
            sidebar_selected: style(Key::UiSidebarSelected),
            sidebar_dir: style(Key::UiSidebarDir),
            popup: style(Key::UiPopup),
            popup_key: style(Key::UiPopupKey),
            picker_match: style(Key::UiPickerMatch),
            picker_selected: style(Key::UiPickerSelected),
            annotation_open: style(Key::AnnotationOpen),
            annotation_resolved: style(Key::AnnotationResolved),
            annotation_auto: style(Key::AnnotationResolvedAuto),
            annotation_detached: style(Key::AnnotationDetached),
            annotation_line: style(Key::AnnotationLine),
        }
    }
}

fn convert_style(style: fathomable_core::theme::Style) -> Style {
    let mut out = Style::default();
    if let Some(fg) = style.fg() {
        out = out.fg(convert_color(fg));
    }
    if let Some(bg) = style.bg() {
        out = out.bg(convert_color(bg));
    }
    let mods = style.modifiers();
    let flags = [
        (mods.bold, Modifier::BOLD),
        (mods.dim, Modifier::DIM),
        (mods.italic, Modifier::ITALIC),
        (mods.underline, Modifier::UNDERLINED),
        (mods.reversed, Modifier::REVERSED),
        (mods.strikethrough, Modifier::CROSSED_OUT),
    ];
    for (on, modifier) in flags {
        if on {
            out = out.add_modifier(modifier);
        }
    }
    out
}

fn convert_color(color: fathomable_core::theme::Color) -> Color {
    use fathomable_core::theme::{AnsiColor, Color as Core};
    match color {
        Core::Rgb(r, g, b) => Color::Rgb(r, g, b),
        Core::Ansi(ansi) => match ansi {
            AnsiColor::Black => Color::Black,
            AnsiColor::Red => Color::Red,
            AnsiColor::Green => Color::Green,
            AnsiColor::Yellow => Color::Yellow,
            AnsiColor::Blue => Color::Blue,
            AnsiColor::Magenta => Color::Magenta,
            AnsiColor::Cyan => Color::Cyan,
            AnsiColor::White => Color::White,
            AnsiColor::BrightBlack => Color::DarkGray,
            AnsiColor::BrightRed => Color::LightRed,
            AnsiColor::BrightGreen => Color::LightGreen,
            AnsiColor::BrightYellow => Color::LightYellow,
            AnsiColor::BrightBlue => Color::LightBlue,
            AnsiColor::BrightMagenta => Color::LightMagenta,
            AnsiColor::BrightCyan => Color::LightCyan,
            AnsiColor::BrightWhite => Color::Gray,
        },
    }
}

/// Width of the gutter: line numbers, a space, the diff bar cell, and the
/// annotation cell (ADR 0013).
pub fn gutter_width(view: &View) -> usize {
    let digits = view.index().line_count().max(1).to_string().len();
    digits + 3
}

fn u16_of(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub fn draw(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = frame.area();
    let rows = app.pane_rows();
    let sidebar = app.sidebar_width();
    let view = app.view();
    let gutter = gutter_width(view);
    let pane_height = u16_of(rows).min(area.height);
    let sidebar_area = Rect {
        width: u16_of(sidebar),
        height: pane_height,
        ..area
    };
    let text_area = Rect {
        x: area.x + sidebar_area.width,
        width: area.width.saturating_sub(sidebar_area.width),
        height: pane_height,
        ..area
    };
    let status_area = Rect {
        y: area.y + pane_height,
        height: area.height.saturating_sub(pane_height),
        ..area
    };

    if let Some(tree) = app.tree() {
        frame.render_widget(
            Paragraph::new(sidebar_lines(app, tree, theme, sidebar, rows)).style(theme.sidebar),
            sidebar_area,
        );
    }
    frame.render_widget(
        Paragraph::new(text_lines(app, theme, gutter, rows)).style(theme.text),
        text_area,
    );
    frame.render_widget(
        status_line(app, theme, usize::from(area.width)),
        status_area,
    );

    match app.popup() {
        Some(Popup::Space) => draw_menu(frame, theme, text_area, &space_entries()),
        Some(Popup::Help) => draw_help(frame, theme, area),
        Some(Popup::Picker(picker)) => {
            draw_picker(frame, theme, area, picker);
        }
        Some(Popup::Compose(compose)) => {
            let box_rows = compose_rows(compose, text_area);
            if let Some(panel) = compose.panel() {
                // The thread stays readable above the box while replying.
                let above = Rect {
                    height: text_area.height.saturating_sub(u16_of(box_rows)),
                    ..text_area
                };
                let rows = usize::from(above.height / 2).clamp(4, usize::from(above.height.max(1)));
                draw_thread(frame, app, theme, above, panel, rows);
            }
            draw_compose(frame, app, theme, text_area, compose, box_rows);
        }
        Some(Popup::Thread(panel)) => {
            let rows =
                usize::from(text_area.height / 3).clamp(6, usize::from(text_area.height.max(1)));
            draw_thread(frame, app, theme, text_area, panel, rows);
        }
        None => {
            if view.pending() == Some('g') {
                let entries = vec![
                    ("g".to_owned(), "go to top".to_owned()),
                    ("s".to_owned(), "toggle source view".to_owned()),
                ];
                draw_menu(frame, theme, text_area, &entries);
            }
            place_cursor(frame, app, view, text_area, status_area, gutter);
        }
    }
}

fn place_cursor(
    frame: &mut Frame<'_>,
    app: &App,
    view: &View,
    text_area: Rect,
    status_area: Rect,
    gutter: usize,
) {
    if matches!(view.mode(), Mode::Command | Mode::Search { .. }) {
        let col = 1 + display_width(view.input());
        frame.set_cursor_position((status_area.x + u16_of(col), status_area.y));
    } else if app.focus() == Focus::Sidebar {
        if let Some(tree) = app.tree() {
            let row = tree.cursor().saturating_sub(app.sidebar_scroll()) + 1;
            frame.set_cursor_position((text_area.x.saturating_sub(1), u16_of(row)));
        }
    } else {
        let cursor = view.cursor();
        let screen_row = cursor.row.saturating_sub(view.scroll());
        frame.set_cursor_position((
            text_area.x + u16_of(gutter + cursor.col),
            text_area.y + u16_of(screen_row),
        ));
    }
}

fn space_entries() -> Vec<(String, String)> {
    SPACE_MENU
        .iter()
        .map(|(key, label)| (key.to_string(), (*label).to_owned()))
        .collect()
}

fn sidebar_lines<'a>(
    app: &App,
    tree: &'a fathomable_core::tree::Tree,
    theme: &Theme,
    width: usize,
    rows: usize,
) -> Vec<Line<'a>> {
    let inner = width.saturating_sub(1);
    let divider = Span::styled("│", theme.marker);
    let root = app
        .workspace()
        .root()
        .file_name()
        .map_or_else(|| "/".to_owned(), |n| n.to_string_lossy().into_owned());
    let mut out = Vec::with_capacity(rows);
    out.push(Line::from(vec![
        Span::styled(
            fit(&format!(" {root}"), inner),
            theme.sidebar_dir.add_modifier(Modifier::BOLD),
        ),
        divider.clone(),
    ]));
    let focused = app.focus() == Focus::Sidebar;
    for (index, row) in tree
        .rows()
        .iter()
        .enumerate()
        .skip(app.sidebar_scroll())
        .take(rows.saturating_sub(1))
    {
        let marker = if row.is_dir() {
            if row.expanded() { "▾ " } else { "▸ " }
        } else {
            "  "
        };
        let text = format!(
            "{}{marker}{}{}",
            " ".repeat(row.depth() + 1),
            row.name(),
            if row.is_dir() { "/" } else { "" }
        );
        let mut style = if row.is_dir() {
            theme.sidebar_dir
        } else {
            theme.sidebar
        };
        if index == tree.cursor() {
            style = style.patch(theme.sidebar_selected);
            if !focused {
                style = style.remove_modifier(Modifier::BOLD);
            }
        }
        out.push(Line::from(vec![
            Span::styled(fit(&text, inner), style),
            divider.clone(),
        ]));
    }
    while out.len() < rows {
        out.push(Line::from(vec![
            Span::raw(" ".repeat(inner)),
            divider.clone(),
        ]));
    }
    out
}

/// Pad or truncate `text` to exactly `width` cells.
fn fit(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = display_width(&ch.to_string());
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    if used < width {
        out.push_str(&" ".repeat(width - used));
    }
    out
}

fn face_style(theme: &Theme, face: &Face_) -> Style {
    let base = match &face.face {
        Face::Text => theme.text,
        Face::Heading(level) => theme.heading[usize::from(level.clamp(&1, &6) - 1)],
        Face::Code => theme.code,
        Face::CodeBlock => theme.code_block,
        Face::Link(_) => theme.link,
        Face::Marker => theme.marker,
        Face::Quote => theme.quote,
    };
    let mut style = base;
    if face.emphasis {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if face.strong {
        style = style.add_modifier(Modifier::BOLD);
    }
    if face.strikethrough {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    style
}

fn mark_style(theme: &Theme, kind: MarkKind) -> Style {
    match kind {
        MarkKind::Open => theme.annotation_open,
        MarkKind::Resolved => theme.annotation_resolved,
        MarkKind::AutoResolved => theme.annotation_auto,
        MarkKind::Detached => theme.annotation_detached,
    }
}

fn text_lines<'a>(app: &'a App, theme: &Theme, gutter: usize, rows: usize) -> Vec<Line<'a>> {
    let view = app.view();
    let digits = gutter - 3;
    let cursor = view.cursor();
    let selection = view.selection();
    let lines = view.layout().lines();
    let mut out = Vec::with_capacity(rows);
    for (row, line) in lines.iter().enumerate().skip(view.scroll()).take(rows) {
        let is_cursor = row == cursor.row;
        let mark = view.source_lines_of_row(row).and_then(|n| app.mark_in(n));
        let mut row_style = Style::default();
        if mark.is_some() {
            row_style = row_style.patch(theme.annotation_line);
        }
        if is_cursor {
            row_style = row_style.patch(theme.cursorline);
        }
        let number = line
            .source_line()
            .map_or_else(|| " ".repeat(digits), |n| format!("{n:>digits$}"));
        // Number, space, diff bar (empty until ADR 0006 lands), note cell.
        let note = mark.map_or_else(
            || Span::styled(" ", row_style),
            |kind| Span::styled("▎", mark_style(theme, kind).patch(row_style)),
        );
        let mut spans = vec![
            Span::styled(number, theme.line_number.patch(row_style)),
            Span::styled("  ", theme.marker.patch(row_style)),
            note,
        ];
        let matches: Vec<_> = view.matches().iter().filter(|m| m.row == row).collect();
        let mut col = 0;
        for span in line.spans() {
            let base = face_style(theme, span.style()).patch(row_style);
            // Split the span per character so selection and match highlights
            // can start and end mid-span.
            for grapheme in grapheme_cells(span.text()) {
                let mut style = base;
                if matches.iter().any(|m| col >= m.start && col < m.end) {
                    style = style.patch(theme.search_match);
                }
                if selection.is_some_and(|s| s.contains(row, col)) {
                    style = style.patch(theme.selection);
                }
                spans.push(Span::styled(grapheme, style));
                col += display_width(grapheme);
            }
        }
        out.push(Line::from(spans).style(row_style));
    }
    out
}

fn grapheme_cells(text: &str) -> impl Iterator<Item = &str> {
    // Splitting at char boundaries is enough for styling; combining marks
    // stay attached to the preceding cell in the terminal.
    text.char_indices()
        .map(|(i, ch)| &text[i..i + ch.len_utf8()])
}

fn status_line<'a>(app: &'a App, theme: &Theme, width: usize) -> Paragraph<'a> {
    let view = app.view();
    let mode = view.mode();
    if matches!(mode, Mode::Command | Mode::Search { .. }) {
        let prompt = match mode {
            Mode::Command => ":",
            Mode::Search { backward: true } => "?",
            _ => "/",
        };
        let mut spans = vec![Span::raw(prompt), Span::raw(view.input())];
        if let Some(message) = view.message() {
            spans.push(Span::styled(format!("  {message}"), theme.info));
        }
        return Paragraph::new(Line::from(spans)).style(theme.statusline);
    }
    let pill_style = match mode {
        Mode::Normal => theme.mode_normal,
        Mode::Select => theme.mode_select,
        _ => theme.mode_input,
    };
    let label = if app.focus() == Focus::Sidebar {
        "TREE".to_owned()
    } else if view.source_view() && mode == Mode::Normal {
        "SRC".to_owned()
    } else {
        mode.to_string()
    };
    let (line, col) = view.source_position();
    let threads = match app.thread_counts() {
        (_, 0) => String::new(),
        (open, total) => format!("{open}/{total} threads  "),
    };
    let right = format!(
        " {line}:{col}  {}%  {threads}{} ",
        view.percent(),
        app.session()
    );
    // Keep the right-hand block visible by trimming the path from the left.
    let fixed = display_width(&label) + 3 + display_width(&right) + 8;
    let path = app.current_path().to_string_lossy();
    let path = truncate_left(&path, width.saturating_sub(fixed));
    let mut left = vec![
        Span::styled(format!(" {label} "), pill_style),
        Span::raw(format!(" {path}")),
    ];
    if view.changed() {
        left.push(Span::styled(" [+]", theme.info));
    }
    if let Some(pending) = app.pending() {
        left.push(Span::styled(format!("  {pending}"), theme.info));
    }
    if let Some(message) = app.message().or_else(|| view.message()) {
        left.push(Span::styled(format!("  {message}"), theme.info));
    }
    let used: usize = left
        .iter()
        .map(|s| display_width(&s.content))
        .sum::<usize>()
        + display_width(&right);
    let pad = width.saturating_sub(used);
    left.push(Span::raw(" ".repeat(pad)));
    left.push(Span::raw(right));
    Paragraph::new(Line::from(left)).style(theme.statusline)
}

/// Drop leading characters so `text` fits `max` cells, marking the cut with `…`.
fn truncate_left(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() && display_width(&chars.iter().collect::<String>()) + 1 > max {
        chars.remove(0);
    }
    format!("…{}", chars.into_iter().collect::<String>())
}

/// A Helix-style key menu anchored to the bottom of `pane`, laid out in
/// columns when the entries do not fit in the rows available.
fn draw_menu(frame: &mut Frame<'_>, theme: &Theme, pane: Rect, entries: &[(String, String)]) {
    if pane.height < 2 || entries.is_empty() {
        return;
    }
    let key_width = entries
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(1);
    let label_width = entries
        .iter()
        .map(|(_, l)| display_width(l))
        .max()
        .unwrap_or(1);
    let column_width = key_width + 2 + label_width + 3;
    let max_rows = usize::from(pane.height.saturating_sub(1)).clamp(1, 8);
    let columns = entries.len().div_ceil(max_rows);
    let rows = entries.len().div_ceil(columns);
    let mut lines = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut spans = vec![Span::raw(" ")];
        for c in 0..columns {
            let Some((key, label)) = entries.get(c * rows + r) else {
                break;
            };
            spans.push(Span::styled(format!("{key:>key_width$}"), theme.popup_key));
            spans.push(Span::raw(format!("  {label:<label_width$}   ")));
        }
        lines.push(Line::from(spans));
    }
    let width = u16_of(columns * column_width + 1).min(pane.width);
    let area = Rect {
        x: pane.x,
        y: pane.y + pane.height - u16_of(rows),
        width,
        height: u16_of(rows),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), area);
}

fn draw_help(frame: &mut Frame<'_>, theme: &Theme, area: Rect) {
    let key_width = HELP
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(1);
    let lines: Vec<Line<'_>> = std::iter::once(Line::from(Span::styled(
        " Keys (any key closes)",
        theme.popup_key,
    )))
    .chain(HELP.iter().map(|(key, label)| {
        Line::from(vec![
            Span::styled(format!(" {key:<key_width$}"), theme.popup_key),
            Span::raw(format!("  {label}")),
        ])
    }))
    .collect();
    let height = u16_of(lines.len()).min(area.height.saturating_sub(1));
    let width = u16_of(
        lines
            .iter()
            .map(|l| display_width(&l.to_string()) + 2)
            .max()
            .unwrap_or(20),
    )
    .min(area.width);
    let popup = centred(area, width, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), popup);
}

fn draw_picker(frame: &mut Frame<'_>, theme: &Theme, area: Rect, picker: &PickerState) {
    let width = area.width.saturating_sub(4).clamp(20, 90);
    let height = area.height.saturating_sub(2).clamp(3, 20);
    let popup = centred(area, width, height);
    let list_rows = usize::from(height) - 1;
    let selected = picker.selected();
    let first = selected.saturating_sub(list_rows.saturating_sub(1));
    let title = match picker.kind() {
        super::PickerKind::Files => "files",
        super::PickerKind::AllFiles => "files (incl. ignored)",
        super::PickerKind::Recent => "recent",
        super::PickerKind::Threads => "threads",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!(" {title} > "), theme.popup_key),
        Span::raw(picker.input().to_owned()),
        Span::styled(
            format!("   {}/{}", picker.matches().len(), picker.total()),
            theme.info,
        ),
    ])];
    let inner = usize::from(width).saturating_sub(2);
    for (index, m) in picker
        .matches()
        .iter()
        .enumerate()
        .skip(first)
        .take(list_rows)
    {
        let item = picker.item(m);
        let row_style = if index == selected {
            theme.picker_selected
        } else {
            Style::default()
        };
        let mut spans = vec![Span::styled(" ", row_style)];
        let mut used = 1;
        for (char_index, ch) in item.chars().enumerate() {
            let w = display_width(&ch.to_string());
            if used + w > inner {
                break;
            }
            let matched = m
                .positions()
                .binary_search(&u32::try_from(char_index).unwrap_or(u32::MAX))
                .is_ok();
            let style = if matched {
                theme.picker_match.patch(row_style)
            } else {
                row_style
            };
            spans.push(Span::styled(ch.to_string(), style));
            used += w;
        }
        spans.push(Span::styled(
            " ".repeat(inner.saturating_sub(used) + 1),
            row_style,
        ));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), popup);
    let col = 1 + display_width(title) + 3 + display_width(picker.input());
    frame.set_cursor_position((popup.x + u16_of(col), popup.y));
}

/// Rows the comment box takes: its text plus a title, capped.
fn compose_rows(compose: &Compose, pane: Rect) -> usize {
    (compose.text().split('\n').count() + 1)
        .min(COMPOSE_MAX_ROWS)
        .min(usize::from(pane.height))
}

/// The comment box: grows up from the status line (ADR 0005, 0013).
fn draw_compose(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    pane: Rect,
    compose: &Compose,
    rows: usize,
) {
    let title = match compose.target() {
        ComposeTarget::New(range) => format!(" comment on L{range}"),
        ComposeTarget::Reply(id) => {
            let range = app
                .marks()
                .iter()
                .find(|m| m.id() == id)
                .map_or_else(String::new, |m| format!(" on L{}", m.range()));
            format!(" reply{range}")
        }
    };
    let hint = if compose.panel().is_some() {
        "  Enter newline · Ctrl/Alt-Enter submit · Up/Down scroll thread · Esc"
    } else {
        "  Enter newline · Ctrl-Enter / Alt-Enter submit · Esc cancel"
    };
    let width = usize::from(pane.width);
    let text: Vec<&str> = compose.text().split('\n').collect();
    if rows < 2 {
        return;
    }
    let body_rows = rows - 1;
    let first = text.len().saturating_sub(body_rows);
    let mut lines = vec![Line::from(vec![
        Span::styled(title, theme.popup_key),
        Span::styled(fit(hint, width), theme.info),
    ])];
    for line in &text[first..] {
        lines.push(Line::from(Span::raw(fit(&format!(" {line}"), width))));
    }
    let area = Rect {
        x: pane.x,
        y: pane.y + pane.height - u16_of(rows),
        width: pane.width,
        height: u16_of(rows),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), area);
    let last = text.last().copied().unwrap_or_default();
    let col = (1 + display_width(last)).min(width.saturating_sub(1));
    frame.set_cursor_position((area.x + u16_of(col), area.y + u16_of(rows - 1)));
}

/// The thread panel: header, quoted snippet, comment, replies.
fn draw_thread(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    pane: Rect,
    panel: &ThreadPanel,
    rows: usize,
) {
    let Some(thread) = app.thread(panel.id()) else {
        return;
    };
    if rows == 0 || pane.height == 0 {
        return;
    }
    let width = usize::from(pane.width);
    let inner = width.saturating_sub(2);
    let mark = app.marks().iter().find(|m| m.id() == thread.id());
    let (index, total) = panel.position();
    let status = match (mark.map(super::threads::Mark::kind), thread.status()) {
        (Some(MarkKind::Detached), _) => "detached",
        (_, Status::Open) => "open",
        (_, Status::Resolved) => "resolved",
        (_, Status::AutoResolved) => "auto-resolved",
    };
    let range = mark.map_or_else(|| thread.range().to_string(), |m| m.range().to_string());
    let which = if total > 1 {
        format!("thread {index}/{total}")
    } else {
        "thread".to_owned()
    };
    let hint = "  r reply · x resolve/reopen · n/p switch · j/k scroll · Esc";
    let mut lines = vec![Line::from(vec![
        Span::styled(format!(" {which}  L{range}  {status}"), theme.popup_key),
        Span::styled(hint, theme.info),
    ])];
    let mut body: Vec<Line<'_>> = Vec::new();
    let snippet: Vec<&str> = thread.snippet().lines().collect();
    for line in snippet.iter().take(SNIPPET_ROWS) {
        body.push(Line::from(Span::styled(
            fit(&format!(" │ {line}"), width),
            theme.info,
        )));
    }
    if snippet.len() > SNIPPET_ROWS {
        body.push(Line::from(Span::styled(" │ …", theme.info)));
    }
    body.push(Line::from(""));
    body.extend(message_lines(
        theme,
        "user",
        thread.created(),
        thread.comment(),
        "",
        inner,
    ));
    for reply in thread.replies() {
        body.push(Line::from(""));
        let tag = if reply.proposes_resolution() {
            "  proposes resolving"
        } else {
            ""
        };
        body.extend(message_lines(
            theme,
            &reply.author().to_string(),
            reply.created(),
            reply.body(),
            tag,
            inner,
        ));
    }
    let body_rows = rows - 1;
    let scroll = panel.scroll().min(body.len().saturating_sub(body_rows));
    lines.extend(body.into_iter().skip(scroll).take(body_rows));
    let area = Rect {
        x: pane.x,
        y: pane.y + pane.height - u16_of(rows),
        width: pane.width,
        height: u16_of(rows),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), area);
}

/// `author  time[tag]` then the wrapped body, indented one cell.
fn message_lines<'a>(
    theme: &Theme,
    author: &str,
    created: u64,
    body: &str,
    tag: &str,
    width: usize,
) -> Vec<Line<'a>> {
    let mut out = vec![Line::from(vec![
        Span::styled(format!(" {author}"), theme.popup_key),
        Span::styled(format!("  {}{tag}", format_time(created)), theme.info),
    ])];
    for paragraph in body.lines() {
        for line in wrap(paragraph, width.saturating_sub(1).max(1)) {
            out.push(Line::from(Span::raw(format!(" {line}"))));
        }
    }
    out
}

/// Greedy word wrap to `width` cells; words longer than a line are split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let mut word = word.to_owned();
        loop {
            let current = lines
                .last_mut()
                .unwrap_or_else(|| unreachable!("always one line"));
            let sep = usize::from(!current.is_empty());
            if display_width(current) + sep + display_width(&word) <= width {
                if sep == 1 {
                    current.push(' ');
                }
                current.push_str(&word);
                break;
            }
            if current.is_empty() {
                // Split a word that cannot fit on an empty line.
                let mut taken = String::new();
                let mut rest = String::new();
                for ch in word.chars() {
                    if rest.is_empty()
                        && display_width(&taken) + display_width(&ch.to_string()) <= width
                    {
                        taken.push(ch);
                    } else {
                        rest.push(ch);
                    }
                }
                if taken.is_empty() {
                    taken = rest.chars().take(1).collect();
                    rest = rest.chars().skip(1).collect();
                }
                current.push_str(&taken);
                if rest.is_empty() {
                    break;
                }
                word = rest;
            }
            lines.push(String::new());
        }
    }
    lines
}

/// `YYYY-MM-DD HH:MM` in UTC from Unix seconds (Howard Hinnant's civil-date
/// algorithm; no calendar crate needed for a timestamp label).
fn format_time(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 3,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::{format_time, wrap};

    #[test]
    fn wraps_words_and_splits_long_ones() {
        assert_eq!(wrap("the quick brown fox", 9), ["the quick", "brown fox"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 4), [""]);
    }

    #[test]
    fn formats_unix_seconds_as_utc() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00");
    }
}
