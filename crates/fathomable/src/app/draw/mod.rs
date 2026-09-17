// @okf-doc: /decisions/0012-workspace-mode.md
//! Draw the app with ratatui: sidebar, gutter and text, thread surfaces,
//! popups, and the status line; `gutter`, `info`, and `message` build the
//! rows the frame draws.

pub(crate) mod author;
pub(crate) mod bar;
mod counts;
pub(crate) mod gutter;
pub(crate) mod header;
pub(crate) mod info;
pub(crate) mod message;
pub(crate) mod nest;
mod note;
mod selection;
mod threads_pane;

use std::fmt::Write as _;

use fathomable_core::layout::{Face, Style as Face_, display_width, graphemes};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use fathomable_core::diff::LineStatus;
use fathomable_core::status::Summary;

use crate::app::draw::author::{
    CHEVRON_DOWN, CHEVRON_RIGHT, CURSOR_BAR, THREAD_GUTTER, name_style, row_style,
};
use crate::app::draw::header::{
    Header, Tone, diff_header, entry_header, expanded_header, files_pane_header, review_footer,
    review_header, summary_line, summary_rehover,
};
use crate::app::draw::info::Info;
use crate::app::draw::message::{MESSAGE_INDENT, expanded_lines, message_line};
use crate::app::draw::nest::{NEST, nest_span};
use crate::app::draw::note::note_cell;
use crate::app::draw::selection::{Navigation, context_marker};
use crate::app::input::bindings::Action;
use crate::app::threads::list::{BODY_INDENT, Row, Rows};
use crate::app::threads::stubs::{Stub, Subject};
use crate::app::threads::words::Words;
use crate::app::threads::{Compose, ThreadState};
use crate::app::view::{Mode, View};

use crate::app::input::bindings;
use crate::app::input::help;
use crate::app::input::keys::place;
use crate::app::input::menu::{Grid, Menu};
use crate::app::menu_bar::{self, Focused, MenuLayout, Row as MenuRow};
use crate::app::{App, Focus, MAX_TOASTS, NoticeTone, PickerState, Popup};

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
    /// Foreground-only treatment for failed actions on the status line.
    pub(crate) statusline_error: Style,
    /// The `deleted` banner (ADR 0028).
    pub(crate) warning: Style,
    /// A pane's header rows and the key bars (ADR 0059, ADR 0067).
    pub(crate) header: Style,
    pub(crate) mode_normal: Style,
    pub(crate) mode_select: Style,
    pub(crate) mode_input: Style,
    pub(crate) sidebar: Style,
    pub(crate) list_active: Style,
    pub(crate) list_inactive: Style,
    pub(crate) list_cursor: Style,
    pub(crate) list_hover: Style,
    pub(crate) sidebar_dir: Style,
    pub(crate) popup: Style,
    pub(crate) popup_key: Style,
    /// The `Space` menu and the right-click menu (ADR 0056).
    pub(crate) menu: Style,
    pub(crate) picker_match: Style,
    pub(crate) thread_active: Style,
    pub(crate) thread_proposed: Style,
    pub(crate) thread_resolved: Style,
    pub(crate) thread_focus: Style,
    pub(crate) thread_bracket: Style,
    /// A stub's background (ADR 0049).
    pub(crate) thread_inline: Style,
    /// The user's messages: the name's colour and the stripe (ADR 0071).
    pub(crate) thread_user: Style,
    /// An agent's messages: the name's colour and the stripe (ADR 0071).
    pub(crate) thread_agent: Style,
    /// The draft's rows while it is written (ADR 0071).
    pub(crate) thread_draft: Style,
    /// The bar down the thread cursor's message (ADR 0071).
    pub(crate) thread_cursor: Style,
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
            statusline_error: style(Key::UiStatuslineError),
            warning: style(Key::UiWarning),
            header: style(Key::UiHeader),
            mode_normal: style(Key::UiStatuslineNormal),
            mode_select: style(Key::UiStatuslineSelect),
            mode_input: style(Key::UiStatuslineInput),
            sidebar: style(Key::UiSidebar),
            list_active: style(Key::UiListActive),
            list_inactive: style(Key::UiListInactive),
            list_cursor: style(Key::UiListCursor),
            list_hover: style(Key::UiListHover),
            sidebar_dir: style(Key::UiSidebarDir),
            popup: style(Key::UiPopup),
            popup_key: style(Key::UiPopupKey),
            menu: style(Key::UiMenu),
            picker_match: style(Key::UiPickerMatch),
            thread_active: style(Key::ThreadActive),
            thread_proposed: style(Key::ThreadProposed),
            thread_resolved: style(Key::ThreadResolved),
            thread_focus: style(Key::ThreadFocus),
            thread_bracket: style(Key::ThreadBracket),
            thread_inline: style(Key::ThreadInline),
            thread_user: style(Key::ThreadUser),
            thread_agent: style(Key::ThreadAgent),
            thread_draft: style(Key::ThreadDraft),
            thread_cursor: style(Key::ThreadCursor),
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
    gutter_width_for_lines(view.index().line_count())
}

/// Width of the gutter for a source with `line_count` lines.
pub(crate) fn gutter_width_for_lines(line_count: usize) -> usize {
    let digits = line_count.max(1).to_string().len();
    digits + 3
}

