// @okf-doc: /decisions/0012-workspace-mode.md
//! Draw the app with ratatui: sidebar, gutter and text, thread surfaces,
//! popups, and the status line; `gutter`, `info`, and `message` build the
//! rows the frame draws.

pub(crate) mod gutter;
pub(crate) mod header;
pub(crate) mod info;
pub(crate) mod message;

use std::fmt::Write as _;
use std::path::Path;

use fathomable_core::layout::{Face, Style as Face_, display_width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use fathomable_core::diff::LineStatus;
use fathomable_core::status::Summary;

use crate::app::draw::header::{
    Header, Tone, diff_header, draft_header, expanded_header, review_footer, review_header,
};
use crate::app::draw::info::Info;
use crate::app::draw::message::{MESSAGE_INDENT, expanded_lines, message_line};
use crate::app::threads::list::{Row, Rows};
use crate::app::threads::stubs::{Stub, Subject};
use crate::app::threads::words::label;
use crate::app::threads::{Compose, ThreadState};
use crate::app::view::{Mode, View};

use crate::app::input::bindings;
use crate::app::input::keys::place;
use crate::app::input::menu::{Grid, Menu};
use crate::app::{App, Focus, MAX_TOASTS, PickerState, Popup};

/// Ratatui styles for the chrome and Markdown faces.
///
/// Built from a resolved [`fathomable_core::theme::Theme`] (ADR 0011) so the
/// draw code never touches theme keys directly.
#[derive(Debug, Clone)]
pub(crate) struct Theme {
    pub(crate) text: Style,
    pub(crate) heading: [Style; 6],
    pub(crate) code: Style,
    pub(crate) code_block: Style,
    pub(crate) link: Style,
    pub(crate) marker: Style,
    pub(crate) quote: Style,
    pub(crate) line_number: Style,
    pub(crate) selection: Style,
    pub(crate) search_match: Style,
    pub(crate) statusline: Style,
    pub(crate) info: Style,
    /// The `deleted` banner (ADR 0028).
    pub(crate) warning: Style,
    /// `(c expand)` on a stub (ADR 0049).
    pub(crate) hint: Style,
    /// A pane's header rows and the review list's key bar (ADR 0059).
    pub(crate) header: Style,
    pub(crate) mode_normal: Style,
    pub(crate) mode_select: Style,
    pub(crate) mode_input: Style,
    pub(crate) sidebar: Style,
    pub(crate) sidebar_selected: Style,
    pub(crate) sidebar_dir: Style,
    pub(crate) popup: Style,
    pub(crate) popup_key: Style,
    /// The `Space` menu and the right-click menu (ADR 0056).
    pub(crate) menu: Style,
    pub(crate) picker_match: Style,
    pub(crate) picker_selected: Style,
    pub(crate) thread_open: Style,
    pub(crate) thread_resolved: Style,
    pub(crate) thread_waiting: Style,
    pub(crate) thread_line: Style,
    pub(crate) thread_focus: Style,
    /// A stub's background (ADR 0049).
    pub(crate) thread_inline: Style,
    pub(crate) diff_plus: Style,
    pub(crate) diff_delta: Style,
    pub(crate) diff_minus: Style,
    pub(crate) git_staged: Style,
    pub(crate) git_unstaged: Style,
}

impl Theme {
    /// Convert a resolved core theme into ratatui styles.
    pub(crate) fn from_core(theme: &fathomable_core::theme::Theme) -> Self {
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
            selection: style(Key::UiSelection),
            search_match: style(Key::UiSearchMatch),
            statusline: style(Key::UiStatusline),
            info: style(Key::UiStatuslineInfo),
            warning: style(Key::UiWarning),
            hint: style(Key::UiHint),
            header: style(Key::UiHeader),
            mode_normal: style(Key::UiStatuslineNormal),
            mode_select: style(Key::UiStatuslineSelect),
            mode_input: style(Key::UiStatuslineInput),
            sidebar: style(Key::UiSidebar),
            sidebar_selected: style(Key::UiSidebarSelected),
            sidebar_dir: style(Key::UiSidebarDir),
            popup: style(Key::UiPopup),
            popup_key: style(Key::UiPopupKey),
            menu: style(Key::UiMenu),
            picker_match: style(Key::UiPickerMatch),
            picker_selected: style(Key::UiPickerSelected),
            thread_open: style(Key::ThreadOpen),
            thread_resolved: style(Key::ThreadResolved),
            thread_waiting: style(Key::ThreadWaiting),
            thread_line: style(Key::ThreadLine),
            thread_focus: style(Key::ThreadFocus),
            thread_inline: style(Key::ThreadInline),
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
pub(crate) fn gutter_width(view: &View) -> usize {
    let digits = view.index().line_count().max(1).to_string().len();
    digits + 3
}

fn u16_of(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
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
    // The text column: the view, the draft written in its rows.
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

    draw_sidebar(frame, app, theme, sidebar_area);
    let text_area = draw_banner(frame, app, theme, text_area);
    let text_area = draw_diff_chrome(frame, app, theme, text_area);
    draw_column(frame, app, theme, text_area, gutter);
    frame.render_widget(
        status_line(app, theme, usize::from(area.width)),
        status_area,
    );

    draw_toasts(frame, app, theme, text_area);
    match app.popup() {
        Some(Popup::Help) => {
            let rows = bindings::help();
            let grid = help_grid(app, &rows);
            let hover = app
                .pointer()
                .and_then(|(column, row)| grid.entry_at(column, row));
            draw_table(frame, theme, grid, HELP_TITLE, &rows, hover);
        }
        Some(Popup::Status) => {
            let rows = app.status_lines();
            let grid = Grid::centred(&rows, STATUS_TITLE, 0, 0, app.size().0, app.pane_rows());
            draw_table(frame, theme, grid, STATUS_TITLE, &rows, None);
        }
        Some(Popup::Menu(menu)) => {
            draw_context_menu(frame, app, theme, menu);
        }
        Some(Popup::Picker(picker)) => {
            draw_picker(frame, theme, area, picker);
        }
        Some(Popup::Compose(_)) => {
            // The terminal cursor sits on the draft's cell in the text
            // (ADR 0054), when its row is on screen.
            if let Some((row, col)) = app.draft_cursor_cell()
                && let Some(screen_row) = row.checked_sub(view.scroll())
                && screen_row < usize::from(text_area.height)
            {
                frame.set_cursor_position((
                    text_area.x + u16_of(gutter + col),
                    text_area.y + u16_of(screen_row),
                ));
            }
        }
        None => {
            // A which-key menu for the keys typed so far (ADR 0045).
            if let Some(place) = place(app).filter(|_| !app.prefix().is_empty()) {
                let entries = bindings::menu(place, app.prefix());
                let grid = which_key_grid(app, &entries);
                let hover = app
                    .pointer()
                    .and_then(|(column, row)| grid.entry_at(column, row));
                draw_menu(
                    frame,
                    theme,
                    grid,
                    &bindings::menu_title(app.prefix()),
                    &entries,
                    hover,
                );
            }
            place_cursor(frame, app, view, text_area, status_area, gutter);
        }
    }
}

/// A deleted file keeps its content under a banner row in the warning
/// face (ADR 0028); the rest of the text area is returned.
fn draw_banner(frame: &mut Frame<'_>, app: &App, theme: &Theme, text_area: Rect) -> Rect {
    let Some(banner) = app.banner().filter(|_| text_area.height > 1) else {
        return text_area;
    };
    let banner_area = Rect {
        height: 1,
        ..text_area
    };
    frame.render_widget(
        Paragraph::new(format!(" {banner}")).style(theme.warning),
        banner_area,
    );
    Rect {
        y: text_area.y + 1,
        height: text_area.height - 1,
        ..text_area
    }
}

/// A diff's header over the text and, while the file has checkpoints,
/// the strip of them under it (ADR 0049, ADR 0060); the rows between
/// are returned.
fn draw_diff_chrome(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) -> Rect {
    let rows = app.diff_chrome_rows();
    let Some(header) = app
        .diff_header()
        .filter(|_| rows > 0 && usize::from(area.height) > rows)
    else {
        return area;
    };
    let width = usize::from(area.width);
    frame.render_widget(
        Paragraph::new(diff_header(&header).line(theme, width)).style(theme.info),
        Rect { height: 1, ..area },
    );
    let strip = app.checkpoint_strip();
    if rows < 2 || strip.is_empty() {
        return Rect {
            y: area.y + 1,
            height: area.height - 1,
            ..area
        };
    }
    let faint = theme.info.add_modifier(Modifier::DIM);
    let mut spans = vec![Span::styled(" checkpoints ", faint)];
    for (i, entry) in strip.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let glyph = if entry.workspace { "◆" } else { "·" };
        let style = if entry.shown {
            theme.popup_key
        } else {
            theme.info
        };
        spans.push(Span::styled(format!("{glyph} {}", entry.label), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme.info),
        Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        },
    );
    Rect {
        y: area.y + 1,
        height: area.height - 2,
        ..area
    }
}

/// The text column: the review list when it is open (ADR 0025), else the
/// file-info pane for a binary or over-limit file (ADR 0026), else the
/// document, else the welcome block.
fn draw_column(frame: &mut Frame<'_>, app: &App, theme: &Theme, text_area: Rect, gutter: usize) {
    let text_rows = usize::from(text_area.height);
    if app.review_list().is_open() {
        draw_review(frame, app, theme, text_area);
    } else if let Some(info) = app.info() {
        draw_info(frame, app, theme, text_area, &info);
    } else if app.has_document() {
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
}

/// What the text column shows before any file is open: the workspace,
/// the session, and the keys that get going, centred as a block.
fn welcome_lines<'a>(app: &App, theme: &Theme, area: Rect) -> Vec<Line<'a>> {
    let root = app.workspace().root().display().to_string();
    let entries: [(&str, String); 6] = [
        ("Space f", "open a file".to_owned()),
        ("Space w h", "browse the files".to_owned()),
        ("Space r", "review the threads".to_owned()),
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
    } else if app.focus() != Focus::View
        || !app.has_document()
        || app.review_list().is_open()
        || app.info().is_some()
    {
        // The highlighted row is the cursor; leaving the terminal cursor
        // unset keeps it hidden rather than parked on the divider.
    } else {
        let screen_row = view.cursor().row.saturating_sub(view.scroll());
        frame.set_cursor_position((
            text_area.x + u16_of(gutter + view.cursor().col),
            text_area.y + u16_of(screen_row),
        ));
    }
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

fn tree_lines<'a>(
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
    let focused = app.focus() == Focus::Tree;
    for (index, row) in tree
        .rows()
        .iter()
        .enumerate()
        .skip(app.tree_scroll())
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
        let (letter, mut tail) = tree_marks(app, row, theme, style, badge);
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

/// The marks around a files pane name: the git letter for the gutter column
/// (ADR 0017; files only, a folder's state is its children's), then the
/// counts and the follow badge (ADR 0015) that follow the name, each drawn
/// over the row's background.
fn tree_marks<'a>(
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
            binary: entry.is_binary(),
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
        // A dirty binary file has no counts; the tag says why (ADR 0026).
        if git.binary {
            tail.push(Span::styled(" bin", on_bg(theme.info)));
        }
    }
    if badge {
        tail.push(Span::styled(" ●", on_bg(theme.diff_delta)));
    }
    // A file with a thread waiting on the user (ADR 0030).
    if !row.is_dir() && app.path_waits(row.path()) {
        tail.push(Span::styled(" ↩", on_bg(theme.thread_waiting)));
    }
    (letter, tail)
}

/// The sidebar (ADR 0049): the files pane on top, the threads pane along
/// the bottom, either one alone when the other is hidden.
fn draw_sidebar(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    if area.width == 0 {
        return;
    }
    let width = usize::from(area.width);
    let tree_area = Rect {
        height: u16_of(app.tree_rows()).min(area.height),
        ..area
    };
    let pane_area = Rect {
        y: tree_area.y + tree_area.height,
        height: area.height.saturating_sub(tree_area.height),
        ..area
    };
    if let Some(tree) = app.tree()
        && tree_area.height > 0
    {
        frame.render_widget(
            Paragraph::new(tree_lines(
                app,
                tree,
                theme,
                width,
                usize::from(tree_area.height),
            ))
            .style(theme.sidebar),
            tree_area,
        );
    }
    if pane_area.height > 0 && app.threads_pane_shown() {
        frame.render_widget(
            Paragraph::new(threads_pane_lines(
                app,
                theme,
                width,
                usize::from(pane_area.height),
            ))
            .style(theme.sidebar),
            pane_area,
        );
    }
}

/// The threads pane (ADR 0027, ADR 0049): a rule, a header with the
/// scope and count and the `s x` keys at the edge, then one row per
/// thread — status glyph, place, first line of the newest message, and
/// the reply count and age at the right edge when the column has room.
fn threads_pane_lines<'a>(app: &App, theme: &Theme, width: usize, rows: usize) -> Vec<Line<'a>> {
    let inner = width.saturating_sub(1);
    let divider = Span::styled("│", theme.marker);
    let with_divider = |spans: Vec<Span<'a>>| {
        let mut spans = spans;
        spans.push(divider.clone());
        Line::from(spans)
    };
    let mut out = Vec::with_capacity(rows);
    out.push(with_divider(vec![Span::styled(
        "─".repeat(inner),
        theme.info,
    )]));
    let scope = app.sidebar_scope();
    let entries = app.threads_pane_rows();
    // The title row is a bar on `ui.header` (ADR 0059).
    let header_style = theme
        .sidebar_dir
        .add_modifier(Modifier::BOLD)
        .patch(theme.header);
    let title = format!(" threads · {} {}", scope.word(), entries.len());
    let keys = "s x ";
    let mut header = Vec::new();
    if inner > display_width(&title) + display_width(keys) {
        header.push(Span::styled(
            fit(&title, inner - display_width(keys)),
            header_style,
        ));
        header.push(Span::styled(
            keys.to_owned(),
            theme.info.patch(theme.header),
        ));
    } else {
        header.push(Span::styled(fit(&title, inner), header_style));
    }
    out.push(with_divider(header));
    if entries.is_empty() {
        let empty = match scope {
            crate::app::threads::pane::PaneScope::File => " no threads in this file",
            crate::app::threads::pane::PaneScope::Workspace => " no threads in the workspace",
        };
        out.push(with_divider(vec![Span::styled(
            fit(empty, inner),
            theme.info,
        )]));
    }
    let focused = app.focus() == Focus::ThreadsPane;
    let selected = app.threads_pane_selected();
    let now = fathomable_core::clock::now();
    for (index, row) in entries
        .iter()
        .enumerate()
        .skip(app.threads_pane_scroll())
        .take(rows.saturating_sub(2))
    {
        let mut style = theme.sidebar;
        if selected == Some(index) {
            style = style.patch(theme.sidebar_selected);
            if !focused {
                style = style.remove_modifier(Modifier::BOLD);
            }
        }
        let on_bg = |mark: Style| style.bg.map_or(mark, |bg| mark.bg(bg));
        let resolved = row.words().is_resolved();
        let glyph = if resolved { "✓" } else { "●" };
        let place = format!(" {} ", row.place(scope));
        let age = format_age_short(row.updated(), now);
        let tail = if row.replies() > 0 {
            format!(" ↩{} {age}", row.replies())
        } else {
            format!(" {age}")
        };
        let lead_width = 1 + display_width(glyph) + display_width(&place);
        // The tail goes first when the column is narrow; the summary
        // takes what is left, however little.
        let tail_width = display_width(&tail);
        let keep_tail = inner >= lead_width + tail_width + 4;
        let summary_width = inner
            .saturating_sub(lead_width)
            .saturating_sub(if keep_tail { tail_width } else { 0 });
        let summary = fit(row.summary(), summary_width);
        let mut spans = vec![
            Span::styled(" ", style),
            Span::styled(glyph, on_bg(mark_style(theme, row.kind())).patch(style)),
            Span::styled(place, style),
            Span::styled(summary, if resolved { on_bg(theme.info) } else { style }),
        ];
        if keep_tail {
            spans.push(Span::styled(tail, on_bg(theme.info)));
        }
        out.push(with_divider(spans));
    }
    while out.len() < rows {
        out.push(with_divider(vec![Span::styled(
            " ".repeat(inner),
            theme.sidebar,
        )]));
    }
    out
}

/// Pad or truncate `text` to exactly `width` cells.
pub(super) fn fit(text: &str, width: usize) -> String {
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

pub(super) fn face_style(theme: &Theme, face: &Face_) -> Style {
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

fn mark_style(theme: &Theme, kind: ThreadState) -> Style {
    match kind {
        ThreadState::Open => theme.thread_open,
        ThreadState::Resolved => theme.thread_resolved,
        ThreadState::Waiting => theme.thread_waiting,
    }
}

fn text_lines<'a>(app: &'a App, theme: &Theme, gutter: usize, rows: usize) -> Vec<Line<'a>> {
    let view = app.view();
    let digits = gutter - 3;
    let selection = view.selection();
    let lines = view.layout().lines();
    let width = gutter + view.layout().width();
    let mut expanded: std::collections::HashMap<usize, Vec<Line<'a>>> =
        std::collections::HashMap::new();
    let mut out = Vec::with_capacity(rows);
    for (row, line) in lines.iter().enumerate().skip(view.scroll()).take(rows) {
        // A stub row says what a thread said, under its lines; an
        // expanded thread's rows show the whole of it (ADR 0049).
        if let Some((stub, index, last)) = app.stub_on_row(row) {
            if stub.expanded() {
                let block = view.stub_slot_of_row(row).map_or(0, |(block, _)| block);
                let body = expanded
                    .entry(block)
                    .or_insert_with(|| expanded_block_lines(app, theme, &stub, width - gutter));
                out.push(with_gutter(
                    app,
                    theme,
                    body.get(index).cloned().unwrap_or_default(),
                    row,
                    digits,
                ));
            } else {
                out.push(stub_line(
                    app, theme, &stub, index, last, row, gutter, width,
                ));
            }
            continue;
        }
        // The note cell brackets a thread's rows (ADR 0027).
        let note = app.note_on_row(row);
        let mut row_style = Style::default();
        if note.is_some() {
            row_style = row_style.patch(theme.thread_line);
        }
        // The open thread's own lines stand out from the rest (ADR 0033).
        if app.open_thread_on_row(row) {
            row_style = row_style.patch(theme.thread_focus);
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
        let note = note.map_or_else(
            || Span::styled(" ", row_style),
            |(glyph, kind)| Span::styled(glyph, mark_style(theme, kind).patch(row_style)),
        );
        let mut spans = vec![
            note,
            Span::styled(number, theme.line_number.patch(row_style)),
            Span::styled(" ", theme.marker.patch(row_style)),
            bar,
        ];
        let matches: Vec<_> = view.matches().iter().filter(|m| m.row == row).collect();
        let styled = |col: usize, base: Style| {
            let mut style = base;
            if matches.iter().any(|m| col >= m.start && col < m.end) {
                style = style.patch(theme.search_match);
            }
            if selection.is_some_and(|s| s.contains(row, col)) {
                style = style.patch(theme.selection);
            }
            style
        };
        let mut col = 0;
        for span in line.spans() {
            let base = face_style(theme, span.style()).patch(row_style);
            // Split the span per character so selection and match highlights
            // can start and end mid-span.
            for grapheme in grapheme_cells(span.text()) {
                spans.push(Span::styled(grapheme, styled(col, base)));
                col += display_width(grapheme);
            }
        }
        out.push(Line::from(spans).style(row_style));
    }
    if out.len() < rows && view.scroll() + out.len() == lines.len() {
        out.push(past_end_line(theme, digits));
    }
    out
}

/// The one row past the end that the view may scroll to: a `~` in the
/// number column and nothing else, as Helix draws it.
fn past_end_line<'a>(theme: &Theme, digits: usize) -> Line<'a> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled(format!("{:>digits$}", "~"), theme.line_number),
    ])
}

