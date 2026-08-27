// @okf-doc: /decisions/0012-workspace-mode.md
//! Draw the app with ratatui: sidebar, gutter and text, popups, status line.

use std::path::Path;

use fathomable_core::layout::{Face, Style as Face_, display_width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use fathomable_core::annotations::Status;
use fathomable_core::diff::LineStatus;
use fathomable_core::status::Summary;

use super::threads::{Compose, ComposeTarget, MarkKind, ThreadPanel};
use super::view::{Mode, View};

use super::{App, Focus, HELP, JUMP_MENU, MAX_TOASTS, PickerState, Popup, SPACE_MENU};

/// Snippet lines quoted at the top of the thread panel.
pub(super) const SNIPPET_ROWS: usize = 3;
/// Cells a message body is indented under its author row.
const MESSAGE_INDENT: usize = 3;

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
    pub annotation_edited: Style,
    pub annotation_line: Style,
    pub diff_plus: Style,
    pub diff_delta: Style,
    pub diff_minus: Style,
    pub git_staged: Style,
    pub git_unstaged: Style,
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
            annotation_edited: style(Key::AnnotationEdited),
            annotation_line: style(Key::AnnotationLine),
            diff_plus: style(Key::DiffPlus),
            diff_delta: style(Key::DiffDelta),
            diff_minus: style(Key::DiffMinus),
            git_staged: style(Key::GitStaged),
            git_unstaged: style(Key::GitUnstaged),
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

/// Width of the gutter: the annotation cell, line numbers, a space, and
/// the diff bar cell (ADR 0006 order).
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
    // The text column: the view on top, the thread pane along the bottom.
    let column = Rect {
        x: area.x + sidebar_area.width,
        width: area.width.saturating_sub(sidebar_area.width),
        height: pane_height,
        ..area
    };
    // The comment box grows up from the status line, pushing the thread
    // pane up so a reply is written under the thread it answers.
    let box_rows = app.compose_rows().min(usize::from(column.height));
    let thread_rows = u16_of(app.thread_rows()).min(column.height.saturating_sub(u16_of(box_rows)));
    let text_area = Rect {
        height: column
            .height
            .saturating_sub(thread_rows)
            .saturating_sub(u16_of(box_rows)),
        ..column
    };
    let thread_area = Rect {
        y: text_area.y + text_area.height,
        height: thread_rows,
        ..column
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
    let text_rows = usize::from(text_area.height);
    if app.has_document() {
        frame.render_widget(
            Paragraph::new(text_lines(app, theme, gutter, text_rows)).style(theme.text),
            text_area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(welcome_lines(app, theme, text_area)).style(theme.text),
            text_area,
        );
    }
    if let Some(panel) = app.thread_panel() {
        draw_thread(frame, app, theme, thread_area, panel);
    }
    frame.render_widget(
        status_line(app, theme, usize::from(area.width)),
        status_area,
    );

    draw_toasts(frame, app, theme, text_area);
    match app.popup() {
        Some(Popup::Space) => draw_menu(frame, theme, column, &space_entries(&SPACE_MENU)),
        Some(Popup::Jump) => draw_menu(frame, theme, column, &space_entries(&JUMP_MENU)),
        Some(Popup::Help) => {
            let rows: Vec<(String, String)> = HELP
                .iter()
                .map(|(k, l)| ((*k).to_owned(), (*l).to_owned()))
                .collect();
            draw_table(frame, theme, area, " Keys (any key closes)", &rows);
        }
        Some(Popup::Status) => {
            draw_table(
                frame,
                theme,
                area,
                " Status (any key closes)",
                &app.status_lines(),
            );
        }
        Some(Popup::Picker(picker)) => {
            draw_picker(frame, theme, area, picker);
        }
        Some(Popup::Compose(compose)) => {
            draw_compose(frame, app, theme, column, compose, box_rows);
        }
        None => {
            if view.pending() == Some('g') {
                let entries = vec![
                    ("g".to_owned(), "go to top".to_owned()),
                    ("s".to_owned(), "toggle source view".to_owned()),
                    ("d".to_owned(), "toggle diff against HEAD".to_owned()),
                    ("D".to_owned(), "toggle diff against last seen".to_owned()),
                ];
                draw_menu(frame, theme, column, &entries);
            }
            place_cursor(frame, app, view, text_area, status_area, gutter);
        }
    }
}

/// What the text column shows before any file is open: the workspace,
/// the session, and the keys that get going, centred as a block.
fn welcome_lines<'a>(app: &App, theme: &Theme, area: Rect) -> Vec<Line<'a>> {
    let root = app.workspace().root().display().to_string();
    let entries: [(&str, String); 6] = [
        ("Space f", "open a file".to_owned()),
        ("Space e", "browse the tree".to_owned()),
        ("Space a", "read the thread under the cursor".to_owned()),
        ("Space ?", "list every key".to_owned()),
        (":q", "quit".to_owned()),
        ("", String::new()),
    ];
    let facts = [("workspace", root), ("viewer", app.viewer_label())];
    let key_width = entries
        .iter()
        .map(|(key, _)| display_width(key))
        .chain(facts.iter().map(|(label, _)| display_width(label)))
        .max()
        .unwrap_or(0);
    let width = usize::from(area.width);
    let block_width = entries
        .iter()
        .map(|(_, label)| key_width + 2 + display_width(label))
        .chain(
            facts
                .iter()
                .map(|(_, value)| key_width + 2 + display_width(value)),
        )
        .max()
        .unwrap_or(0)
        .min(width);
    let left = " ".repeat(width.saturating_sub(block_width) / 2);
    let mut body: Vec<Line<'a>> = vec![
        Line::from(Span::styled(
            format!("{left}Fathomable"),
            theme.heading[0].add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (label, value) in facts {
        body.push(Line::from(vec![
            Span::styled(format!("{left}{label:<key_width$}"), theme.info),
            Span::raw(format!(
                "  {}",
                truncate_left(&value, block_width.saturating_sub(key_width + 2))
            )),
        ]));
    }
    body.push(Line::from(""));
    for (key, label) in entries.iter().filter(|(key, _)| !key.is_empty()) {
        body.push(Line::from(vec![
            Span::styled(format!("{left}{key:<key_width$}"), theme.popup_key),
            Span::raw(format!("  {label}")),
        ]));
    }
    let top = usize::from(area.height).saturating_sub(body.len()) / 3;
    let mut out = vec![Line::from(""); top];
    out.extend(body);
    out
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
    } else if app.focus() != Focus::View || !app.has_document() {
        // The highlighted row is the cursor; leaving the terminal cursor
        // unset keeps it hidden rather than parked on the divider.
    } else {
        let cursor = view.cursor();
        let screen_row = cursor.row.saturating_sub(view.scroll());
        frame.set_cursor_position((
            text_area.x + u16_of(gutter + cursor.col),
            text_area.y + u16_of(screen_row),
        ));
    }
}

fn space_entries(menu: &[(char, &str)]) -> Vec<(String, String)> {
    menu.iter()
        .map(|(key, label)| (key.to_string(), (*label).to_owned()))
        .collect()
}

/// Change toasts, bottom-right above the status line, newest at the
/// bottom (ADR 0015).
fn draw_toasts(frame: &mut Frame<'_>, app: &App, theme: &Theme, pane: Rect) {
    let toasts = app.toasts();
    if toasts.is_empty() || pane.height == 0 {
        return;
    }
    let shown = toasts.iter().rev().take(MAX_TOASTS).rev();
    let lines: Vec<String> = shown.map(|t| format!(" {} ", t.text())).collect();
    let width = lines
        .iter()
        .map(|l| display_width(l))
        .max()
        .unwrap_or(0)
        .min(usize::from(pane.width));
    let height = u16_of(lines.len()).min(pane.height);
    let area = Rect {
        x: pane.x + pane.width - u16_of(width),
        y: pane.y + pane.height - height,
        width: u16_of(width),
        height,
    };
    let text: Vec<Line<'_>> = lines
        .iter()
        .rev()
        .take(usize::from(height))
        .rev()
        .map(|l| Line::from(Span::styled(fit(l, width), theme.popup)))
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(text).style(theme.popup), area);
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
    // The header carries the repo's summed `+n -m` (ADR 0017).
    let header_style = theme.sidebar_dir.add_modifier(Modifier::BOLD);
    // A sidebar too narrow for the whole name cuts it rather than spilling
    // over the divider.
    let title = fit(&format!(" {root}"), inner).trim_end().to_owned();
    let mut header_width = display_width(&title);
    let mut header = vec![Span::styled(title, header_style)];
    if let Some(total) = app.status().summary_under(Path::new("")) {
        let counts = [
            ('+', total.added, theme.diff_plus),
            ('-', total.removed, theme.diff_minus),
        ];
        for (sign, count, style) in counts {
            if count == 0 {
                continue;
            }
            let text = format!(" {sign}{count}");
            if header_width + display_width(&text) > inner {
                break;
            }
            header_width += display_width(&text);
            header.push(Span::styled(text, style));
        }
    }
    header.push(Span::styled(
        " ".repeat(inner.saturating_sub(header_width)),
        header_style,
    ));
    header.push(divider.clone());
    out.push(Line::from(header));
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
        // A queued change marks its file, and its collapsed ancestors so it
        // shows however the tree is folded (ADR 0015).
        let badge = if row.is_dir() {
            !row.expanded() && app.has_change_under(row.path())
        } else {
            app.queue().contains(row.path())
        };
        let (letter, mut tail) = sidebar_marks(app, row, theme, style, badge);
        // The marks follow the name directly, one space apart, and the
        // rest of the row is padded; a narrow sidebar drops the marks.
        let mut tail_width: usize = tail.iter().map(|span| span.content.chars().count()).sum();
        if tail_width == 0 || inner <= tail_width + 1 {
            tail.clear();
            tail_width = 0;
        }
        let name = fit(&text, inner - tail_width).trim_end().to_owned();
        // The git letter takes the gutter column ahead of the indent, which
        // is the name's leading space; a sidebar too narrow to hold any of
        // the name has no such column to take.
        let letter = letter.filter(|_| !name.is_empty());
        let used = display_width(&name) + tail_width;
        let mut spans = match letter {
            Some(letter) => vec![letter, Span::styled(name[1..].to_owned(), style)],
            None => vec![Span::styled(name, style)],
        };
        spans.extend(tail);
        spans.push(Span::styled(" ".repeat(inner.saturating_sub(used)), style));
        spans.push(divider.clone());
        out.push(Line::from(spans));
    }
    while out.len() < rows {
        out.push(Line::from(vec![
            Span::raw(" ".repeat(inner)),
            divider.clone(),
        ]));
    }
    out
}