fn u16_of(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[expect(
    clippy::too_many_lines,
    reason = "the top-level draw order keeps every overlapping surface explicit"
)]
pub(crate) fn draw(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = frame.area();
    let rows = app.pane_rows();
    let pane_top = u16_of(app.pane_top());
    let sidebar = app.sidebar_width();
    let view = app.view();
    let gutter = gutter_width(view);
    let pane_height = u16_of(rows).min(area.height);
    let sidebar_area = Rect {
        y: area.y + pane_top,
        width: u16_of(sidebar),
        height: pane_height,
        ..area
    };
    // The text column: the view, the draft written in its rows.
    let text_area = Rect {
        x: area.x + sidebar_area.width,
        y: area.y + pane_top,
        width: area.width.saturating_sub(sidebar_area.width),
        height: pane_height,
    };
    let status_area = Rect {
        y: area.y + pane_top + pane_height,
        height: area
            .height
            .saturating_sub(pane_top.saturating_add(pane_height)),
        ..area
    };

    draw_menu_bar(frame, app, theme, area);
    draw_sidebar(frame, app, theme, sidebar_area);
    let text_area = draw_banner(frame, app, theme, text_area);
    let text_area = draw_diff_chrome(frame, app, theme, text_area);
    draw_column(frame, app, theme, text_area, gutter);
    draw_text_bar(frame, app, theme, text_area);
    frame.render_widget(
        status_line(app, theme, usize::from(area.width)),
        status_area,
    );

    draw_toasts(frame, app, theme, text_area);
    match app.popup() {
        Some(Popup::Help(_)) => {
            draw_help(frame, app, theme);
        }
        Some(Popup::Status) => {
            let rows = app.status_lines();
            let grid = Grid::centred(
                &rows,
                STATUS_TITLE,
                0,
                app.pane_top(),
                app.size().0,
                app.pane_rows(),
            );
            draw_table(frame, theme, grid, STATUS_TITLE, &rows, None);
        }
        Some(Popup::Doctor(doctor)) => draw_doctor(frame, app, theme, doctor),
        Some(Popup::About) => draw_about(frame, app, theme),
        Some(Popup::Menu(menu)) => {
            draw_context_menu(frame, app, theme, menu);
        }
        Some(Popup::ConfirmBoard {
            counts, changed, ..
        }) => {
            draw_board_confirmation(frame, theme, app, *counts, *changed);
        }
        Some(Popup::Picker(picker)) => {
            draw_picker(
                frame,
                theme,
                Rect {
                    y: u16_of(app.pane_top()),
                    height: u16_of(app.pane_rows()),
                    ..area
                },
                picker,
                Some(app),
            );
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
                let entries: Vec<(String, String)> = app
                    .which_key(place)
                    .into_iter()
                    .map(|(chord, label)| (chord.to_string(), label))
                    .collect();
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
    draw_title_menus(frame, app, theme);
}

fn draw_menu_bar(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    if !app.menu_bar_shown() || area.height == 0 {
        return;
    }
    let width = usize::from(area.width);
    let labels = menu_bar::labels(width);
    let hovered = app
        .pointer()
        .filter(|(_, row)| *row == 0)
        .map(|(column, _)| column);
    let open = app.menu_bar.open();
    let focused = open.and_then(|open| matches!(open.focused, Focused::Label).then_some(open.root));
    let mut spans = Vec::new();
    for label in labels {
        let active = hovered.is_some_and(|column| {
            menu_bar::label_at(usize::from(area.width), column) == Some(label.root)
        }) || open.is_some_and(|open| open.root == label.root)
            || focused == Some(label.root);
        let style = if active {
            theme.menu.patch(theme.list_hover)
        } else {
            theme.menu
        };
        let text = format!(" {} ", label.root.label());
        spans.push(Span::styled(text, style));
    }
    let tail = menu_bar::bar_tail(app, width);
    spans.push(Span::raw(" ".repeat(tail.padding)));
    if let (Some(base), Some(target)) = (&tail.base, &tail.target) {
        let button = |label: &menu_bar::BarLabel| {
            let surface = if hovered.is_some_and(|column| label.contains(column)) {
                theme.menu.patch(theme.list_hover)
            } else {
                theme.menu
            };
            on_surface(surface, theme.popup_key)
        };
        spans.push(Span::styled(base.text.clone(), button(base)));
        spans.push(Span::styled(" to ", theme.menu.patch(theme.info)));
        spans.push(Span::styled(target.text.clone(), button(target)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme.menu),
        Rect { height: 1, ..area },
    );
    if let Some(identity) = menu_bar::bar_identity(app, width) {
        let repo_surface = if identity.picker.is_some()
            && hovered.is_some_and(|column| identity.repo_contains(column))
        {
            theme.menu.patch(theme.list_hover)
        } else {
            theme.menu
        };
        let repo_style = if identity.picker.is_some() {
            on_surface(repo_surface, theme.popup_key)
        } else {
            repo_surface.patch(theme.info).add_modifier(Modifier::DIM)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(identity.repo, repo_style),
                Span::styled(
                    identity.suffix,
                    theme.menu.patch(theme.info).add_modifier(Modifier::DIM),
                ),
            ]))
            .style(theme.menu),
            Rect {
                x: area.x.saturating_add(u16_of(identity.x)),
                width: u16_of(identity.width),
                height: 1,
                ..area
            },
        );
    }
}

fn draw_title_menus(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(open) = app.menu_bar.open() else {
        return;
    };
    let root_rows = menu_bar::rows(app, open.root);
    let Some(root) = menu_bar::root_layout(app, &root_rows) else {
        return;
    };
    draw_framed_menu(
        frame,
        theme,
        root,
        open.root.title(),
        match open.focused {
            Focused::Root(index) => Some(index),
            Focused::Label | Focused::Child(_) => None,
        },
        &root_rows,
    );
    let Some((submenu, _)) = open.submenu else {
        return;
    };
    let child_rows = menu_bar::submenu_rows(app, submenu);
    let Some(child) = menu_bar::child_layout(app, root, &root_rows, &child_rows) else {
        return;
    };
    draw_framed_menu(
        frame,
        theme,
        child,
        submenu.title(),
        match open.focused {
            Focused::Child(index) => Some(index),
            Focused::Label | Focused::Root(_) => None,
        },
        &child_rows,
    );
}

fn draw_doctor(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    doctor: &crate::app::doctor_view::Doctor,
) {
    let area = doctor_area(app);
    if area.width < 2 || area.height < 2 {
        return;
    }
    let block = rounded_block(
        theme,
        format!(
            " Doctor · {} · r rerun · j/k scroll · Esc close ",
            if doctor.report().passed() {
                "ok"
            } else {
                "failures"
            }
        ),
        theme.popup,
    );
    let inner = block.inner(area);
    let lines = doctor.visual_lines(usize::from(inner.width));
    let shown = lines
        .into_iter()
        .skip(doctor.scroll())
        .take(usize::from(inner.height))
        .map(|(kind, text)| {
            let style = match kind {
                crate::doctor::Kind::Section => theme.popup_key.add_modifier(Modifier::BOLD),
                crate::doctor::Kind::Info => theme.popup,
                crate::doctor::Kind::Ok => theme.diff_plus,
                crate::doctor::Kind::Fail => theme.diff_minus.add_modifier(Modifier::BOLD),
            };
            Line::from(Span::styled(text, on_surface(theme.popup, style)))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(shown).style(theme.popup), inner);
}

fn draw_about(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let area = about_area(app);
    if area.width < 2 || area.height < 2 {
        return;
    }
    let block = rounded_block(theme, " About ", theme.popup);
    let inner = block.inner(area);
    let lines = vec![
        Line::from(Span::styled(
            format!("Fathomable {}", env!("CARGO_PKG_VERSION")),
            theme.popup_key.add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::raw("Read-only terminal workspace viewer and annotation side-car"),
        Line::raw("for agent-driven work."),
        Line::raw(""),
        Line::from(vec![
            Span::styled("License  ", theme.info),
            Span::raw("MIT"),
        ]),
        Line::from(vec![
            Span::styled("Source   ", theme.info),
            Span::styled("https://github.com/hbeberman/fathomable", theme.link),
        ]),
        Line::raw(""),
        Line::from(Span::styled("Esc close", theme.info)),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
}

pub(crate) fn doctor_area(app: &App) -> Rect {
    Rect {
        x: 1,
        y: u16_of(app.pane_top()),
        width: u16_of(app.size().0.saturating_sub(2)),
        height: u16_of(app.pane_rows()),
    }
}

pub(crate) fn about_area(app: &App) -> Rect {
    let (width, _) = app.size();
    let popup_width = width.saturating_sub(4).clamp(2, 64);
    let popup_height = app.pane_rows().clamp(2, 11);
    Rect {
        x: u16_of((width.saturating_sub(popup_width)) / 2),
        y: u16_of(app.pane_top() + app.pane_rows().saturating_sub(popup_height) / 3),
        width: u16_of(popup_width),
        height: u16_of(popup_height),
    }
}

fn rounded_block<'a>(theme: &Theme, title: impl Into<Line<'a>>, surface: Style) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(on_surface(surface, theme.info))
        .title(title)
        .style(surface)
}

fn draw_framed_menu(
    frame: &mut Frame<'_>,
    theme: &Theme,
    layout: MenuLayout,
    title: &str,
    selected: Option<usize>,
    rows: &[MenuRow],
) {
    if layout.width < 2 || layout.height < 2 {
        return;
    }
    let more_above = layout.scroll > 0;
    let more_below = layout.scroll + layout.visible_rows < rows.len();
    let title = format!(
        " {}{}{} ",
        if more_above { "▲ " } else { "" },
        title,
        if more_below { " ▼" } else { "" }
    );
    let area = Rect {
        x: u16_of(layout.x),
        y: u16_of(layout.y),
        width: u16_of(layout.width),
        height: u16_of(layout.height),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.info)
        .title(Span::styled(title, theme.info))
        .style(theme.menu);
    let inner = block.inner(area);
    let inner_width = usize::from(inner.width);
    let lines: Vec<Line<'static>> = rows
        .iter()
        .enumerate()
        .skip(layout.scroll)
        .take(layout.visible_rows)
        .map(|(index, row)| match row {
            MenuRow::Separator => Line::from(Span::styled(
                "─".repeat(inner_width),
                theme.menu.patch(theme.info),
            )),
            MenuRow::Item(item) => {
                let hovered = selected == Some(index);
                let surface = if hovered {
                    theme.menu.patch(theme.list_hover)
                } else {
                    theme.menu
                };
                let label_style = if item.enabled {
                    surface
                } else {
                    surface.patch(theme.info).add_modifier(Modifier::DIM)
                };
                let hint_style = surface.patch(theme.info).add_modifier(if item.enabled {
                    Modifier::empty()
                } else {
                    Modifier::DIM
                });
                let check = if item.checked { "✓ " } else { "  " };
                let arrow = if matches!(item.target, menu_bar::Target::Submenu(_)) {
                    " ›"
                } else {
                    ""
                };
                let fixed = display_width(check)
                    + display_width(&item.label)
                    + display_width(&item.hint)
                    + display_width(arrow);
                let gap = inner_width.saturating_sub(fixed).max(1);
                Line::from(vec![
                    Span::styled(check.to_owned(), label_style),
                    Span::styled(item.label.clone(), label_style),
                    Span::styled(" ".repeat(gap), surface),
                    Span::styled(item.hint.clone(), hint_style),
                    Span::styled(arrow.to_owned(), hint_style),
                ])
                .style(surface)
            }
        })
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).style(theme.menu), inner);
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

/// The text's key bar over the bottom text row (ADR 0067), while it has
/// something to say; the text does not move for it.
fn draw_text_bar(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    if !app.text_bar_shown() || area.height == 0 {
        return;
    }
    let width = usize::from(area.width);
    frame.render_widget(
        Paragraph::new(bar::text_bar(app).line(theme, width)).style(theme.info),
        Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        },
    );
}

/// A comparison header over the text; the rows below it are returned.
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
    Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    }
}