/// One row of a collapsed stub (ADR 0049): the gutter's bracket if an
/// outer thread spans the row, then the state glyph, the author, the
/// age, and the first line of the message, on the `thread.inline`
/// background — or behind a `▎` in the state colour when the theme sets
/// none. The thread under the cursor reads in the text colour, the
/// others dimmed; the thread cursor's last row ends with `(c expand)`.
#[expect(clippy::too_many_arguments, reason = "one row's facts, read once each")]
fn stub_line<'a>(
    app: &App,
    theme: &Theme,
    stub: &Stub,
    message: usize,
    last: bool,
    row: usize,
    gutter: usize,
    width: usize,
) -> Line<'a> {
    let digits = gutter - 3;
    let Some(thread) = stub.thread().and_then(|id| app.thread(id)) else {
        return Line::from("");
    };
    let kind = app
        .marks()
        .iter()
        .find(|mark| mark.id() == thread.id())
        .map_or(ThreadState::Open, crate::app::threads::Mark::kind);
    let (author, created, body) = match message.checked_sub(1) {
        None => (
            author_label(thread.author(), app.user_name()),
            thread.created(),
            thread.comment(),
        ),
        Some(index) => {
            let reply = &thread.replies()[index];
            (
                author_label(reply.author(), app.user_name()),
                reply.created(),
                reply.body(),
            )
        }
    };
    let covered = app.threads_at_cursor().contains(thread.id());
    let hinted = last && covered && app.thread_cursor().thread() == Some(thread.id());
    let row_style = theme.thread_inline;
    let text_style = if covered {
        theme
            .text
            .patch(Style::default().fg(theme.thread_focus.fg.unwrap_or_default()))
    } else {
        theme.info
    }
    .patch(row_style);
    let text_style = if theme.thread_focus.fg.is_none() && covered {
        theme.text.patch(row_style)
    } else {
        text_style
    };
    let note = app.note_on_row(row).map_or_else(
        || Span::styled(" ", row_style),
        |(glyph, kind)| Span::styled(glyph, mark_style(theme, kind).patch(row_style)),
    );
    let edge = if row_style.bg.is_none() {
        Span::styled("▎", mark_style(theme, kind))
    } else {
        Span::styled(" ", row_style)
    };
    let now = fathomable_core::clock::now();
    let lead = format!(" {author} ");
    // The age in the info colour, as every other row gives it (ADR 0059).
    let age = format!("{}  ", format_age_short(created, now));
    let hint = if hinted { " (c expand)" } else { "" };
    let free = width
        .saturating_sub(gutter)
        .saturating_sub(1)
        .saturating_sub(1 + display_width(&lead) + display_width(&age))
        .saturating_sub(display_width(hint));
    let first = body.lines().next().unwrap_or("");
    let text = if display_width(first) > free && free > 0 {
        let mut shortened: String = first.chars().collect();
        while display_width(&shortened) + 1 > free && !shortened.is_empty() {
            shortened.pop();
        }
        fit(&format!("{shortened}…"), free)
    } else {
        fit(first, free)
    };
    let spans = vec![
        note,
        Span::styled(" ".repeat(digits), row_style),
        Span::styled(" ", row_style),
        Span::styled(" ", row_style),
        edge,
        Span::styled("●".to_owned(), mark_style(theme, kind).patch(row_style)),
        Span::styled(lead, theme.popup_key.patch(row_style)),
        Span::styled(age, theme.info.patch(row_style)),
        Span::styled(text, text_style),
        Span::styled(hint.to_owned(), theme.hint.patch(row_style)),
    ];
    Line::from(spans).style(row_style)
}

