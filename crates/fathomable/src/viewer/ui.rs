// @okf-doc: /decisions/0010-viewer-ux.md
//! Draw the view with ratatui: gutter, text, and the status line.

use fathomable_core::layout::{Face, Style as Face_, display_width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::view::{Mode, View};

/// Ratatui styles for the viewer chrome and Markdown faces.
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
            spans.push(Span::styled(format!("  {message}"), theme.info));
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
        left.push(Span::styled(" [+]", theme.info));
    }
    if let Some(pending) = view.pending() {
        left.push(Span::styled(format!("  {pending}"), theme.info));
    }
    if let Some(message) = view.message() {
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