/// The text column: the review list when it is open (ADR 0025), else the
/// selected directory's summary (ADR 0023), else the file-info pane for
/// a binary or over-limit file (ADR 0026), else the document, else the
/// welcome block.
fn draw_column(frame: &mut Frame<'_>, app: &App, theme: &Theme, text_area: Rect, gutter: usize) {
    let text_rows = usize::from(text_area.height);
    if app.getting_started() {
        frame.render_widget(
            Paragraph::new(welcome_lines(app, theme, text_area)).style(theme.text),
            text_area,
        );
    } else if app.review_list().is_open() {
        draw_review(frame, app, theme, text_area);
    } else if let Some(directory) = app.directory_info() {
        draw_directory_info(frame, theme, text_area, &directory);
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
        ("t", "toggle review threads".to_owned()),
        ("Space ?", "view the keymap".to_owned()),
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
        || app.title_menu_open()
        || app.getting_started()
        || !app.has_document()
        || app.directory_path().is_some()
        || app.review_list().is_open()
        || app.info().is_some()
        || view.stub_slot_of_row(view.cursor().row).is_some()
    {
        // The highlighted row is the cursor; leaving the terminal cursor
        // unset keeps it hidden rather than parked on the divider. On an
        // expanded thread's rows the cursor bar (ADR 0071) is the
        // cursor, so the terminal's would only sit on the bar's cell.
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
    let divider = Span::styled("│", sidebar_divider_style(theme));
    let mut out = Vec::with_capacity(rows);
    // The header row on `ui.header` (ADR 0068): Files, then the filter
    // state before the summed `+n -m` totals (ADR 0017).
    let files_header = files_pane_header(app);
    let title_hovered = app
        .pointer()
        .is_some_and(|(column, row)| row == app.pane_top() && column < files_header.left_width());
    let mut header = files_header.line_with_left_hover(theme, inner, title_hovered);
    header.spans.push(divider.clone());
    out.push(header);
    let navigation = Navigation::for_pane(app, Focus::Tree);
    let circles = app.file_circles();
    for (index, row) in tree
        .rows()
        .iter()
        .enumerate()
        .skip(app.tree_scroll())
        .take(rows.saturating_sub(1))
    {
        if inner == 0 {
            out.push(Line::from(divider.clone()));
            continue;
        }
        let marker = if row.is_dir() {
            if row.expanded() { "▾ " } else { "▸ " }
        } else {
            "  "
        };
        let text = format!(
            "{}{marker}{}{}",
            " ".repeat(row.depth() + 2),
            row.name(),
            if row.is_dir() { "/" } else { "" }
        );
        let style = if row.is_dir() {
            theme.sidebar_dir
        } else {
            theme.sidebar
        };
        let selection = navigation.selection(index == tree.cursor());
        let style = theme.sidebar.patch(style).patch(selection.style(theme));
        // A queued change marks its file, and its collapsed ancestors so it
        // shows however the tree is folded (ADR 0015).
        let badge = if row.is_dir() {
            !row.expanded() && app.has_change_under(row.path())
        } else {
            app.queue().contains(row.path())
        };
        let (letters, mut tail) = tree_marks(app, row, theme, style, badge, &circles);
        // The marks follow the name directly, one space apart, and the
        // rest of the row is padded; a narrow sidebar drops the marks.
        let mut tail_width: usize = tail.iter().map(|span| span.content.chars().count()).sum();
        let content_width = inner.saturating_sub(1);
        if tail_width == 0 || content_width <= tail_width + 1 {
            tail.clear();
            tail_width = 0;
        }
        let name = fit(&text, content_width - tail_width).trim_end().to_owned();
        // The Git XY code takes the two gutter columns ahead of the indent.
        let letters = if name.len() >= 2 { letters } else { Vec::new() };
        let used = display_width(&name) + tail_width;
        let mut spans = vec![selection.marker(theme)];
        if letters.is_empty() {
            spans.push(Span::styled(name, style));
        } else {
            spans.extend(letters);
            spans.push(Span::styled(name[2..].to_owned(), style));
        }
        spans.extend(tail);
        spans.push(Span::styled(
            " ".repeat(content_width.saturating_sub(used)),
            style,
        ));
        spans.push(divider.clone());
        out.push(Line::from(spans).style(style));
    }
    while out.len() < rows {
        out.push(Line::from(vec![
            Span::raw(" ".repeat(inner)),
            divider.clone(),
        ]));
    }
    out
}

/// The sidebar divider keeps its own surface when appended to a styled row.
pub(super) fn sidebar_divider_style(theme: &Theme) -> Style {
    theme.marker.bg(theme.sidebar.bg.unwrap_or(Color::Reset))
}

/// The marks around a files pane name: current Git XY status, selected
/// comparison counts, the follow badge, and the thread circle.
#[expect(
    clippy::too_many_lines,
    reason = "the files row keeps Git status and comparison facts aligned"
)]
fn tree_marks<'a>(
    app: &App,
    row: &fathomable_core::tree::Row,
    theme: &Theme,
    style: Style,
    badge: bool,
    circles: &[(std::path::PathBuf, Words)],
) -> (Vec<Span<'a>>, Vec<Span<'a>>) {
    // A collapsed directory folds what is beneath it.
    let comparison_status = app.comparison_status();
    let comparison = if row.is_dir() {
        (!row.expanded())
            .then(|| comparison_status.summary_under(row.path()))
            .flatten()
    } else {
        comparison_status.get(row.path()).map(|entry| Summary {
            state: entry.state(),
            staged: entry.is_staged(),
            changes: entry.changes(),
            added: entry.added(),
            removed: entry.removed(),
            binary: entry.is_binary(),
        })
    };
    let git = if row.is_dir() {
        (!row.expanded())
            .then(|| app.status().summary_under(row.path()))
            .flatten()
    } else {
        app.status().get(row.path()).map(|entry| Summary {
            state: entry.state(),
            staged: entry.is_staged(),
            changes: entry.changes(),
            added: entry.added(),
            removed: entry.removed(),
            binary: entry.is_binary(),
        })
    };
    let on_bg = |mark: Style| style.bg.map_or(mark, |bg| mark.bg(bg));
    let mut letters = Vec::new();
    let mut tail = Vec::new();
    let comparison_kind = (!row.is_dir())
        .then(|| app.comparison_kind(row.path()))
        .flatten();
    if let Some(kind) = comparison_kind {
        let (letter, letter_style) = match kind {
            fathomable_core::diff::PathChangeKind::Added => ('A', theme.diff_plus),
            fathomable_core::diff::PathChangeKind::Deleted => ('D', theme.diff_minus),
            fathomable_core::diff::PathChangeKind::Binary
            | fathomable_core::diff::PathChangeKind::Unsupported => ('B', theme.info),
            fathomable_core::diff::PathChangeKind::Missing => ('!', theme.info),
            fathomable_core::diff::PathChangeKind::ContentChanged
            | fathomable_core::diff::PathChangeKind::ModeChanged
            | fathomable_core::diff::PathChangeKind::TypeChanged => ('M', theme.git_unstaged),
        };
        letters.push(Span::styled(letter.to_string(), on_bg(letter_style)));
        letters.push(Span::styled(" ".to_owned(), on_bg(Style::default())));
    } else if let Some(git) = git
        && !row.is_dir()
    {
        let [staged, unstaged] = git.changes.code();
        for (letter, staged_side) in [(staged, true), (unstaged, false)] {
            let letter_style = match letter {
                'D' => theme.diff_minus,
                '?' | 'U' => theme.diff_plus,
                _ if staged_side => theme.git_staged,
                ' ' => Style::default(),
                _ => theme.git_unstaged,
            };
            letters.push(Span::styled(letter.to_string(), on_bg(letter_style)));
        }
    }
    if let Some(comparison) = comparison {
        if comparison.added > 0 {
            tail.push(Span::styled(
                format!(" +{}", comparison.added),
                on_bg(theme.diff_plus),
            ));
        }
        if comparison.removed > 0 {
            tail.push(Span::styled(
                format!(" -{}", comparison.removed),
                on_bg(theme.diff_minus),
            ));
        }
        // A dirty binary file has no counts; the tag says why (ADR 0026).
        if comparison.binary {
            tail.push(Span::styled(" bin", on_bg(theme.info)));
        }
    }
    if badge {
        tail.push(Span::styled(" ●", on_bg(theme.diff_delta)));
    }
    // The most urgent circle of a file's listed threads, or of a folded
    // directory's (ADR 0066).
    let circle = if row.is_dir() {
        (!row.expanded())
            .then(|| {
                circles
                    .iter()
                    .filter(|(path, _)| path.starts_with(row.path()))
                    .map(|(_, words)| *words)
                    .max_by_key(|words| words.urgency())
            })
            .flatten()
    } else {
        circles
            .iter()
            .find(|(path, _)| path == row.path())
            .map(|(_, words)| *words)
    };
    if let Some(words) = circle {
        tail.push(Span::styled(
            format!(" {}", words.glyph()),
            on_bg(mark_style(theme, words.state())),
        ));
    }
    (letters, tail)
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
            Paragraph::new(threads_pane::threads_pane_lines(
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

/// Pad or truncate `text` to exactly `width` cells.
pub(super) fn fit(text: &str, width: usize) -> String {
    let (prefix, used) = fitting_prefix(text, width);
    let mut out = String::with_capacity(prefix.len() + width - used);
    out.push_str(prefix);
    out.push_str(&" ".repeat(width - used));
    out
}

/// The longest whole-grapheme prefix fitting `width`, and its cell width.
fn fitting_prefix(text: &str, width: usize) -> (&str, usize) {
    if width == 0 {
        return ("", 0);
    }
    let mut used = 0;
    for (offset, grapheme) in graphemes(text) {
        let cells = display_width(grapheme);
        if cells > width - used {
            return (&text[..offset], used);
        }
        used += cells;
    }
    (text, used)
}

/// Pad `text` to exactly `width` cells, or cut it to fit with `…` as
/// its last cell (ADR 0066).
pub(super) fn fit_ellipsis(text: &str, width: usize) -> String {
    if width == 0 || display_width(text) <= width {
        return fit(text, width);
    }
    let (prefix, used) = fitting_prefix(text, width - 1);
    let mut out = String::with_capacity(prefix.len() + '…'.len_utf8() + width - used - 1);
    out.push_str(prefix);
    out.push('…');
    out.push_str(&" ".repeat(width - used - 1));
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
        ThreadState::Active => theme.thread_active,
        ThreadState::Proposed => theme.thread_proposed,
        ThreadState::Resolved => theme.thread_resolved,
    }
}

fn text_lines<'a>(app: &'a App, theme: &Theme, gutter: usize, rows: usize) -> Vec<Line<'a>> {
    let view = app.view();
    let digits = gutter - 3;
    let selection = view.selection();
    let cursor_line = if Navigation::for_pane(app, Focus::View) == Navigation::Active
        && matches!(view.mode(), Mode::Normal | Mode::Select)
    {
        view.source_line_of_row(view.cursor().row)
    } else {
        None
    };
    let lines = view.layout().lines();
    let width = gutter + view.layout().width();
    let mut expanded: std::collections::HashMap<usize, Vec<Line<'a>>> =
        std::collections::HashMap::new();
    let mut out = Vec::with_capacity(rows);
    for (row, line) in lines.iter().enumerate().skip(view.scroll()).take(rows) {
        // A stub row says what a thread said, under its lines; an
        // expanded thread's rows show the whole of it (ADR 0049).
        if let Some((stub, index, _)) = app.stub_on_row(row) {
            if stub.expanded() {
                let block = view.stub_slot_of_row(row).map_or(0, |(block, _)| block);
                let body = expanded.entry(block).or_insert_with(|| {
                    expanded_block_lines(app, theme, &stub, width - gutter, row - index)
                });
                out.push(with_gutter(
                    app,
                    theme,
                    body.get(index).cloned().unwrap_or_default(),
                    row,
                    digits,
                ));
            } else {
                out.push(stub_line(app, theme, &stub, index, row, gutter, width));
            }
            continue;
        }
        // The note cell brackets a thread's rows (ADR 0027) and lights
        // up on the focused thread's (ADR 0074); the rows themselves
        // carry no tint.
        let note = note_cell(app, theme, row, Style::default());
        let number = line
            .source_line()
            .map_or_else(|| " ".repeat(digits), |n| format!("{n:>digits$}"));
        // Note cell (ADR 0013), number, space, diff bar (ADR 0006). A
        // removal has no line of its own, so it draws as a thin rule along
        // the top of the cell of the line below it.
        let bar = app.git_on_row(row).map_or_else(
            || Span::raw(" "),
            |(status, staged)| {
                // A hunk the index already holds draws thicker
                // (ADR 0017): `▌` staged, `▎` not yet.
                let glyph = match (status, staged) {
                    (LineStatus::Removed, _) => "▔",
                    (_, true) => "▌",
                    (_, false) => "▎",
                };
                Span::styled(glyph, status_style(theme, status))
            },
        );
        let mut spans = vec![
            note,
            Span::styled(
                number,
                if line.source_line().is_some() && line.source_line() == cursor_line {
                    on_surface(
                        theme.line_number.remove_modifier(Modifier::DIM),
                        theme.list_cursor,
                    )
                } else {
                    theme.line_number
                },
            ),
            Span::styled(" ", theme.marker),
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
            let base = face_style(theme, span.style());
            // Split the span per character so selection and match highlights
            // can start and end mid-span.
            for grapheme in grapheme_cells(span.text()) {
                spans.push(Span::styled(grapheme, styled(col, base)));
                col += display_width(grapheme);
            }
        }
        out.push(Line::from(spans));
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

/// Row `index` of a collapsed inline thread header.
fn stub_line<'a>(
    app: &App,
    theme: &Theme,
    stub: &Stub,
    index: usize,
    row: usize,
    gutter: usize,
    width: usize,
) -> Line<'a> {
    let digits = gutter - 3;
    let Some(thread) = stub.thread().and_then(|id| app.thread(id)) else {
        return Line::from("");
    };
    let Some(_message) = stub.message_of_row(index) else {
        return Line::from("");
    };
    let covered = app.threads_at_cursor().contains(thread.id());
    let marked = covered && app.thread_cursor().thread() == Some(thread.id());
    let content_width = width.saturating_sub(gutter);
    let layout = expanded_header(app, thread, marked, false, content_width);
    let screen_row = app.text_top() + row.saturating_sub(app.view().scroll());
    let hover = summary_hover(
        app.pointer(),
        &layout,
        screen_row,
        app.sidebar_width() + gutter,
    );
    let leading = vec![if marked {
        Span::styled(CURSOR_BAR, theme.header.patch(theme.thread_cursor))
    } else {
        Span::styled(" ", theme.header)
    }];
    let line = summary_line(theme, &layout, leading, hover);
    with_gutter(app, theme, line, row, digits)
}