/// The rows of `stub`'s block expanded in place (ADR 0049), at the text
/// width: a header with the state, placement, and watchers on the left
/// and the keys on the right, then every message as the pane drew them,
/// the draft in its place among them (ADR 0054); for a draft block, a
/// header naming the lines and the draft.
fn expanded_block_lines<'a>(app: &App, theme: &Theme, stub: &Stub, width: usize) -> Vec<Line<'a>> {
    let draft = app
        .draft()
        .map(|compose| draft_lines(app, theme, compose, width));
    let mut lines = match stub.subject() {
        Subject::Draft(range) => vec![
            Header::new(
                vec![(format!(" comment on L{range}"), Tone::Key)],
                Vec::new(),
            )
            .line(theme, width),
        ],
        Subject::FileDraft => vec![
            Header::new(
                vec![(
                    format!(" comment on {}", app.current_path().display()),
                    Tone::Key,
                )],
                Vec::new(),
            )
            .line(theme, width),
        ],
        Subject::Thread(id) => {
            let Some(thread) = app.thread(id) else {
                return Vec::new();
            };
            let cursor = app.thread_cursor();
            let selected = (cursor.thread() == Some(id)).then_some(cursor.message());
            let mut lines = vec![expanded_header(app, thread).line(theme, width)];
            lines.extend(expanded_lines(
                theme,
                app.highlighter(),
                thread,
                app.user_name(),
                fathomable_core::clock::now(),
                width,
                selected,
            ));
            lines
        }
    };
    if let (Some((at, replaces)), Some(draft)) = (stub.draft_slot(), draft) {
        let end = (at + replaces).min(lines.len());
        lines.splice(at.min(lines.len())..end, draft);
    }
    lines
}

