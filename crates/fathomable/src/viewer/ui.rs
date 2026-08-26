// @okf-doc: /decisions/0010-viewer-ux.md
//! Draw the view with ratatui: gutter, text, and the status line.

use fathomable_core::layout::{Face, Style as Face_, display_width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::view::{Mode, View};

/// Colours for the viewer chrome and Markdown faces.
///
/// The key names follow ADR 0010 so a KDL theme can populate this later; the
/// values are the built-in default until the theme format exists.
#[derive(Debug, Clone)]
pub struct Theme {
    pub text: Style,
    pub heading: Style,
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
    pub mode_normal: Style,
    pub mode_select: Style,
    pub mode_input: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Style::default(),
            heading: Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
            code: Style::default().fg(Color::Yellow),
            code_block: Style::default().fg(Color::Yellow),
            link: Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED),
            marker: Style::default().fg(Color::DarkGray),
            quote: Style::default().fg(Color::Green),
            line_number: Style::default().fg(Color::DarkGray),
            cursorline: Style::default().bg(Color::Indexed(236)),
            selection: Style::default().bg(Color::Indexed(24)),
            search_match: Style::default().fg(Color::Black).bg(Color::Yellow),
            statusline: Style::default().bg(Color::Indexed(236)),
            mode_normal: Style::default()
                .fg(Color::Black)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
            mode_select: Style::default()
                .fg(Color::Black)
                .bg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
            mode_input: Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }
}

/// Text describing where the view is, for the status line.
#[derive(Debug, Clone)]
pub struct StatusInfo<'a> {
    pub path: &'a str,
    pub session: &'a str,
}

/// Width of the gutter: line numbers, a space, and the diff bar cell.
pub fn gutter_width(view: &View) -> usize {
    let digits = view.index().line_count().max(1).to_string().len();
    digits + 2
}

/// Number of rows available to text once the status line is taken.
pub fn text_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(1))
}

pub fn draw(frame: &mut Frame<'_>, view: &View, theme: &Theme, status: &StatusInfo<'_>) {
    let area = frame.area();
    let rows = text_rows(area);
    let gutter = gutter_width(view);
    let text_area = Rect {
        height: u16::try_from(rows).unwrap_or(u16::MAX),
        ..area
    };
    let status_area = Rect {
        y: area.y + text_area.height,
        height: area.height.saturating_sub(text_area.height),
        ..area
    };
    frame.render_widget(
        Paragraph::new(text_lines(view, theme, gutter, rows)),
        text_area,
    );
    frame.render_widget(
        status_line(view, theme, status, usize::from(area.width)),
        status_area,
    );

    if matches!(view.mode(), Mode::Command | Mode::Search { .. }) {
        let col = 1 + display_width(view.input());
        frame.set_cursor_position((
            status_area.x + u16::try_from(col).unwrap_or(u16::MAX),
            status_area.y,
        ));
    } else {
        let cursor = view.cursor();
        let screen_row = cursor.row.saturating_sub(view.scroll());
        frame.set_cursor_position((
            text_area.x + u16::try_from(gutter + cursor.col).unwrap_or(u16::MAX),
            text_area.y + u16::try_from(screen_row).unwrap_or(u16::MAX),
        ));
    }
}

fn face_style(theme: &Theme, face: &Face_) -> Style {
    let base = match &face.face {
        Face::Text => theme.text,
        Face::Heading(_) => theme.heading,
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

fn text_lines<'a>(view: &'a View, theme: &Theme, gutter: usize, rows: usize) -> Vec<Line<'a>> {
    let digits = gutter - 2;
    let cursor = view.cursor();
    let selection = view.selection();
    let lines = view.layout().lines();
    let mut out = Vec::with_capacity(rows);
    for (row, line) in lines.iter().enumerate().skip(view.scroll()).take(rows) {
        let is_cursor = row == cursor.row;
        let row_style = if is_cursor {
            theme.cursorline
        } else {
            Style::default()
        };
        let number = line
            .source_line()
            .map_or_else(|| " ".repeat(digits), |n| format!("{n:>digits$}"));
        // Number, space, diff bar (empty until ADR 0006 lands).
        let mut spans = vec![
            Span::styled(number, theme.line_number.patch(row_style)),
            Span::styled("  ", theme.marker.patch(row_style)),
        ];
        let matches: Vec<_> = view.matches().iter().filter(|m| m.row == row).collect();
        let mut col = 0;
        for span in line.spans() {
            let base = face_style(theme, span.style()).patch(row_style);
            // Split the span per grapheme so selection and match highlights
            // can start and end mid-span.
            for (start, grapheme) in grapheme_cells(span.text()) {
                let mut style = base;
                if matches.iter().any(|m| col >= m.start && col < m.end) {
                    style = style.patch(theme.search_match);
                }
                if selection.is_some_and(|s| s.contains(row, col)) {
                    style = style.patch(theme.selection);
                }
                spans.push(Span::styled(grapheme, style));
                col += display_width(grapheme);
                let _ = start;
            }
        }
        out.push(Line::from(spans).style(row_style));
    }
    out
}

fn grapheme_cells(text: &str) -> impl Iterator<Item = (usize, &str)> {
    // Splitting at char boundaries is enough for styling; combining marks
    // stay attached to the preceding cell in the terminal.
    text.char_indices()
        .map(|(i, ch)| (i, &text[i..i + ch.len_utf8()]))
}

fn status_line<'a>(
    view: &'a View,
    theme: &Theme,
    status: &StatusInfo<'a>,
    width: usize,
) -> Paragraph<'a> {
    let mode = view.mode();
    if matches!(mode, Mode::Command | Mode::Search { .. }) {
        let prompt = match mode {
            Mode::Command => ":",
            Mode::Search { backward: true } => "?",
            _ => "/",
        };
        let mut spans = vec![Span::raw(prompt), Span::raw(view.input())];
        if let Some(message) = view.message() {
            spans.push(Span::styled(format!("  {message}"), theme.marker));
        }
        return Paragraph::new(Line::from(spans)).style(theme.statusline);
    }
    let pill_style = match mode {
        Mode::Normal => theme.mode_normal,
        Mode::Select => theme.mode_select,
        _ => theme.mode_input,
    };
    let label = if view.source_view() && mode == Mode::Normal {
        "SRC".to_owned()
    } else {
        mode.to_string()
    };
    let (line, col) = view.source_position();
    let right = format!(" {line}:{col}  {}%  {} ", view.percent(), status.session);
    // Keep the right-hand block visible by trimming the path from the left.
    let fixed = display_width(&label) + 3 + display_width(&right) + 8;
    let path = truncate_left(status.path, width.saturating_sub(fixed));
    let mut left = vec![
        Span::styled(format!(" {label} "), pill_style),
        Span::raw(format!(" {path}")),
    ];
    if view.changed() {
        left.push(Span::styled(" [+]", theme.code));
    }
    if let Some(pending) = view.pending() {
        left.push(Span::styled(format!("  {pending}"), theme.marker));
    }
    if let Some(message) = view.message() {
        left.push(Span::styled(format!("  {message}"), theme.quote));
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