fn summary_hover(
    pointer: Option<(usize, usize)>,
    layout: &crate::app::threads::summary::SummaryLayout,
    screen_row: usize,
    column_origin: usize,
) -> Option<Action> {
    pointer
        .filter(|(_, row)| *row == screen_row)
        .and_then(|(column, _)| column.checked_sub(column_origin))
        .and_then(|column| layout.action_at(column))
}

/// The rows of `stub`'s block expanded in place (ADR 0049), at the text
/// width: a shared factual header, then every message as
/// the pane drew them,
/// the draft in its place among them (ADR 0054); for a draft block, a
/// header naming the lines and the draft.
fn expanded_block_lines<'a>(
    app: &App,
    theme: &Theme,
    stub: &Stub,
    width: usize,
    first_row: usize,
) -> Vec<Line<'a>> {
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
            // The header's bar says this is the thread the keys act on
            // (ADR 0071); a message's bar waits for the text cursor to
            // be on the thread's own rows, so from its lines above no
            // message reads as the one under the cursor.
            let current = app.thread_cursor().thread() == Some(id);
            let selected = app
                .expanded_row_message(app.view().cursor().row)
                .filter(|(on, _)| on == id)
                .map(|(_, message)| message);
            let layout = expanded_header(app, thread, current, true, width);
            let screen_row = app.text_top() + first_row.saturating_sub(app.view().scroll());
            let hover = summary_hover(
                app.pointer(),
                &layout,
                screen_row,
                app.sidebar_width() + gutter_width(app.view()),
            );
            let leading = vec![if current {
                Span::styled(CURSOR_BAR, theme.header.patch(theme.thread_cursor))
            } else {
                Span::styled(" ", theme.header)
            }];
            let mut lines = vec![summary_line(theme, &layout, leading, hover)];
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

/// The draft's rows (ADR 0054): the author row, ` User  draft` as a
/// message's author row reads with the user's name colour, then the
/// text wrapped at the draft's width and indented as a body is, every
/// row on `thread.draft` (ADR 0071); the draft's keys are on the text's
/// key bar (ADR 0067).
fn draft_lines<'a>(app: &App, theme: &Theme, compose: &Compose, width: usize) -> Vec<Line<'a>> {
    let surface = theme
        .thread_draft
        .bg
        .map_or_else(Style::default, |bg| Style::default().bg(bg));
    let header = vec![
        Span::raw(" ".repeat(THREAD_GUTTER)),
        Span::styled(
            app.user_name().to_owned(),
            name_style(theme, &fathomable_core::annotations::Author::User, false),
        ),
        Span::styled("  draft", theme.info),
    ];
    let mut lines = vec![message_line(header, width, surface)];
    let buffer = compose.buffer();
    let text_width = app.draft_width();
    let indent = " ".repeat(MESSAGE_INDENT);
    let rows = buffer.rows(text_width);
    if rows.is_empty() {
        lines.push(message_line(vec![Span::raw(indent)], width, surface));
        return lines;
    }
    for row in rows {
        let spans = vec![
            Span::raw(indent.clone()),
            Span::styled(buffer.row_text(row).to_owned(), theme.text),
        ];
        lines.push(message_line(spans, width, surface));
    }
    lines
}