/// The draft's rows (ADR 0054): the author row with the draft keys, then
/// the text wrapped at the draft's width and indented as a body is.
fn draft_lines<'a>(app: &App, theme: &Theme, compose: &Compose, width: usize) -> Vec<Line<'a>> {
    let mut lines = vec![draft_header(compose).line(theme, width)];
    let buffer = compose.buffer();
    let text_width = app.draft_width();
    let indent = " ".repeat(MESSAGE_INDENT);
    let rows = buffer.rows(text_width);
    if rows.is_empty() {
        lines.push(message_line(theme, vec![Span::raw(indent)], width, false));
        return lines;
    }
    for row in rows {
        let spans = vec![
            Span::raw(indent.clone()),
            Span::styled(buffer.row_text(row).to_owned(), theme.text),
        ];
        lines.push(message_line(theme, spans, width, false));
    }
    lines
}

/// An expanded thread's row with the gutter in front of it: the bracket
/// of a thread spanning the row, blanks for the number and the bar, on
/// the `thread.inline` background.
fn with_gutter<'a>(
    app: &App,
    theme: &Theme,
    line: Line<'a>,
    row: usize,
    digits: usize,
) -> Line<'a> {
    let row_style = theme.thread_inline;
    let note = app.note_on_row(row).map_or_else(
        || Span::styled(" ", row_style),
        |(glyph, kind)| Span::styled(glyph, mark_style(theme, kind).patch(row_style)),
    );
    let mut spans = vec![
        note,
        Span::styled(" ".repeat(digits), row_style),
        Span::styled(" ", row_style),
        Span::styled(" ", row_style),
    ];
    let inner = line.style;
    spans.extend(line.spans);
    Line::from(spans).style(row_style.patch(inner))
}