/// The marks around a sidebar name: the git letter for the gutter column
/// (ADR 0017; files only, a folder's state is its children's), then the
/// counts and the follow badge (ADR 0015) that follow the name, each drawn
/// over the row's background.
fn sidebar_marks<'a>(
    app: &App,
    row: &fathomable_core::tree::Row,
    theme: &Theme,
    style: Style,
    badge: bool,
) -> (Option<Span<'a>>, Vec<Span<'a>>) {
    // A collapsed directory folds what is beneath it.
    let git = if row.is_dir() {
        (!row.expanded())
            .then(|| app.status().summary_under(row.path()))
            .flatten()
    } else {
        app.status().get(row.path()).map(|entry| Summary {
            state: entry.state(),
            staged: entry.is_staged(),
            added: entry.added(),
            removed: entry.removed(),
        })
    };
    let on_bg = |mark: Style| style.bg.map_or(mark, |bg| mark.bg(bg));
    let mut letter = None;
    let mut tail = Vec::new();
    if let Some(git) = git {
        let letter_style = if git.staged {
            theme.git_staged
        } else {
            theme.git_unstaged
        };
        if !row.is_dir() {
            letter = Some(Span::styled(
                git.state.letter().to_string(),
                on_bg(letter_style),
            ));
        }
        if git.added > 0 {
            tail.push(Span::styled(
                format!(" +{}", git.added),
                on_bg(theme.diff_plus),
            ));
        }
        if git.removed > 0 {
            tail.push(Span::styled(
                format!(" -{}", git.removed),
                on_bg(theme.diff_minus),
            ));
        }
    }
    if badge {
        tail.push(Span::styled(" ●", on_bg(theme.diff_delta)));
    }
    (letter, tail)
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
        Face::DiffAdded => theme.diff_plus,
        Face::DiffRemoved => theme.diff_minus,
        Face::DiffHeader => theme.diff_delta,
    };
    let mut style = base;
    if let Some(fg) = face.fg {
        style = style.fg(convert_color(fg));
    }
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