/// An expanded thread's row with the gutter in front of it: the bracket
/// of a thread spanning the row, blanks for the number and the bar, on
/// the `thread.inline` background; a header row on `ui.header` paints
/// its gutter cells on that surface too, so it reaches the left edge
/// (ADR 0064).
fn with_gutter<'a>(
    app: &App,
    theme: &Theme,
    line: Line<'a>,
    row: usize,
    digits: usize,
) -> Line<'a> {
    let inner = line.style;
    let row_style = if inner == theme.header {
        theme.thread_inline.patch(inner)
    } else {
        theme.thread_inline
    };
    let note = note_cell(app, theme, row, row_style);
    let mut spans = vec![
        note,
        Span::styled(" ".repeat(digits), row_style),
        Span::styled(" ", row_style),
        Span::styled(" ", row_style),
    ];
    spans.extend(line.spans);
    Line::from(spans).style(theme.thread_inline.patch(inner))
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
    let badges: Vec<&String> = if app.menu_bar_shown() {
        Vec::new()
    } else {
        parts.badges.iter().collect()
    };
    // Keep the right-hand block visible by trimming identity from the left.
    let badges_width: usize = badges.iter().map(|badge| display_width(badge) + 2).sum();
    let identity_width = usize::from(!app.menu_bar_shown());
    let fixed =
        display_width(parts.pill) + 3 + badges_width + parts.right_width() + 8 * identity_width;
    let directory = app.directory_path();
    let path = directory.map_or_else(
        || app.current_path().to_string_lossy().into_owned(),
        |directory| format!("{}/", directory.display()),
    );
    let mut left = vec![Span::styled(format!(" {} ", parts.pill), pill_style)];
    if !app.menu_bar_shown() {
        let path = truncate_left(&path, width.saturating_sub(fixed));
        left.push(Span::raw(format!(" {path}")));
        if directory.is_none() && view.changed() {
            left.push(Span::styled(" [+]", theme.info));
        }
    }
    for badge in badges {
        left.push(Span::styled(format!("  {badge}"), theme.info));
    }
    if !app.prefix().is_empty() {
        left.push(Span::styled(
            format!("  {}", bindings::spell(app.prefix())),
            theme.info,
        ));
    }
    if let Some(message) = app.message() {
        left.push(Span::styled(
            format!("  {message}"),
            status_message_style(app, theme),
        ));
    } else if let Some(hint) = hint {
        left.push(Span::styled(format!("  {hint}"), theme.diff_delta));
    }
    let used: usize = left
        .iter()
        .map(|s| display_width(&s.content))
        .sum::<usize>()
        + parts.right_width();
    let pad = width.saturating_sub(used);
    left.push(Span::raw(" ".repeat(pad)));
    for segment in parts.right {
        let style = match segment.tone {
            StatusTone::Plain => Style::default(),
        };
        left.push(Span::styled(segment.text, style));
    }
    Paragraph::new(Line::from(left)).style(theme.statusline)
}

fn status_message_style(app: &App, theme: &Theme) -> Style {
    match app.message_tone() {
        NoticeTone::Info => theme.info,
        NoticeTone::Error => theme.statusline_error,
    }
}

/// The status line's words (ADR 0010, amended by 0046's session): the
/// pill says one thing, the mode or the focused pane; the badges after
/// the path say how the text is shown (`SRC`, or `DIFF` and the base,
/// ADR 0060); the right block is
/// `line:col`, the percentage, and `N word` counts, in segments so the
/// thread total remains clickable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusParts {
    pub(crate) pill: &'static str,
    pub(crate) badges: Vec<String>,
    pub(crate) right: Vec<StatusSegment>,
}

impl StatusParts {
    /// The cells the right block takes.
    pub(crate) fn right_width(&self) -> usize {
        self.right
            .iter()
            .map(|segment| display_width(&segment.text))
            .sum()
    }

    /// The right block as one string.
    #[cfg(test)]
    pub(crate) fn right_text(&self) -> String {
        self.right
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    /// The action a click `column` cells into the right block runs.
    pub(crate) fn action_at(&self, column: usize) -> Option<Action> {
        let mut at = 0;
        for segment in &self.right {
            let end = at + display_width(&segment.text);
            if column >= at && column < end {
                return segment.action;
            }
            at = end;
        }
        None
    }
}

/// One run of the status line's right block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusSegment {
    pub(crate) text: String,
    tone: StatusTone,
    /// What a click on it runs (ADR 0066).
    action: Option<Action>,
}

impl StatusSegment {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: StatusTone::Plain,
            action: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusTone {
    Plain,
}

pub(super) fn status_parts(app: &App) -> StatusParts {
    let view = app.view();
    let directory = app.directory_path();
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
    if directory.is_none() {
        if let Some(diff) = view.diff() {
            badges.push(diff.badge.clone());
        } else if view.source_view() {
            badges.push("SRC".to_owned());
        }
        if app.comparison_badge_visible() {
            badges.push(app.comparison_badge());
        }
    }
    let mut right = Vec::new();
    if directory.is_none() {
        let (line, col) = view.source_position();
        let mut position = format!(" {line}:{col}  {}%", view.percent());
        let counts = if view.diff_view() {
            view.pair_counts()
        } else {
            view.diff_counts()
        };
        if let Some((added, removed)) = counts.filter(|(a, r)| a + r > 0) {
            // Infallible: writing to a `String` cannot fail.
            let _ = write!(position, "  +{added} -{removed}");
        }
        right.push(StatusSegment::plain(position));
        let proposed = app.proposed_count();
        if proposed > 0 {
            right.push(StatusSegment::plain(format!("  {proposed} proposed")));
        }
        let threads = app.thread_counts().1;
        if threads > 0 {
            right.push(StatusSegment {
                text: format!("  {threads} threads"),
                tone: StatusTone::Plain,
                action: Some(Action::WindowThreads),
            });
        }
    }
    right.push(StatusSegment::plain("  "));
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
    let counts = if app.directory_path().is_none() && app.current_path() == change.path {
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

/// The which-key grid at the viewer's bottom right, above the status
/// line, shared with mouse hit testing (ADR 0050).
pub(crate) fn which_key_grid(app: &App, entries: &[(String, String)]) -> Grid {
    Grid::bottom(
        entries,
        &bindings::menu_title(app.prefix()),
        0,
        app.pane_top(),
        app.size().0,
        app.pane_rows(),
    )
}

const STATUS_TITLE: &str = " Status · any key closes ";

/// A key menu in a rounded border titled by its prefix or context;
/// `hover` is the entry under the pointer.
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
    let mut lines = Vec::with_capacity(grid.rows);
    for r in 0..grid.rows {
        let mut spans = Vec::new();
        for c in 0..grid.columns {
            let index = c * grid.rows + r;
            let Some((key, label)) = entries.get(index) else {
                break;
            };
            let row_style = if hover == Some(index) {
                theme.list_hover
            } else {
                Style::default()
            };
            spans.push(Span::styled(
                format!("{key:>key_width$}"),
                theme.menu.patch(theme.info).patch(row_style),
            ));
            spans.push(Span::styled(
                format!("  {label:<label_width$}   "),
                theme.menu.patch(row_style),
            ));
        }
        lines.push(Line::from(spans));
    }
    let area = grid_rect(grid);
    let block = rounded_block(
        theme,
        Span::styled(format!(" {title} "), theme.info),
        theme.menu,
    );
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).style(theme.menu), inner);
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
    let (width, _) = app.size();
    let grid = menu.grid_in(width, app.pane_top(), app.pane_rows());
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

/// The compact keymap: grouped binding rows flow through one or
/// two columns, with the footer and hit geometry owned by `input::help`.
fn draw_help(frame: &mut Frame<'_>, app: &App, theme: &Theme) {
    let Some(help) = help::state(app) else {
        return;
    };
    let (width, height) = app.size();
    let layout = help.layout(width, height);
    let popup = Rect {
        x: u16_of(layout.x),
        y: u16_of(layout.y),
        width: u16_of(layout.width),
        height: u16_of(layout.height),
    };
    let block = rounded_block(theme, help_title(help, &layout, theme), theme.popup);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let hovered = app
        .pointer()
        .and_then(|(column, row)| layout.binding_index_at(column, row));
    for (row_index, row) in layout.rows.iter().enumerate() {
        frame.render_widget(
            Paragraph::new(help_body_line(row, &layout, help.query(), hovered, theme))
                .style(theme.popup),
            Rect {
                x: inner.x,
                y: u16_of(layout.body_y + row_index),
                width: inner.width,
                height: 1,
            },
        );
    }

    for (index, footer) in layout.footer.iter().enumerate() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                footer.clone(),
                on_surface(theme.popup, theme.info),
            )))
            .style(theme.popup),
            Rect {
                x: inner.x,
                y: u16_of(layout.footer_y() + index),
                width: inner.width,
                height: 1,
            },
        );
    }
}

fn help_title<'a>(help: &help::Help, layout: &help::Layout, theme: &Theme) -> Line<'a> {
    let mut spans = vec![Span::styled(
        " View keymap",
        theme.popup_key.add_modifier(Modifier::BOLD),
    )];
    if !help.query().is_empty() {
        spans.push(Span::styled("  /", on_surface(theme.popup, theme.info)));
        spans.push(Span::styled(
            help.query().to_owned(),
            theme.popup.patch(theme.picker_match),
        ));
    } else if help.filtering() {
        spans.push(Span::styled("  /", theme.popup.patch(theme.picker_match)));
    }
    let position = format!(
        "  {}{}  {}/{} ",
        if layout.more_above { "↑ " } else { "" },
        if layout.more_below { "↓ more" } else { "" },
        layout.shown_bindings,
        layout.total_bindings
    );
    spans.push(Span::styled(position, on_surface(theme.popup, theme.info)));
    Line::from(spans)
}