/// How a message's author reads on a stub: the user by the configured
/// name (ADR 0058), an agent as `name (type)` when it subscribed with a
/// type.
fn author_label(author: &fathomable_core::annotations::Author, user: &str) -> String {
    use fathomable_core::annotations::Author;
    match author {
        Author::User => user.to_owned(),
        Author::Agent {
            name,
            kind: Some(kind),
            ..
        } => format!("{name} ({kind})"),
        Author::Agent { name, .. } => name.clone(),
    }
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
    let parts = status_parts(app);
    let hint = change_hint(app);
    // Keep the right-hand block visible by trimming the path from the left.
    let badges: usize = parts.badges.iter().map(|b| display_width(b) + 2).sum();
    let fixed = display_width(parts.pill) + 3 + badges + display_width(&parts.right) + 8;
    let path = app.current_path().to_string_lossy();
    let path = truncate_left(&path, width.saturating_sub(fixed));
    let mut left = vec![
        Span::styled(format!(" {} ", parts.pill), pill_style),
        Span::raw(format!(" {path}")),
    ];
    if view.changed() {
        left.push(Span::styled(" [+]", theme.info));
    }
    for badge in parts.badges {
        left.push(Span::styled(format!("  {badge}"), theme.info));
    }
    if !app.prefix().is_empty() {
        left.push(Span::styled(
            format!("  {}", bindings::spell(app.prefix())),
            theme.info,
        ));
    }
    if let Some(message) = app.message() {
        left.push(Span::styled(format!("  {message}"), theme.info));
    } else if let Some(hint) = hint {
        left.push(Span::styled(format!("  {hint}"), theme.diff_delta));
    }
    let right = parts.right;
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

/// The status line's words (ADR 0010, amended by 0046's session): the
/// pill says one thing, the mode or the focused pane; the badges after
/// the path say how the text is shown (`SRC`, or `DIFF` and the base,
/// ADR 0060) and whether auto-jump is on; the
/// right block is `line:col`, the percentage, and `N word` counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StatusParts {
    pub(crate) pill: &'static str,
    pub(crate) badges: Vec<String>,
    pub(crate) right: String,
}