fn status_style(theme: &Theme, status: LineStatus) -> Style {
    match status {
        LineStatus::Added => theme.diff_plus,
        LineStatus::Modified => theme.diff_delta,
        LineStatus::Removed => theme.diff_minus,
    }
}

fn mark_style(theme: &Theme, kind: MarkKind) -> Style {
    match kind {
        MarkKind::Open => theme.annotation_open,
        MarkKind::Resolved => theme.annotation_resolved,
        MarkKind::AutoResolved => theme.annotation_auto,
        MarkKind::Detached => theme.annotation_detached,
        MarkKind::Edited => theme.annotation_edited,
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
        // Note cell (ADR 0013), number, space, diff bar (ADR 0006). A
        // removal has no line of its own, so it draws as a thin rule along
        // the top of the cell of the line below it.
        let bar = view
            .source_line_of_row(row)
            .and_then(|line| view.line_status(line))
            .map_or_else(
                || Span::styled(" ", row_style),
                |status| {
                    // A hunk the index already holds draws thicker
                    // (ADR 0017): `▌` staged, `▎` not yet.
                    let staged = view
                        .source_line_of_row(row)
                        .is_some_and(|line| view.line_staged(line));
                    let glyph = match (status, staged) {
                        (LineStatus::Removed, _) => "▔",
                        (_, true) => "▌",
                        (_, false) => "▎",
                    };
                    Span::styled(glyph, status_style(theme, status).patch(row_style))
                },
            );
        let note = mark.map_or_else(
            || Span::styled(" ", row_style),
            |kind| Span::styled("▎", mark_style(theme, kind).patch(row_style)),
        );
        let mut spans = vec![
            note,
            Span::styled(number, theme.line_number.patch(row_style)),
            Span::styled(" ", theme.marker.patch(row_style)),
            bar,
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
    } else if app.focus() == Focus::Thread {
        "THREAD".to_owned()
    } else if view.source_view() && mode == Mode::Normal {
        "SRC".to_owned()
    } else if view.diff_view() && mode == Mode::Normal {
        if view.diff_seen() {
            "DIFF seen".to_owned()
        } else {
            "DIFF".to_owned()
        }
    } else if app.auto_jump() && mode == Mode::Normal {
        "AUTO".to_owned()
    } else {
        mode.to_string()
    };
    let (line, col) = view.source_position();
    let threads = match app.thread_counts() {
        (_, 0) => String::new(),
        (open, total) => format!("{open}/{total} threads  "),
    };
    let followed = match app.followed().len() {
        0 => String::new(),
        n => format!("follow {n}  "),
    };
    let hint = change_hint(app);
    let changes = match view.diff_counts() {
        None | Some((0, 0)) => String::new(),
        Some((added, removed)) => format!("+{added} -{removed}  "),
    };
    let right = format!(
        " {line}:{col}  {}%  {changes}{threads}{followed}",
        view.percent()
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
    } else if let Some(hint) = hint {
        left.push(Span::styled(format!("  {hint}"), theme.diff_delta));
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

/// The newest queued change for the status line (ADR 0015), with its
/// counts when it is the open file.
fn change_hint(app: &App) -> Option<String> {
    let change = app.queue().newest()?;
    let counts = if app.current_path() == change.path {
        app.view().diff_counts()
    } else {
        None
    };
    let counts = match counts {
        Some((a, r)) if a + r > 0 => format!(" +{a} -{r}"),
        _ => String::new(),
    };
    Some(format!(
        "→ {}{counts} ({})",
        change.path.display(),
        app.queue().len()
    ))
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

/// A centred two-column popup with a title row: the key help and the
/// status overlay.
fn draw_table(
    frame: &mut Frame<'_>,
    theme: &Theme,
    area: Rect,
    title: &str,
    rows: &[(String, String)],
) {
    let key_width = rows
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(1);
    let lines: Vec<Line<'_>> = std::iter::once(Line::from(Span::styled(title, theme.popup_key)))
        .chain(rows.iter().map(|(key, label)| {
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

/// The comment box: grows up from the status line (ADR 0005, 0013), the
/// draft wrapped and scrolled so the cursor's row is visible (ADR 0018).
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
    let hint = if compose.confirming_discard() {
        "Esc again to discard · any key keeps the draft"
    } else if app.thread_panel().is_some() {
        "Ctrl/Alt-Enter submit · PgUp/PgDn thread · Ctrl-e $EDITOR · Esc"
    } else {
        "Enter newline · Ctrl/Alt-Enter submit · Ctrl-e $EDITOR · Esc"
    };
    let width = usize::from(pane.width);
    if rows < 3 {
        return;
    }
    let body_rows = rows - 2;
    let text_width = app.compose_width();
    let buffer = compose.buffer();
    let wrapped = buffer.rows(text_width);
    let first = app.compose_first_row();
    let mut lines = vec![
        rule_line(theme, width),
        header_line(
            theme,
            vec![Span::styled(title, theme.popup_key)],
            hint,
            width,
        ),
    ];
    for row in wrapped.iter().skip(first).take(body_rows) {
        let text = buffer.row_text(*row);
        lines.push(Line::from(Span::raw(fit(&format!(" {text}"), width))));
    }
    let area = Rect {
        x: pane.x,
        y: pane.y + pane.height - u16_of(rows),
        width: pane.width,
        height: u16_of(rows),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), area);
    let cell = buffer.cursor_cell(text_width);
    let col = (1 + cell.column).min(width.saturating_sub(1));
    let row = 2 + cell.row.saturating_sub(first).min(body_rows - 1);
    frame.set_cursor_position((area.x + u16_of(col), area.y + u16_of(row)));
}

/// The thread pane, filling `area`: rule, header, quoted snippet, comment,
/// replies. It is a pane, not a popup, so it draws on the text background.
fn draw_thread(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect, panel: &ThreadPanel) {
    let Some(thread) = app.thread(panel.id()) else {
        return;
    };
    let rows = usize::from(area.height);
    if rows < 3 {
        return;
    }
    let width = usize::from(area.width);
    let inner = width.saturating_sub(2);
    let now = super::threads::now();
    let mark = app.marks().iter().find(|m| m.id() == thread.id());
    let (index, total) = panel.position();
    let (status, status_style) = match (mark.map(super::threads::Mark::kind), thread.status()) {
        (Some(MarkKind::Detached), _) => ("detached", theme.annotation_detached),
        (Some(MarkKind::Edited), _) => ("edited", theme.annotation_edited),
        (_, Status::Open) => ("open", theme.annotation_open),
        (_, Status::Resolved) => ("resolved", theme.annotation_resolved),
        (_, Status::AutoResolved) => ("auto-resolved", theme.annotation_auto),
    };
    let range = mark.map_or_else(|| thread.range(), super::threads::Mark::range);
    let which = if total > 1 {
        format!(" thread {index}/{total}")
    } else {
        " thread".to_owned()
    };
    let left = vec![
        Span::styled(which, theme.popup_key),
        Span::styled(format!("  L{range}  "), theme.info),
        Span::styled(status.to_owned(), status_style),
    ];
    let hint = if app.focus() == Focus::Thread {
        "r reply · x resolve/reopen · n/p switch · j/k scroll · Esc close"
    } else {
        "click or Space a to focus"
    };
    let mut lines = vec![
        rule_line(theme, width),
        header_line(theme, left, hint, width),
    ];
    let mut body: Vec<Line<'_>> = Vec::new();
    let snippet: Vec<&str> = thread.snippet().lines().collect();
    let number_width = (range.start() + snippet.len()).to_string().len();
    for (offset, line) in snippet.iter().take(SNIPPET_ROWS).enumerate() {
        body.push(Line::from(Span::styled(
            fit(
                &format!(" {:>number_width$} │ {line}", range.start() + offset),
                width,
            ),
            theme.info,
        )));
    }
    if snippet.len() > SNIPPET_ROWS {
        body.push(Line::from(Span::styled(
            format!(" {:>number_width$} │ …", ""),
            theme.info,
        )));
    }
    body.push(Line::from(""));
    body.extend(message_lines(
        theme,
        "user",
        thread.created(),
        now,
        thread.comment(),
        None,
        inner,
    ));
    for reply in thread.replies() {
        body.push(Line::from(""));
        let badge = reply.proposes_resolution().then_some("proposes resolving");
        body.extend(message_lines(
            theme,
            reply.author().name(),
            reply.created(),
            now,
            reply.body(),
            badge,
            inner,
        ));
    }
    let body_rows = rows - 2;
    let scroll = panel.scroll().min(body.len().saturating_sub(body_rows));
    let below = body.len().saturating_sub(scroll + body_rows);
    let shown = if below > 0 { body_rows - 1 } else { body_rows };
    lines.extend(body.into_iter().skip(scroll).take(shown));
    if below > 0 {
        // The last body row becomes the indicator, so it hides one more.
        lines.push(Line::from(Span::styled(
            format!(" ▼ {} more", below + 1),
            theme.info,
        )));
    }
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(theme.text), area);
}

/// `author  time[tag]` then the wrapped body, indented one cell.
/// A `thread` panel message: author, age and an optional badge on one
/// row, the body indented beneath it.
fn message_lines<'a>(
    theme: &Theme,
    author: &str,
    created: u64,
    now: u64,
    body: &str,
    badge: Option<&str>,
    width: usize,
) -> Vec<Line<'a>> {
    let mut header = vec![
        Span::styled(format!(" {author}"), theme.popup_key),
        Span::styled(format!("  {}", format_age(created, now)), theme.info),
    ];
    if let Some(badge) = badge {
        header.push(Span::styled(format!("  [{badge}]"), theme.annotation_open));
    }
    let mut out = vec![Line::from(header)];
    for paragraph in body.lines() {
        for line in wrap(paragraph, width.saturating_sub(MESSAGE_INDENT).max(1)) {
            out.push(Line::from(Span::raw(format!(
                "{}{line}",
                " ".repeat(MESSAGE_INDENT)
            ))));
        }
    }
    out
}

/// A popup header: `left` spans, then `hint` right-aligned when it fits.
fn header_line<'a>(theme: &Theme, left: Vec<Span<'a>>, hint: &str, width: usize) -> Line<'a> {
    let used: usize = left.iter().map(|s| display_width(&s.content)).sum();
    let mut spans = left;
    let free = width.saturating_sub(used);
    let hint_width = display_width(hint) + 1;
    if free >= hint_width + 2 {
        spans.push(Span::raw(" ".repeat(free - hint_width)));
        spans.push(Span::styled(format!("{hint} "), theme.info));
    }
    Line::from(spans)
}

/// A full-width rule marking the top edge of a bottom-anchored popup.
fn rule_line<'a>(theme: &Theme, width: usize) -> Line<'a> {
    Line::from(Span::styled("─".repeat(width), theme.info))
}

/// `created` relative to `now` when recent, otherwise the UTC date.
fn format_age(created: u64, now: u64) -> String {
    let elapsed = now.saturating_sub(created);
    match elapsed {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", elapsed / 60),
        3600..86_400 => format!("{}h ago", elapsed / 3600),
        86_400..172_800 => {
            let stamp = format_time(created);
            format!("yesterday {}", &stamp[11..])
        }
        _ => format_time(created),
    }
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
    use super::{format_age, format_time, wrap};

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

    #[test]
    fn ages_recent_times_and_dates_old_ones() {
        let now = 1_700_000_000;
        assert_eq!(format_age(now, now), "just now");
        assert_eq!(format_age(now - 59, now), "just now");
        assert_eq!(format_age(now - 60, now), "1m ago");
        assert_eq!(format_age(now - 3_599, now), "59m ago");
        assert_eq!(format_age(now - 7_200, now), "2h ago");
        assert_eq!(format_age(now - 86_400, now), "yesterday 22:13");
        assert_eq!(format_age(now - 172_800, now), "2023-11-12 22:13");
        assert_eq!(format_age(now + 500, now), "just now");
    }
}