fn help_body_line<'a>(
    row: &help::Row,
    layout: &help::Layout,
    query: &str,
    hovered: Option<usize>,
    theme: &Theme,
) -> Line<'a> {
    let mut spans = Vec::new();
    for (column, cell) in row.cells.iter().enumerate() {
        if column > 0 {
            spans.push(Span::raw(" ".repeat(layout.column_gap)));
        }
        let Some(cell) = cell else {
            spans.push(Span::raw(" ".repeat(layout.column_width)));
            continue;
        };
        let row_style = if cell
            .binding()
            .is_some_and(|binding| hovered == Some(binding))
        {
            theme.list_hover
        } else {
            Style::default()
        };
        match cell {
            help::Cell::Heading(text) => spans.push(Span::styled(
                pad(text, layout.column_width),
                matched_style(
                    text,
                    query,
                    theme.popup.add_modifier(Modifier::BOLD),
                    theme.picker_match,
                    row_style,
                ),
            )),
            help::Cell::Entry {
                key,
                label,
                key_width,
                ..
            } => {
                spans.push(Span::styled(
                    pad(key, *key_width),
                    matched_style(
                        key,
                        query,
                        theme.popup.patch(theme.popup_key),
                        theme.picker_match,
                        row_style,
                    ),
                ));
                spans.push(Span::styled("  ", row_style));
                spans.push(Span::styled(
                    pad(label, layout.column_width.saturating_sub(*key_width + 2)),
                    matched_style(label, query, theme.popup, theme.picker_match, row_style),
                ));
            }
            help::Cell::Message(text) => spans.push(Span::styled(
                pad(text, layout.column_width),
                on_surface(theme.popup, theme.info).patch(row_style),
            )),
        }
    }
    Line::from(spans)
}

fn pad(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(display_width(text)))
    )
}

fn matched_style(text: &str, query: &str, base: Style, matched: Style, row: Style) -> Style {
    let style = if !query.is_empty() && text.to_lowercase().contains(&query.to_lowercase()) {
        base.patch(matched)
    } else {
        base
    };
    style.patch(row)
}

/// Apply an accent's foreground and modifiers without allowing its
/// background to replace the surface beneath it.
fn on_surface(surface: Style, accent: Style) -> Style {
    let mut style = surface.patch(accent);
    style.bg = surface.bg;
    style
}

/// A centred popup with a rounded titled border.
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
    // Rows that do not fit flow into further columns.
    let mut lines: Vec<Line<'_>> = Vec::new();
    for r in 0..grid.rows.min(rows.len()) {
        let mut spans = Vec::new();
        for col in 0..grid.columns {
            let index = col * grid.rows + r;
            let Some((key, label)) = rows.get(index) else {
                break;
            };
            let row_style = if hover == Some(index) {
                theme.list_hover
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
    let block = rounded_block(
        theme,
        Span::styled(title.to_owned(), theme.info),
        theme.popup,
    );
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
}

type PickerCells = Vec<(char, Option<u32>)>;

fn plain_picker_cells(item: &str, width: usize) -> PickerCells {
    let fitted = fit_ellipsis(item, width);
    let source_len = item.chars().count();
    fitted
        .chars()
        .enumerate()
        .map(|(index, ch)| {
            let source = (index < source_len && ch != '…')
                .then(|| u32::try_from(index).ok())
                .flatten();
            (ch, source)
        })
        .collect()
}

fn picker_row_parts(item: &str, width: usize, badge_width: usize) -> (PickerCells, PickerCells) {
    let Some(id) = super::comparison::commit_id_from_row(item) else {
        return (
            plain_picker_cells(item, width.saturating_sub(badge_width)),
            Vec::new(),
        );
    };
    let rest = item
        .strip_prefix(id)
        .and_then(|rest| rest.strip_prefix(' '))
        .unwrap_or_default();
    let Some((subject, date)) = rest.rsplit_once(' ') else {
        return (
            plain_picker_cells(item, width.saturating_sub(badge_width)),
            Vec::new(),
        );
    };
    if date.len() != 10
        || !date.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
    {
        return (
            plain_picker_cells(item, width.saturating_sub(badge_width)),
            Vec::new(),
        );
    }

    let width = width.saturating_sub(badge_width);
    let short = &id[..7];
    let subject_start = id.chars().count() + 1;
    let date_start = subject_start + subject.chars().count() + 1;
    let mut cells = Vec::with_capacity(width);
    for (index, ch) in short.chars().enumerate() {
        cells.push((ch, u32::try_from(index).ok()));
    }
    if width < 18 {
        return (cells, Vec::new());
    }
    if width == 18 {
        cells.push((' ', None));
        for (index, ch) in date.chars().enumerate() {
            cells.push((ch, u32::try_from(date_start + index).ok()));
        }
        return (cells, Vec::new());
    }

    cells.push((' ', None));
    let subject_width = width - 19;
    let fitted = fit_ellipsis(subject, subject_width);
    let subject_len = subject.chars().count();
    for (index, ch) in fitted.chars().enumerate() {
        let source = (index < subject_len && ch != '…')
            .then(|| u32::try_from(subject_start + index).ok())
            .flatten();
        cells.push((ch, source));
    }
    let mut trailing = vec![(' ', None)];
    for (index, ch) in date.chars().enumerate() {
        trailing.push((ch, u32::try_from(date_start + index).ok()));
    }
    (cells, trailing)
}

#[cfg(test)]
fn picker_row_cells(item: &str, width: usize) -> PickerCells {
    let (mut leading, trailing) = picker_row_parts(item, width, 0);
    leading.extend(trailing);
    leading
}

fn push_picker_cells(
    spans: &mut Vec<Span<'_>>,
    used: &mut usize,
    cells: PickerCells,
    matched_positions: &[u32],
    row_style: Style,
    match_style: Style,
    width: usize,
) {
    for (ch, source_index) in cells {
        let cells = display_width(&ch.to_string());
        if *used + cells > width {
            break;
        }
        let matched = source_index
            .is_some_and(|source_index| matched_positions.binary_search(&source_index).is_ok());
        let style = if matched {
            on_surface(row_style, match_style)
        } else {
            row_style
        };
        spans.push(Span::styled(ch.to_string(), style));
        *used += cells;
    }
}

fn picker_roles(
    picker: &PickerState,
    item: &str,
    app: Option<&App>,
) -> super::comparison::EndpointRoles {
    if matches!(
        picker.kind(),
        super::PickerKind::ComparisonBase
            | super::PickerKind::ComparisonTarget
            | super::PickerKind::ComparisonTags(_)
            | super::PickerKind::ComparisonBranchCommits(_)
            | super::PickerKind::ComparisonReviewPoints
            | super::PickerKind::ComparisonAdvanced(_)
    ) {
        app.map_or_else(super::comparison::EndpointRoles::default, |app| {
            app.picker_endpoint_roles(item)
        })
    } else {
        super::comparison::EndpointRoles::default()
    }
}

fn picker_line(
    theme: &Theme,
    picker: &PickerState,
    index: usize,
    item: &fathomable_core::picker::Match,
    width: usize,
    app: Option<&App>,
    hovered: bool,
) -> Line<'static> {
    let text = picker.item(item);
    let selection = Navigation::Active.selection(index == picker.selected());
    let surface = if hovered {
        theme.popup.patch(theme.list_hover)
    } else {
        theme.popup
    };
    let row_style = surface.patch(selection.style(theme));
    let mut spans = vec![selection.marker(theme)];
    let mut used = 1;
    let roles = picker_roles(picker, text, app);
    let content_width = width.saturating_sub(1);
    let min_content = if super::comparison::commit_id_from_row(text).is_some() {
        18
    } else {
        1
    };
    let mut badges = Vec::new();
    let mut reserved = 0;
    for (shown, label, style) in [
        (roles.base, "[current base]", theme.popup_key),
        (roles.target, "[current target]", theme.popup_key),
    ] {
        let badge_width = 1 + display_width(label);
        if shown && content_width.saturating_sub(reserved + badge_width) >= min_content {
            badges.push((label, style, badge_width));
            reserved += badge_width;
        }
    }
    let badge_width = badges.iter().map(|(_, _, width)| width).sum();
    let (leading, trailing) = picker_row_parts(text, content_width, badge_width);
    push_picker_cells(
        &mut spans,
        &mut used,
        leading,
        item.positions(),
        row_style,
        theme.picker_match,
        width,
    );
    for (label, style, _) in badges {
        spans.push(Span::styled(" ", row_style));
        spans.push(Span::styled(
            label,
            on_surface(row_style, style).add_modifier(Modifier::BOLD),
        ));
        used += 1 + display_width(label);
    }
    push_picker_cells(
        &mut spans,
        &mut used,
        trailing,
        item.positions(),
        row_style,
        theme.picker_match,
        width,
    );
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(used)),
        row_style,
    ));
    Line::from(spans).style(row_style)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PickerLayout {
    popup: Rect,
    body: Rect,
    first: usize,
    rows: usize,
}

impl PickerLayout {
    pub(crate) fn contains(self, column: usize, row: usize) -> bool {
        let Ok(column) = u16::try_from(column) else {
            return false;
        };
        let Ok(row) = u16::try_from(row) else {
            return false;
        };
        column >= self.popup.x
            && column < self.popup.x.saturating_add(self.popup.width)
            && row >= self.popup.y
            && row < self.popup.y.saturating_add(self.popup.height)
    }