pub(super) fn status_parts(app: &App) -> StatusParts {
    let view = app.view();
    let pill = match app.focus() {
        Focus::Tree => "FILES",
        Focus::Review => "REVIEW",
        Focus::ThreadsPane => "THREADS",
        Focus::View => match view.mode() {
            Mode::Normal => "NOR",
            Mode::Select => "SEL",
            Mode::Command => "CMD",
            Mode::Search { .. } => "SRCH",
        },
    };
    let mut badges = Vec::new();
    if let Some(diff) = view.diff() {
        badges.push(diff.badge.clone());
    } else if view.source_view() {
        badges.push("SRC".to_owned());
    }
    if app.auto_jump() {
        badges.push("AUTO".to_owned());
    }
    let (line, col) = view.source_position();
    let mut right = format!(" {line}:{col}  {}%", view.percent());
    let counts = if view.diff_view() {
        view.pair_counts()
    } else {
        view.diff_counts()
    };
    if let Some((added, removed)) = counts.filter(|(a, r)| a + r > 0) {
        // Infallible: writing to a `String` cannot fail.
        let _ = write!(right, "  +{added} -{removed}");
    }
    let counts = [
        (app.proposed_count(), "proposed"),
        (app.waiting_count(), "waiting"),
        (app.thread_counts().1, "threads"),
    ];
    for (count, word) in counts {
        if count > 0 {
            let _ = write!(right, "  {count} {word}");
        }
    }
    right.push_str("  ");
    StatusParts {
        pill,
        badges,
        right,
    }
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

/// A Helix-style key menu anchored to the bottom of `pane`, under a
/// breadcrumb row naming the prefix (ADR 0049), laid out in columns when
/// the entries do not fit in the rows available.
/// The which-key menu's grid along the bottom of the text column, the
/// one the mouse reads back (ADR 0050).
pub(crate) fn which_key_grid(app: &App, entries: &[(String, String)]) -> Grid {
    Grid::bottom(
        entries,
        &bindings::menu_title(app.prefix()),
        app.sidebar_width(),
        0,
        app.column_width(),
        app.pane_rows(),
    )
}

/// The help popup's grid (ADR 0050).
pub(crate) fn help_grid(app: &App, rows: &[(String, String)]) -> Grid {
    Grid::centred(rows, HELP_TITLE, 0, 0, app.size().0, app.pane_rows())
}

pub(crate) const HELP_TITLE: &str = " Keys (any key closes)";
const STATUS_TITLE: &str = " Status (any key closes)";

/// A Helix-style key menu in `grid` under a breadcrumb row naming the
/// prefix (ADR 0049); `hover` is the entry under the pointer.
fn draw_menu(
    frame: &mut Frame<'_>,
    theme: &Theme,
    grid: Grid,
    title: &str,
    entries: &[(String, String)],
    hover: Option<usize>,
) {
    if grid.height < 2 || entries.is_empty() {
        return;
    }
    let key_width = grid.key_width;
    let label_width = grid.label_width;
    let mut lines = Vec::with_capacity(grid.rows + 1);
    lines.push(Line::from(Span::styled(
        format!(" {title} "),
        theme.mode_normal,
    )));
    for r in 0..grid.rows {
        let mut spans = vec![Span::raw(" ")];
        for c in 0..grid.columns {
            let index = c * grid.rows + r;
            let Some((key, label)) = entries.get(index) else {
                break;
            };
            let row_style = if hover == Some(index) {
                theme.picker_selected
            } else {
                Style::default()
            };
            spans.push(Span::styled(
                format!("{key:>key_width$}"),
                theme.popup_key.patch(row_style),
            ));
            spans.push(Span::styled(
                format!("  {label:<label_width$}   "),
                row_style,
            ));
        }
        lines.push(Line::from(spans));
    }
    let area = grid_rect(grid);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(theme.menu), area);
}