    pub(crate) fn entry_at(self, column: usize, row: usize, matched: usize) -> Option<usize> {
        let column = u16::try_from(column).ok()?;
        let row = u16::try_from(row).ok()?;
        if column < self.body.x
            || column >= self.body.x.saturating_add(self.body.width)
            || row < self.body.y
            || row >= self.body.y.saturating_add(self.body.height)
        {
            return None;
        }
        let offset = usize::from(row - self.body.y);
        (offset < self.rows)
            .then(|| self.first + offset)
            .filter(|index| *index < matched)
    }
}

fn picker_layout_in(area: Rect, picker: &PickerState) -> PickerLayout {
    let width = area.width.saturating_sub(4).clamp(22, 90);
    let height = area.height.saturating_sub(2).clamp(3, 20);
    let popup = centred(area, width, height);
    let body = Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let rows = super::picker_list_rows(usize::from(area.height));
    PickerLayout {
        popup,
        body,
        first: picker.first_visible(rows),
        rows,
    }
}

pub(crate) fn picker_layout(app: &App, picker: &PickerState) -> PickerLayout {
    picker_layout_in(
        Rect {
            x: 0,
            y: u16_of(app.pane_top()),
            width: u16_of(app.size().0),
            height: u16_of(app.pane_rows()),
        },
        picker,
    )
}

fn draw_picker(
    frame: &mut Frame<'_>,
    theme: &Theme,
    area: Rect,
    picker: &PickerState,
    app: Option<&App>,
) {
    let layout = picker_layout_in(area, picker);
    let popup = layout.popup;
    let list_rows = layout.rows;
    let first = layout.first;
    let title = match picker.kind() {
        super::PickerKind::Files => "files".to_owned(),
        super::PickerKind::AllFiles => "files (incl. ignored)".to_owned(),
        super::PickerKind::Recent => "recent".to_owned(),
        super::PickerKind::ComparisonControl => "comparison".to_owned(),
        super::PickerKind::ComparisonBase => "comparison base".to_owned(),
        super::PickerKind::ComparisonTarget => "comparison target".to_owned(),
        super::PickerKind::ComparisonTags(side) => format!("{} tags", side.label()),
        super::PickerKind::ComparisonBranches(side) => format!("{} branches", side.label()),
        super::PickerKind::ComparisonBranchCommits(side) => picker.scope().map_or_else(
            || format!("{} branch commits", side.label()),
            |branch| format!("{} · {branch}", side.label()),
        ),
        super::PickerKind::ComparisonReviewPoints => "comparison review points".to_owned(),
        super::PickerKind::ComparisonAdvanced(side) => {
            format!("{} advanced endpoints", side.label())
        }
        super::PickerKind::ReviewPointName => "review point name (optional)".to_owned(),
        super::PickerKind::Worktree => "worktree".to_owned(),
    };
    let title_line = Line::from(vec![
        Span::styled(format!(" {title} > "), theme.popup_key),
        Span::raw(picker.input().to_owned()),
        Span::styled(
            format!("   {}/{} ", picker.matched(), picker.total()),
            theme.info,
        ),
    ]);
    let block = rounded_block(theme, title_line, theme.popup);
    let body = block.inner(popup);
    let hovered = app
        .and_then(App::pointer)
        .and_then(|(column, row)| layout.entry_at(column, row, picker.matched()));
    let mut lines = Vec::new();
    let inner = usize::from(body.width);
    for (index, m) in picker
        .matches()
        .iter()
        .enumerate()
        .skip(first)
        .take(list_rows)
    {
        lines.push(picker_line(
            theme,
            picker,
            index,
            m,
            inner,
            app,
            hovered == Some(index),
        ));
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), body);
    let col = (1 + display_width(&title) + 3 + display_width(picker.input()))
        .min(usize::from(popup.width.saturating_sub(1)));
    frame.set_cursor_position((popup.x + u16_of(col), popup.y));
}

fn draw_board_confirmation(
    frame: &mut Frame<'_>,
    theme: &Theme,
    app: &App,
    counts: crate::app::threads::archive::BoardCounts,
    changed: bool,
) {
    let area = Rect {
        x: 0,
        y: u16_of(app.pane_top()),
        width: u16_of(app.size().0),
        height: u16_of(app.pane_rows()),
    };
    let width = area.width.saturating_sub(4).clamp(42, 90);
    let height = 7_u16.min(area.height.max(1));
    let popup = centred(area, width, height);
    let block = rounded_block(theme, " Clear board ", theme.popup);
    let inner = block.inner(popup);
    let mut lines = Vec::new();
    if changed {
        lines.push(Line::from("The board changed; review the updated counts."));
    }
    lines.extend([
        Line::from("Archive the shared board across this repository and all worktrees?"),
        Line::from(format!(
            "{} active/proposed · {} resolved",
            counts.open, counts.resolved
        )),
        Line::from("Enter clear · Esc cancel"),
    ]);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
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

/// A selected directory's path and brief repository summary (ADR 0023).
fn draw_directory_info(
    frame: &mut Frame<'_>,
    theme: &Theme,
    area: Rect,
    info: &crate::app::files_pane::DirectoryInfo,
) {
    let count = |value: Option<usize>| {
        value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
    };
    let mut rows = vec![
        ("files", count(info.files)),
        ("subdirectories", count(info.subdirectories)),
    ];
    if info.changed_files > 0 {
        rows.push((
            "changes",
            format!(
                "{} {} · +{} -{}",
                info.changed_files,
                if info.changed_files == 1 {
                    "file"
                } else {
                    "files"
                },
                info.added,
                info.removed
            ),
        ));
    }
    if info.active_threads + info.proposed_threads + info.resolved_threads > 0 {
        let mut parts = Vec::new();
        if info.active_threads > 0 {
            parts.push(format!("● {} active", info.active_threads));
        }
        if info.proposed_threads > 0 {
            parts.push(format!("◐ {} resolution proposed", info.proposed_threads));
        }
        if info.resolved_threads > 0 {
            parts.push(format!("○ {} resolved", info.resolved_threads));
        }
        rows.push(("threads", parts.join(" · ")));
    }
    let label_width = rows
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0);
    let width = usize::from(area.width);
    let header = vec![Span::styled(
        format!(" {}/", info.path.display()),
        theme.popup_key,
    )];
    let mut lines = vec![
        padded_line(header, width).style(theme.header),
        Line::default(),
    ];
    for (label, value) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {label:>label_width$}"), theme.popup_key),
            Span::raw(format!("  {value}")),
        ]));
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
    let Rows {
        rows: all, entries, ..
    } = app.review_rows(width);
    let now = fathomable_core::clock::now();
    // The header, the entries between, and the key bar on the last row
    // (ADR 0059); one row shows the header alone.
    let mut lines = vec![review_header(app).line(theme, width)];
    let body = rows.saturating_sub(2);
    let scroll = list.scroll().min(all.len().saturating_sub(body));
    for (offset, row) in all.iter().skip(scroll).take(body).enumerate() {
        let context = ListRender {
            theme,
            now,
            width,
            navigation: Navigation::for_pane(app, Focus::Review),
            screen_row: app.pane_top() + 1 + offset,
            pointer: app.pointer(),
            column_origin: app.sidebar_width(),
        };
        lines.push(list_row(&context, row));
    }
    if rows >= 2 {
        lines.resize_with(rows - 1, Line::default);
        lines.push(review_footer(app, &entries).line(theme, width));
    }
    frame.render_widget(Paragraph::new(lines).style(theme.text), area);
}

struct ListRender<'a> {
    theme: &'a Theme,
    now: u64,
    width: usize,
    navigation: Navigation,
    screen_row: usize,
    pointer: Option<(usize, usize)>,
    column_origin: usize,
}

#[expect(
    clippy::too_many_lines,
    reason = "One match keeps every review row variant's surface treatment together."
)]
fn list_row<'a>(context: &ListRender<'a>, row: &Row) -> Line<'a> {
    let theme = context.theme;
    let now = context.now;
    let width = context.width;
    let navigation = context.navigation;
    let screen_row = context.screen_row;
    let pointer = context.pointer;
    let column_origin = context.column_origin;
    match row {
        // A file's row over its threads (ADR 0066): the path in the
        // directory colour after `▾`, or `▸` when folded (ADR 0076), the
        // count at the edge; a muted ancestor bar when the cursor is
        // inside the file, shared selection when it rests on this row.
        Row::File {
            path,
            count,
            folded,
            inside,
            selected,
            ..
        } => {
            let count = format!("{count} ");
            let name = format!("{} {}", file_chevron(*folded), path.display());
            let name_width = width.saturating_sub(1 + display_width(&count));
            let selection = navigation.selection(*selected);
            let marker = if *selected {
                selection.marker(theme)
            } else {
                context_marker(theme, *inside)
            };
            selection.paint(
                theme,
                Line::from(vec![
                    marker,
                    Span::styled(fit_ellipsis(&name, name_width), theme.sidebar_dir),
                    Span::styled(count, theme.info),
                ]),
            )
        }
        // The header as the expanded thread in the text reads (ADR
        // 0066), on `ui.header` unless selected; the cursor's thread
        // takes the list's active or remembered selection.
        Row::Header {
            summary, selected, ..
        } => {
            let selection = navigation.selection(*selected);
            let layout = entry_header(summary, now, *selected, true, width);
            let hover = summary_hover(pointer, &layout, screen_row, column_origin);
            let line = summary_line(
                theme,
                &layout,
                vec![selection.marker(theme), nest_span()],
                hover,
            );
            let mut line = selection.paint(theme, line);
            summary_rehover(theme, &layout, &mut line, 2, hover);
            line
        }
        Row::Stub { .. } => stub_row(context, row),
        // A message's rows on its author's stripe, the name in the
        // author's colour, the cursor's message with the bar down its
        // left edge and its name bold (ADR 0071), in the nest (ADR
        // 0077).
        Row::Message {
            user,
            author,
            created,
            badge,
            dim,
            selected,
            ..
        } => {
            let who = list_author(*user);
            let mut spans = vec![
                navigation.selection(*selected).marker(theme),
                Span::raw(" ".repeat(NEST + 2)),
                Span::styled(
                    author.clone(),
                    if *dim {
                        theme.info
                    } else {
                        name_style(theme, &who, *selected)
                    },
                ),
                Span::styled(format!("  {}", format_age(*created, now)), theme.info),
            ];
            if let Some(badge) = badge {
                spans.push(Span::styled(
                    format!("  [{badge}]"),
                    name_style(theme, &who, false),
                ));
            }
            message_line(spans, width, row_style(theme, &who))
        }
        // A body row's Markdown faces as the file view draws them (ADR
        // 0037); a resolved thread's whole body dimmed.
        Row::Body {
            user,
            line,
            dim,
            selected,
            ..
        } => {
            let mut spans = vec![
                navigation.selection(*selected).marker(theme),
                Span::raw(" ".repeat(BODY_INDENT - 1)),
            ];
            spans.extend(line.spans().iter().map(|span| {
                Span::styled(
                    span.text().to_owned(),
                    if *dim {
                        theme.info
                    } else {
                        face_style(theme, span.style())
                    },
                )
            }));
            message_line(spans, width, row_style(theme, &list_author(*user)))
        }
        Row::Evidence {
            line,
            dim,
            selected,
            ..
        } => {
            let mut spans = vec![
                navigation.selection(*selected).marker(theme),
                Span::raw(" ".repeat(BODY_INDENT - 1)),
            ];
            spans.extend(line.spans().iter().map(|span| {
                Span::styled(
                    span.text().to_owned(),
                    if *dim {
                        theme.info
                    } else {
                        theme.info.patch(face_style(theme, span.style()))
                    },
                )
            }));
            message_line(spans, width, theme.info)
        }
        Row::Blank => Line::from(""),
    }
}

/// A folded thread's one row in the list (ADR 0076), the stub's form:
/// the chevron after the nest (ADR 0077), the circle, the place, the
/// newest message's author on their stripe, its short age, and its
/// first line cut with `…`; selected rows take the shared list tint
/// and show the cursor bar only while the list owns navigation.
fn stub_row<'a>(context: &ListRender<'a>, row: &Row) -> Line<'a> {
    let theme = context.theme;
    let now = context.now;
    let width = context.width;
    let navigation = context.navigation;
    let screen_row = context.screen_row;
    let pointer = context.pointer;
    let column_origin = context.column_origin;
    let Row::Stub {
        summary, selected, ..
    } = row
    else {
        return Line::from("");
    };
    let selection = navigation.selection(*selected);
    let layout = entry_header(summary, now, *selected, false, width);
    let hover = summary_hover(pointer, &layout, screen_row, column_origin);
    let line = summary_line(
        theme,
        &layout,
        vec![selection.marker(theme), nest_span()],
        hover,
    );
    let mut line = selection.paint(theme, line);
    summary_rehover(theme, &layout, &mut line, 2, hover);
    line
}

/// The arrow before a file row's path in the list and the threads pane
/// (ADR 0076): `▾` while its threads show, `▸` while it is folded, so
/// the path never moves.
pub(crate) fn file_chevron(folded: bool) -> &'static str {
    if folded { CHEVRON_RIGHT } else { CHEVRON_DOWN }
}

/// The author kind a list row carries, as [`row_style`] and
/// [`name_style`] read it.
fn list_author(user: bool) -> fathomable_core::annotations::Author {
    if user {
        fathomable_core::annotations::Author::User
    } else {
        fathomable_core::annotations::Author::agent("")
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
pub(super) fn format_age_short(created: u64, now: u64) -> String {
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
    use fathomable_core::highlight::Highlighter;
    use fathomable_core::layout::{Layout, display_width};
    use ratatui::style::{Color, Style};

    use crate::app::testing;
    use crate::app::threads::list::Row;

    use super::{
        ListRender, Theme, fit, fit_ellipsis, format_age, format_age_short, format_time, list_row,
        picker_row_cells, status_message_style,
    };

    #[test]
    fn fitting_keeps_whole_graphemes_and_exact_cell_width() {
        for (text, width, plain, ellipsis) in [
            ("", 0, "", ""),
            ("anything", 0, "", ""),
            ("", 1, " ", " "),
            ("a", 1, "a", "a"),
            ("ab", 1, "a", "…"),
            ("abc", 5, "abc  ", "abc  "),
            ("abcdef", 4, "abcd", "abc…"),
            ("界界", 3, "界 ", "界…"),
            ("界界", 2, "界", "… "),
            ("界", 1, " ", "…"),
            ("e\u{301}", 1, "e\u{301}", "e\u{301}"),
            ("e\u{301}xy", 2, "e\u{301}x", "e\u{301}…"),
            ("👩‍💻xy", 3, "👩‍💻x", "👩‍💻…"),
            ("👩‍💻xy", 2, "👩‍💻", "… "),
            ("👩‍💻", 3, "👩‍💻 ", "👩‍💻 "),
            ("\u{301}", 0, "", ""),
        ] {
            assert_eq!(fit(text, width), plain, "{text:?} at {width}");
            assert_eq!(fit_ellipsis(text, width), ellipsis, "{text:?} at {width}");
            assert_eq!(display_width(plain), width);
            assert_eq!(display_width(ellipsis), width);
        }
    }

    #[test]
    fn fitting_a_long_label_only_copies_its_visible_prefix() {
        let text = "label".repeat(20_000);
        assert_eq!(fit_ellipsis(&text, 6), "label…");
    }

    #[test]
    fn commit_picker_rows_keep_short_ids_and_dates_visible() {
        let row = format!(
            "{} this subject is much too long for the picker 2026-09-16",
            "1234567890abcdef1234567890abcdef12345678"
        );
        let rendered: String = picker_row_cells(&row, 36)
            .into_iter()
            .map(|(ch, _)| ch)
            .collect();
        assert_eq!(display_width(&rendered), 36);
        assert!(rendered.starts_with("1234567 "));
        assert!(rendered.contains('…'));
        assert!(rendered.ends_with(" 2026-09-16"));
    }

    #[test]
    fn error_status_messages_use_the_foreground_only_error_face() -> anyhow::Result<()> {
        let dir = testing::workspace("status-error", testing::README)?;
        let mut app = testing::app(&dir)?;
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let mut theme = Theme::from_core(&core);
        theme.info = Style::default().fg(Color::Blue);
        theme.statusline_error = Style::default().fg(Color::Red);

        app.notice("ordinary");
        assert_eq!(status_message_style(&app, &theme), theme.info);
        app.error("failed");
        assert_eq!(status_message_style(&app, &theme), theme.statusline_error);
        assert_eq!(status_message_style(&app, &theme).bg, None);
        Ok(())
    }

    /// ADR 0071: the cursor's message in the list fills its rows with
    /// the author's stripe and starts each with the bar.
    #[test]
    fn selected_review_messages_fill_the_row_on_their_stripe() -> anyhow::Result<()> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let mut rows = vec![Row::Message {
            entry: 0,
            message: 1,
            user: true,
            author: "User".to_owned(),
            created: 0,
            badge: None,
            dim: false,
            selected: true,
        }];
        let body = Layout::render_message("revised answer", 25, &Highlighter::plain());
        rows.extend(body.lines().iter().map(|line| Row::Body {
            entry: 0,
            message: 1,
            user: true,
            line: line.clone(),
            dim: false,
            selected: true,
        }));
        assert_eq!(rows.len(), 2);
        for row in rows {
            let context = ListRender {
                theme: &theme,
                now: 0,
                width: 30,
                navigation: super::Navigation::Active,
                screen_row: 0,
                pointer: None,
                column_origin: 0,
            };
            let line = list_row(&context, &row);
            let width: usize = line
                .spans
                .iter()
                .map(|span| display_width(&span.content))
                .sum();
            assert_eq!(line.style.bg, theme.thread_user.bg);
            assert_eq!(line.spans[0].content, "▎");
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