fn grid_rect(grid: Grid) -> Rect {
    Rect {
        x: u16_of(grid.x),
        y: u16_of(grid.y),
        width: u16_of(grid.width),
        height: u16_of(grid.height),
    }
}

/// The context menu at the pointer (ADR 0050): a title row in the pill
/// colour naming what it acts on, then `key  label` rows, the one under
/// the pointer highlighted.
fn draw_context_menu(frame: &mut Frame<'_>, app: &App, theme: &Theme, menu: &Menu) {
    let (width, height) = app.size();
    let grid = menu.grid(width, height);
    let entries: Vec<(String, String)> = menu
        .entries()
        .iter()
        .map(|entry| (entry.key().to_owned(), entry.label().to_owned()))
        .collect();
    let hover = app
        .pointer()
        .and_then(|(column, row)| grid.entry_at(column, row));
    draw_menu(frame, theme, grid, menu.title(), &entries, hover);
}

/// A centred two-column popup with a title row: the key help and the
/// status overlay.
fn draw_table(
    frame: &mut Frame<'_>,
    theme: &Theme,
    grid: Grid,
    title: &str,
    rows: &[(String, String)],
    hover: Option<usize>,
) {
    let key_width = grid.key_width;
    let label_width = grid.label_width;
    // Rows that do not fit under the title flow into further columns, so
    // a long table (`Space ?`) is read like a menu, not cut off.
    let mut lines: Vec<Line<'_>> = vec![Line::from(Span::styled(title, theme.popup_key))];
    for r in 0..grid.rows.min(rows.len()) {
        let mut spans = Vec::new();
        for col in 0..grid.columns {
            let index = col * grid.rows + r;
            let Some((key, label)) = rows.get(index) else {
                break;
            };
            let row_style = if hover == Some(index) {
                theme.picker_selected
            } else {
                Style::default()
            };
            let gap = if col == 0 { " " } else { "   " };
            spans.push(Span::styled(
                format!("{gap}{key:<key_width$}"),
                theme.popup_key.patch(row_style),
            ));
            spans.push(Span::styled(format!("  {label:<label_width$}"), row_style));
        }
        lines.push(Line::from(spans));
    }
    let popup = grid_rect(grid);
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
        super::PickerKind::Wake => "wake",
        super::PickerKind::DiffBase => "base",
        super::PickerKind::DiffTarget => "target",
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

/// The file-info pane (ADR 0026): the path as a header, the labelled
/// rows with their labels right-aligned, then the notice.
fn draw_info(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect, info: &Info) {
    let width = usize::from(area.width);
    let label_width = info
        .rows
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0);
    let header = vec![Span::styled(
        format!(" {}", app.current_path().display()),
        theme.popup_key,
    )];
    // The path row is the pane's header, on `ui.header` (ADR 0059).
    let mut lines = vec![
        padded_line(header, width).style(theme.header),
        Line::default(),
    ];
    for (label, value) in &info.rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {label:>label_width$}"), theme.popup_key),
            Span::raw(format!("  {value}")),
        ]));
    }
    lines.push(Line::default());
    for line in &info.notice {
        lines.push(Line::from(Span::styled(format!("  {line}"), theme.info)));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_review(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    let width = usize::from(area.width);
    let rows = usize::from(area.height);
    if rows == 0 {
        return;
    }
    let list = app.review_list();
    let Rows { rows: all, entries } = app.review_rows(width);
    let now = fathomable_core::clock::now();
    // The header, the entries between, and the key bar on the last row
    // (ADR 0059); one row shows the header alone.
    let mut lines = vec![review_header(app, &entries).line(theme, width)];
    let body = rows.saturating_sub(2);
    let scroll = list.scroll().min(all.len().saturating_sub(body));
    for row in all.iter().skip(scroll).take(body) {
        lines.push(list_row(theme, row, now, width));
    }
    if rows >= 2 {
        lines.resize_with(rows - 1, Line::default);
        lines.push(review_footer(app, &entries).line(theme, width));
    }
    frame.render_widget(Paragraph::new(lines).style(theme.text), area);
}

fn list_row<'a>(theme: &Theme, row: &Row, now: u64, width: usize) -> Line<'a> {
    match row {
        Row::Header {
            path,
            range,
            kind,
            proposed,
            updated,
            selected,
            folded,
            dim,
            ..
        } => {
            let (status, status_style) = (label(*kind), mark_style(theme, *kind));
            // The third word (ADR 0053), in the state's colour.
            let status = if *proposed {
                format!("{status} · proposed")
            } else {
                status.to_owned()
            };
            let fold = if *folded { "  ▸" } else { "" };
            // Every header carries the path (ADR 0049), so either order
            // reads on its own.
            let spans = vec![
                Span::styled(
                    format!(" {}", path.display()),
                    if *dim { theme.info } else { theme.heading[2] },
                ),
                Span::styled(
                    // A thread on the file as a whole says so (ADR 0063).
                    range.map_or_else(|| "  file  ".to_owned(), |range| format!("  L{range}  ")),
                    if *dim { theme.info } else { theme.text },
                ),
                Span::styled(status, status_style),
                Span::styled(format!("  {}{fold}", format_age(*updated, now)), theme.info),
            ];
            // The row is a bar on `ui.header` (ADR 0059); the selection
            // colour wins on the selected entry.
            let style = if *selected {
                theme.picker_selected
            } else {
                theme.header
            };
            padded_line(spans, width).style(style)
        }
        Row::Message {
            author,
            created,
            badge,
            dim,
            selected,
            ..
        } => {
            let mut spans = vec![
                Span::styled(
                    format!("   {author}"),
                    if *dim { theme.info } else { theme.popup_key },
                ),
                Span::styled(format!("  {}", format_age(*created, now)), theme.info),
            ];
            if let Some(badge) = badge {
                spans.push(Span::styled(format!("  [{badge}]"), theme.thread_open));
            }
            list_selection_line(theme, spans, width, *selected)
        }
        Row::Body {
            text,
            dim,
            selected,
            ..
        } => list_selection_line(
            theme,
            vec![Span::styled(
                text.clone(),
                if *dim { theme.info } else { theme.text },
            )],
            width,
            *selected,
        ),
        Row::Blank => Line::from(""),
    }
}

fn list_selection_line<'a>(
    theme: &Theme,
    spans: Vec<Span<'a>>,
    width: usize,
    selected: bool,
) -> Line<'a> {
    if selected {
        padded_line(spans, width).style(theme.picker_selected)
    } else {
        Line::from(spans)
    }
}

/// `spans` padded to `width` cells so a row's background reaches the
/// right edge.
fn padded_line(mut spans: Vec<Span<'_>>, width: usize) -> Line<'_> {
    let used: usize = spans.iter().map(|span| display_width(&span.content)).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans)
}

/// `created` relative to `now` when recent, otherwise the UTC date.
pub(super) fn format_age(created: u64, now: u64) -> String {
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

/// `created` relative to `now` in the fewest cells: `now`, `5m`, `2h`, `3d`.
fn format_age_short(created: u64, now: u64) -> String {
    let elapsed = now.saturating_sub(created);
    match elapsed {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", elapsed / 60),
        3600..86_400 => format!("{}h", elapsed / 3600),
        _ => format!("{}d", elapsed / 86_400),
    }
}

/// `YYYY-MM-DD HH:MM` in UTC from Unix seconds (Howard Hinnant's civil-date
/// algorithm; no calendar crate needed for a timestamp label).
pub(super) fn format_time(secs: u64) -> String {
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
    use fathomable_core::layout::display_width;

    use crate::app::threads::list::Row;

    use super::{Theme, format_age, format_age_short, format_time, list_row};

    #[test]
    fn selected_review_messages_fill_the_row() -> anyhow::Result<()> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let rows = [
            Row::Message {
                entry: 0,
                message: 1,
                author: "user".to_owned(),
                created: 0,
                badge: None,
                dim: false,
                selected: true,
            },
            Row::Body {
                entry: 0,
                message: 1,
                text: "     revised answer".to_owned(),
                dim: false,
                selected: true,
            },
        ];
        for row in rows {
            let line = list_row(&theme, &row, 0, 30);
            let width: usize = line
                .spans
                .iter()
                .map(|span| display_width(&span.content))
                .sum();
            assert_eq!(line.style, theme.picker_selected);
            assert_eq!(width, 30);
        }
        Ok(())
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
        assert_eq!(format_age_short(now - 30, now), "now");
        assert_eq!(format_age_short(now - 300, now), "5m");
        assert_eq!(format_age_short(now - 7_200, now), "2h");
        assert_eq!(format_age_short(now - 259_200, now), "3d");
    }
}
