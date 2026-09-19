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
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use fathomable_core::diff::LineStatus;
use fathomable_core::status::Summary;

use crate::app::draw::author::{
    CHEVRON_DOWN, CHEVRON_RIGHT, CURSOR_BAR, THREAD_GUTTER, name_style, row_style,
};
use crate::app::draw::header::{
    Header, Tone, entry_header, expanded_header, file_header, files_pane_header, review_footer,
    review_header, summary_line,
};
use crate::app::draw::info::Info;
use crate::app::draw::message::{MESSAGE_INDENT, expanded_lines, message_line};
use crate::app::draw::nest::{NEST, nest_span};
use crate::app::draw::note::note_cell;
use crate::app::draw::selection::{Navigation, context_marker};
use crate::app::input::bindings::Action;
use crate::app::threads::list::{BODY_INDENT, ReviewView, Row, Rows};
use crate::app::threads::stubs::{Stub, Subject};
use crate::app::threads::words::Words;
use crate::app::threads::{Compose, ThreadState};
use crate::app::view::{Mode, View};

use crate::app::input::bindings;
use crate::app::input::help;
use crate::app::input::keys::place;
use crate::app::input::menu::{Grid, HintCell, HintGrid, Menu};
use crate::app::menu_bar::{self, Focused, MenuLayout, Row as MenuRow};
use crate::app::{App, Focus, MAX_TOASTS, NoticeTone, PickerState, Popup, Toast, ToastKind};

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
    /// The marker and pane name while that pane owns navigation (ADR 0091).
    pub(crate) pane_focus: Style,
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
            pane_focus: style(Key::UiPaneFocus),
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
    if !app.panes_fit() {
        draw_size_warning(frame, app, theme, area);
        frame.render_widget(
            status_line(app, theme, usize::from(area.width)),
            status_area,
        );
        if matches!(app.popup(), Some(Popup::ConfirmQuit)) {
            draw_quit_confirmation(frame, theme, app);
        }
        draw_title_menus(frame, app, theme);
        return;
    }
    draw_sidebar(frame, app, theme, sidebar_area);
    let text_area = draw_banner(frame, app, theme, text_area);
    let text_area = draw_file_chrome(frame, app, theme, text_area);
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
        Some(Popup::Licenses(licenses)) => draw_licenses(frame, app, theme, licenses),
        Some(Popup::McpSetup(setup)) => draw_mcp_setup(frame, app, theme, setup),
        Some(Popup::About) => draw_about(frame, app, theme),
        Some(Popup::Menu(menu)) => {
            draw_context_menu(frame, app, theme, menu, None);
        }
        Some(Popup::DiffMode(menu)) => {
            draw_context_menu(frame, app, theme, menu.menu(), Some(menu.selected()));
        }
        Some(Popup::ConfirmQuit) => draw_quit_confirmation(frame, theme, app),
        Some(Popup::ConfirmBoard {
            counts, changed, ..
        }) => {
            draw_board_confirmation(frame, theme, app, *counts, *changed);
        }
        Some(Popup::ReviewPointAction(action)) => {
            draw_review_point_action(frame, theme, app, action);
        }
        Some(Popup::ReviewPointRename(rename)) => {
            draw_review_point_rename(frame, theme, app, rename);
        }
        Some(Popup::ConfirmReviewPointDelete {
            id, name, created, ..
        }) => {
            draw_review_point_delete_confirmation(frame, theme, app, id, name.as_deref(), *created);
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
                let sections = app.which_key_sections(place);
                let enabled: Vec<bool> = sections
                    .iter()
                    .flat_map(bindings::MenuSection::entries)
                    .map(|entry| app.which_key_enabled(place, entry.chord()))
                    .collect();
                let grid = which_key_grid(app, &sections);
                let hover = app
                    .pointer()
                    .and_then(|(column, row)| grid.entry_at(column, row));
                draw_menu(
                    frame,
                    theme,
                    &grid,
                    &bindings::menu_title(app.prefix()),
                    &sections,
                    &enabled,
                    hover,
                );
            }
            draw_command_completion(frame, app, theme, area, status_area);
            place_cursor(frame, app, view, text_area, status_area, gutter);
        }
    }
    draw_title_menus(frame, app, theme);
}

/// Replace pane content when the selected composition cannot be drawn safely.
fn draw_size_warning(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    let top = u16_of(app.pane_top()).min(area.height.saturating_sub(1));
    let available = Rect {
        y: area.y.saturating_add(top),
        height: u16_of(app.pane_rows()).min(area.height.saturating_sub(top)),
        ..area
    };
    if available.width == 0 || available.height == 0 {
        return;
    }
    let (minimum_width, minimum_height) = app.minimum_pane_size();
    let (width, height) = app.size();
    let message = vec![
        Line::from("Terminal too small"),
        Line::from(format!(
            "Need {minimum_width}×{minimum_height}; have {width}×{height}"
        )),
        Line::from("Resize, use Layout to hide a pane, or q to quit"),
    ];
    let warning = centred(available, available.width, u16_of(message.len()));
    frame.render_widget(Clear, available);
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(theme.warning),
        warning,
    );
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
    let button = |label: &menu_bar::BarLabel| {
        let surface = if hovered.is_some_and(|column| label.contains(column)) {
            theme.menu.patch(theme.list_hover)
        } else {
            theme.menu
        };
        on_surface(surface, theme.popup_key)
    };
    if let Some(target) = &tail.target {
        if let Some(base) = &tail.base {
            spans.push(Span::styled(base.text.clone(), button(base)));
            spans.push(Span::styled(" to ", theme.menu.patch(theme.info)));
        }
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
            Paragraph::new(Line::from(Span::styled(identity.repo, repo_style))).style(theme.menu),
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
    let area = report_area(app);
    if area.width < 2 || area.height < 2 {
        return;
    }
    let block = rounded_block(
        theme,
        format!(
            " Doctor · {} · r rerun · j/k scroll · Esc close ",
            if !doctor.report().passed() {
                "failures"
            } else if doctor.report().has_warnings() {
                "warnings"
            } else {
                "ok"
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
                crate::doctor::Kind::Warn => theme.warning,
                crate::doctor::Kind::Fail => theme.diff_minus.add_modifier(Modifier::BOLD),
            };
            Line::from(Span::styled(text, on_surface(theme.popup, style)))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(shown).style(theme.popup), inner);
}

fn draw_licenses(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    licenses: &crate::app::licenses::Licenses,
) {
    let area = report_area(app);
    if area.width < 2 || area.height < 2 {
        return;
    }
    let block = rounded_block(
        theme,
        " Licenses · j/k scroll · PgUp/PgDn page · Esc close ",
        theme.popup,
    );
    let inner = block.inner(area);
    let lines = licenses
        .visible_lines(usize::from(inner.height))
        .map(|line| Line::raw(line.text()))
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
}

fn draw_mcp_setup(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    setup: &crate::app::mcp_setup::McpSetup,
) {
    let area = report_area(app);
    if area.width < 2 || area.height < 2 {
        return;
    }
    let block = rounded_block(
        theme,
        " MCP Setup · j/k scroll · PgUp/PgDn page · Esc close ",
        theme.popup,
    );
    let inner = block.inner(area);
    let lines = setup
        .visible_lines(usize::from(inner.height))
        .map(|line| Line::raw(line.text()))
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
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
        Line::raw("A read-only workspace viewer for reviewing"),
        Line::raw("diffs and interactive comment threads with agents"),
        Line::raw("via MCP."),
        Line::raw(""),
        Line::from(vec![
            Span::styled("License  ", theme.info),
            Span::raw("MIT (Fathomable)"),
        ]),
        Line::from(vec![
            Span::styled("Source   ", theme.info),
            Span::styled("https://github.com/hbeberman/fathomable", theme.link),
        ]),
        Line::raw("Third-party notices: Help > Licenses"),
        Line::raw(""),
        Line::from(Span::styled("Esc close", theme.info)),
    ];
    let copy_width = lines.iter().map(Line::width).max().unwrap_or_default();
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
    draw_about_anchor(frame, inner, copy_width, theme);
}

const ABOUT_ANCHOR: [&str; 10] = [
    "      (",
    "       )",
    "      (",
    "     _|_",
    "    (   )",
    " ====`|'====",
    "      |",
    " |\\   |   /|",
    " \\'-._|_.-'/",
    "   `-\\|/-'",
];
const ABOUT_ANCHOR_WIDTH: u16 = 12;
const ABOUT_ANCHOR_HEIGHT: u16 = 10;
const ABOUT_ANCHOR_STEAM_COLUMN: u16 = 7;

fn draw_about_anchor(frame: &mut Frame<'_>, inner: Rect, copy_width: usize, theme: &Theme) {
    if inner.width < ABOUT_ANCHOR_WIDTH || inner.height < ABOUT_ANCHOR_HEIGHT {
        return;
    }
    let anchor_x = inner.width - ABOUT_ANCHOR_WIDTH;
    if usize::from(anchor_x) < copy_width {
        return;
    }
    let area = Rect {
        x: inner.x + anchor_x,
        y: inner.y,
        width: ABOUT_ANCHOR_WIDTH,
        height: ABOUT_ANCHOR_HEIGHT,
    };
    let style = on_surface(theme.popup, theme.info);
    let lines = ABOUT_ANCHOR
        .iter()
        .map(|row| Line::from(Span::styled(*row, style)))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
    frame.render_widget(
        Paragraph::new(Span::styled(")", style)),
        Rect::new(area.x + ABOUT_ANCHOR_STEAM_COLUMN, inner.y - 1, 1, 1),
    );
}

pub(crate) fn report_area(app: &App) -> Rect {
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
    let popup_height = app.pane_rows().clamp(2, 12);
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
                let marker = if item.checked {
                    "✓ "
                } else if item.active {
                    "▌ "
                } else {
                    "  "
                };
                let marker_style = if item.active {
                    on_surface(surface, theme.popup_key).add_modifier(Modifier::BOLD)
                } else {
                    label_style
                };
                let arrow = if matches!(item.target, menu_bar::Target::Submenu(_)) {
                    " ›"
                } else {
                    ""
                };
                let fixed = display_width(marker)
                    + display_width(&item.label)
                    + display_width(&item.hint)
                    + display_width(arrow);
                let gap = inner_width.saturating_sub(fixed).max(1);
                Line::from(vec![
                    Span::styled(marker.to_owned(), marker_style),
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
    let bar = bar::text_bar(app);
    let row = usize::from(area.y + area.height - 1);
    let hovered = app
        .pointer()
        .filter(|(_, pointer_row)| *pointer_row == row)
        .and_then(|(column, _)| column.checked_sub(usize::from(area.x)))
        .and_then(|column| bar.action_at(width, column));
    frame.render_widget(
        Paragraph::new(bar.line_with_action_hover(theme, width, hovered)).style(theme.info),
        Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        },
    );
}

/// The file surface's clickable title, current path, and lifecycle counts.
fn draw_file_chrome(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) -> Rect {
    let rows = app.file_chrome_rows();
    if rows == 0 || usize::from(area.height) <= rows {
        return area;
    }
    let header = file_header(app);
    let width = usize::from(area.width);
    let local_pointer = app.pointer().and_then(|(column, row)| {
        (row == usize::from(area.y) && column >= usize::from(area.x))
            .then(|| column - usize::from(area.x))
    });
    let hovered = local_pointer.is_some_and(|column| column < header.title_width());
    let control_hovered = local_pointer.and_then(|column| header.control_at(width, column));
    frame.render_widget(
        Paragraph::new(header.line_with_header_hovers(
            theme,
            width,
            hovered,
            control_hovered,
            app.pane_has_navigation(Focus::View),
        ))
        .style(theme.info),
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
        draw_welcome(frame, app, theme, text_area);
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
        draw_welcome(frame, app, theme, text_area);
    }
}

fn draw_welcome(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    if let Some(lines) = welcome_lines(app, theme, area) {
        frame.render_widget(Paragraph::new(lines).style(theme.text), area);
        return;
    }
    let message = vec![
        Line::from("Terminal too small"),
        Line::from(format!(
            "Welcome needs more room; have {}×{}",
            area.width, area.height
        )),
        Line::from("Resize, use Layout to hide a pane, or q to quit"),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(theme.warning),
        centred(area, area.width, u16_of(3)),
    );
}

type WelcomeEntry = (&'static str, &'static str);

const WELCOME_COLUMN_GAP: usize = 3;
const WELCOME_PANE: &[WelcomeEntry] = &[
    ("f", "File pane"),
    ("F", "File list"),
    ("t", "Threads pane"),
    ("T", "Threads list"),
    ("w/W", "cycle pane focus"),
    ("q", "quit"),
];
const WELCOME_DIFF: &[WelcomeEntry] = &[
    ("J/K", "next/previous change"),
    ("L/H", "next/previous changed file"),
    ("Space d d", "show uncommitted changes"),
    ("Space d l", "show latest commit"),
    ("Space d c", "show a specific commit"),
];
const WELCOME_COMMENT: &[WelcomeEntry] = &[
    ("Tab/⇧Tab", "next/previous comment thread"),
    ("c", "add a comment"),
    ("r", "resolve/reopen thread"),
];
const WELCOME_PRODUCT: &[&str] = &[
    "A read-only workspace viewer for reviewing diffs and",
    "interactive comment threads with agents via MCP.",
];
const WELCOME_INTERACTION: &[&str] = &[
    "Usable with both mouse (right/left click) and keyboard.",
    "Space opens a hotkey list.",
    "Alt-Space moves keyboard focus to the menu bar.",
];

#[derive(Clone, Copy)]
enum WelcomeLayout {
    Single,
    Two,
    Three,
}

struct WelcomePlan {
    product: Vec<String>,
    interaction: Vec<String>,
    layout: WelcomeLayout,
    spacing: usize,
    block_width: usize,
}

/// What the text column shows before any file is open: a short introduction
/// and the keys that get going, centred as a block.
fn welcome_lines<'a>(app: &App, theme: &Theme, area: Rect) -> Option<Vec<Line<'a>>> {
    let root = app.workspace().root().display().to_string();
    let width = usize::from(area.width);
    let plan = welcome_plan(width, usize::from(area.height), display_width(&root))?;
    let left = " ".repeat(width.saturating_sub(plan.block_width) / 2);
    let mut body: Vec<Line<'a>> = vec![
        Line::from(Span::styled(
            format!("{left}Fathomable"),
            theme.heading[0].add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("{left}{}", truncate_left(&root, plan.block_width)),
            theme.text.add_modifier(Modifier::DIM),
        )),
    ];
    if plan.spacing >= 1 {
        body.push(Line::from(""));
    }
    for line in &plan.product {
        body.push(Line::raw(format!("{left}{line}")));
    }
    if plan.spacing >= plan.layout.maximum_spacing() {
        body.push(Line::from(""));
    }
    for line in &plan.interaction {
        body.push(Line::raw(format!("{left}{line}")));
    }
    if plan.spacing >= 2 {
        body.push(Line::from(""));
    }
    let mut controls = welcome_controls(theme, plan.layout, plan.spacing);
    for line in &mut controls {
        line.spans.insert(0, Span::raw(left.clone()));
    }
    body.extend(controls);
    let top = usize::from(area.height).saturating_sub(body.len()) / 3;
    let mut out = vec![Line::from(""); top];
    out.extend(body);
    Some(out)
}

impl WelcomeLayout {
    const fn maximum_spacing(self) -> usize {
        match self {
            Self::Single => 5,
            Self::Two => 4,
            Self::Three => 3,
        }
    }
}

fn welcome_plan(width: usize, height: usize, root_width: usize) -> Option<WelcomePlan> {
    let product = WELCOME_PRODUCT
        .iter()
        .flat_map(|line| wrap_words(line, width))
        .collect::<Vec<_>>();
    let interaction = WELCOME_INTERACTION
        .iter()
        .flat_map(|line| wrap_words(line, width))
        .collect::<Vec<_>>();
    let pane_width = welcome_section_width(WELCOME_PANE);
    let diff_width = welcome_section_width(WELCOME_DIFF);
    let comment_width = welcome_section_width(WELCOME_COMMENT);
    let intro_rows = 2 + product.len() + interaction.len();
    let single_rows = intro_rows
        + welcome_section_height(WELCOME_PANE)
        + welcome_section_height(WELCOME_DIFF)
        + welcome_section_height(WELCOME_COMMENT);
    let single_width = welcome_single_width();
    let two_width = pane_width + WELCOME_COLUMN_GAP + diff_width.max(comment_width);
    let two_rows = intro_rows
        + welcome_section_height(WELCOME_PANE)
            .max(welcome_section_height(WELCOME_DIFF) + welcome_section_height(WELCOME_COMMENT));
    let three_width =
        pane_width + diff_width + comment_width + WELCOME_COLUMN_GAP.saturating_mul(2);
    let three_rows = intro_rows
        + welcome_section_height(WELCOME_PANE)
            .max(welcome_section_height(WELCOME_DIFF))
            .max(welcome_section_height(WELCOME_COMMENT));
    let (layout, required_rows, controls_width) = if single_width <= width && single_rows <= height
    {
        (WelcomeLayout::Single, single_rows, single_width)
    } else if two_width <= width && two_rows <= height {
        (WelcomeLayout::Two, two_rows, two_width)
    } else if three_width <= width && three_rows <= height {
        (WelcomeLayout::Three, three_rows, three_width)
    } else {
        return None;
    };
    let spacing = height
        .saturating_sub(required_rows)
        .min(layout.maximum_spacing());
    let block_width = product
        .iter()
        .chain(&interaction)
        .map(|line| display_width(line))
        .chain([controls_width, root_width.min(width)])
        .max()
        .unwrap_or(0)
        .min(width);
    Some(WelcomePlan {
        product,
        interaction,
        layout,
        spacing,
        block_width,
    })
}

fn welcome_controls<'a>(theme: &Theme, layout: WelcomeLayout, spacing: usize) -> Vec<Line<'a>> {
    let shared_key_width = WELCOME_PANE
        .iter()
        .chain(WELCOME_DIFF)
        .chain(WELCOME_COMMENT)
        .map(|(key, _)| display_width(key))
        .max()
        .unwrap_or(0);
    let key_width = |entries| match layout {
        WelcomeLayout::Single => shared_key_width,
        WelcomeLayout::Two | WelcomeLayout::Three => welcome_key_width(entries),
    };
    let pane = welcome_section(
        theme,
        "Pane navigation",
        WELCOME_PANE,
        key_width(WELCOME_PANE),
    );
    let diff = welcome_section(
        theme,
        "Diff controls",
        WELCOME_DIFF,
        key_width(WELCOME_DIFF),
    );
    let comment = welcome_section(
        theme,
        "Comment controls",
        WELCOME_COMMENT,
        key_width(WELCOME_COMMENT),
    );
    match layout {
        WelcomeLayout::Single => {
            let mut lines = pane;
            if spacing >= 3 {
                lines.push(Line::from(""));
            }
            lines.extend(diff);
            if spacing >= 4 {
                lines.push(Line::from(""));
            }
            lines.extend(comment);
            lines
        }
        WelcomeLayout::Two => {
            let mut second = diff;
            if spacing >= 3 {
                second.push(Line::from(""));
            }
            second.extend(comment);
            welcome_columns(
                &[pane, second],
                &[
                    welcome_section_width(WELCOME_PANE),
                    welcome_section_width(WELCOME_DIFF).max(welcome_section_width(WELCOME_COMMENT)),
                ],
                WELCOME_COLUMN_GAP,
            )
        }
        WelcomeLayout::Three => welcome_columns(
            &[pane, diff, comment],
            &[
                welcome_section_width(WELCOME_PANE),
                welcome_section_width(WELCOME_DIFF),
                welcome_section_width(WELCOME_COMMENT),
            ],
            WELCOME_COLUMN_GAP,
        ),
    }
}

fn welcome_section<'a>(
    theme: &Theme,
    title: &'static str,
    entries: &[WelcomeEntry],
    key_width: usize,
) -> Vec<Line<'a>> {
    let mut lines = vec![Line::from(Span::styled(
        title,
        theme.info.add_modifier(Modifier::BOLD),
    ))];
    lines.extend(entries.iter().map(|(key, label)| {
        Line::from(vec![
            Span::styled(format!("{key:<key_width$}"), theme.popup_key),
            Span::raw(format!("  {label}")),
        ])
    }));
    lines
}

fn welcome_key_width(entries: &[WelcomeEntry]) -> usize {
    entries
        .iter()
        .map(|(key, _)| display_width(key))
        .max()
        .unwrap_or(0)
}

fn welcome_section_width(entries: &[WelcomeEntry]) -> usize {
    let key_width = welcome_key_width(entries);
    entries
        .iter()
        .map(|(_, label)| key_width + 2 + display_width(label))
        .max()
        .unwrap_or(0)
}

const fn welcome_section_height(entries: &[WelcomeEntry]) -> usize {
    entries.len() + 1
}

fn welcome_single_width() -> usize {
    let key_width = WELCOME_PANE
        .iter()
        .chain(WELCOME_DIFF)
        .chain(WELCOME_COMMENT)
        .map(|(key, _)| display_width(key))
        .max()
        .unwrap_or(0);
    WELCOME_PANE
        .iter()
        .chain(WELCOME_DIFF)
        .chain(WELCOME_COMMENT)
        .map(|(_, label)| key_width + 2 + display_width(label))
        .max()
        .unwrap_or(0)
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() || width == 0 {
        return vec![text.to_owned()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let joined_width =
            display_width(&line) + usize::from(!line.is_empty()) + display_width(word);
        if !line.is_empty() && joined_width > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn welcome_columns<'a>(columns: &[Vec<Line<'a>>], widths: &[usize], gap: usize) -> Vec<Line<'a>> {
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    (0..rows)
        .map(|row| {
            let mut spans = Vec::new();
            for (index, (column, width)) in columns.iter().zip(widths).enumerate() {
                let line = column.get(row);
                if let Some(line) = line {
                    spans.extend(line.spans.clone());
                }
                let used = line.map_or(0, Line::width);
                spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
                if index + 1 < columns.len() {
                    spans.push(Span::raw(" ".repeat(gap)));
                }
            }
            Line::from(spans)
        })
        .collect()
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

const COMMAND_COLUMN_WIDTH: usize = 18;
const COMMAND_MAX_ROWS: usize = 10;

fn draw_command_completion(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    area: Rect,
    status_area: Rect,
) {
    let Some(completion) = app.view().command_completion() else {
        return;
    };
    let candidates = completion.candidates();
    let available_rows = usize::from(status_area.y.saturating_sub(u16_of(app.pane_top())));
    if candidates.is_empty() || available_rows == 0 || area.width == 0 {
        return;
    }

    let columns = (usize::from(area.width) / COMMAND_COLUMN_WIDTH)
        .max(1)
        .min(candidates.len());
    let wanted_rows = candidates.len().div_ceil(columns).min(COMMAND_MAX_ROWS);
    let grid_rows = wanted_rows.min(available_rows);
    let description_rows = usize::from(
        completion.selected().is_some() && available_rows.saturating_sub(grid_rows) >= 3,
    ) * 3;
    let total_rows = grid_rows + description_rows;
    let start_y = status_area.y.saturating_sub(u16_of(total_rows));
    let overlay = Rect {
        y: start_y,
        height: u16_of(total_rows),
        ..area
    };
    frame.render_widget(Clear, overlay);

    if let Some(command) = completion.selected().filter(|_| description_rows == 3) {
        let description = Rect {
            height: 3,
            ..overlay
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme.info);
        let text = fit_ellipsis(
            command.description(),
            usize::from(description.width.saturating_sub(4)),
        );
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {text} ")))
                .style(theme.popup)
                .block(block),
            description,
        );
    }

    let grid = Rect {
        y: start_y.saturating_add(u16_of(description_rows)),
        height: u16_of(grid_rows),
        ..overlay
    };
    let capacity = columns.saturating_mul(grid_rows).min(candidates.len());
    let first = completion.selected_index().map_or(0, |selected| {
        if selected < capacity {
            0
        } else {
            selected
                .saturating_add(1)
                .saturating_sub(capacity)
                .min(candidates.len().saturating_sub(capacity))
        }
    });
    let mut lines = vec![Line::default(); grid_rows];
    for (offset, index) in (first..first.saturating_add(capacity)).enumerate() {
        let row = offset % grid_rows;
        let column = offset / grid_rows;
        let used: usize = lines[row]
            .spans
            .iter()
            .map(|span| display_width(&span.content))
            .sum();
        let target = column.saturating_mul(COMMAND_COLUMN_WIDTH);
        lines[row]
            .spans
            .push(Span::raw(" ".repeat(target.saturating_sub(used))));
        let command = candidates[index].form();
        let style = if completion.selected_index() == Some(index) {
            theme.list_cursor
        } else {
            theme.popup
        };
        lines[row].spans.push(Span::styled(
            fit_ellipsis(command, COMMAND_COLUMN_WIDTH),
            style,
        ));
    }
    frame.render_widget(Paragraph::new(lines).style(theme.popup), grid);
}

/// Change toasts, bottom-right above the status line, newest at the
/// bottom (ADR 0015).
fn draw_toasts(frame: &mut Frame<'_>, app: &App, theme: &Theme, pane: Rect) {
    let toasts = app.toasts();
    if toasts.is_empty() || pane.height == 0 {
        return;
    }
    let mut shown: Vec<&Toast> = toasts
        .iter()
        .rev()
        .filter(|toast| {
            app.diff_mode() != fathomable_core::config::DiffMode::Off
                || !matches!(toast.kind(), ToastKind::FileEdit { .. })
        })
        .take(MAX_TOASTS)
        .collect();
    shown.reverse();
    if shown.is_empty() {
        return;
    }
    let width = shown
        .iter()
        .map(|toast| display_width(toast.text()).saturating_add(2))
        .max()
        .unwrap_or(0)
        .min(usize::from(pane.width));
    let height = u16_of(shown.len()).min(pane.height);
    let area = Rect {
        x: pane.x + pane.width - u16_of(width),
        y: pane.y + pane.height - height,
        width: u16_of(width),
        height,
    };
    let text: Vec<Line<'_>> = shown
        .into_iter()
        .rev()
        .take(usize::from(height))
        .rev()
        .map(|toast| toast_line(toast, theme))
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(text).style(theme.popup), area);
}

fn toast_line(toast: &Toast, theme: &Theme) -> Line<'static> {
    let ToastKind::FileEdit {
        path,
        added,
        removed,
    } = toast.kind()
    else {
        return Line::from(Span::styled(format!(" {} ", toast.text()), theme.popup));
    };
    let mut spans = vec![
        Span::styled(" ".to_owned(), theme.popup),
        Span::styled(path.display().to_string(), theme.popup),
    ];
    if *added > 0 {
        spans.push(Span::styled(
            format!("  +{added}"),
            on_surface(theme.popup, theme.diff_plus),
        ));
    }
    if *removed > 0 {
        spans.push(Span::styled(
            format!("  -{removed}"),
            on_surface(theme.popup, theme.diff_minus),
        ));
    }
    spans.push(Span::styled(" ".to_owned(), theme.popup));
    Line::from(spans)
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
    // The header row on `ui.header` (ADR 0068): File list, then the filter
    // state before the summed `+n -m` totals (ADR 0017).
    let files_header = files_pane_header(app);
    let title_hovered = app
        .pointer()
        .is_some_and(|(column, row)| row == app.pane_top() && column < files_header.left_width());
    let mut header = files_header.line_with_left_hover(
        theme,
        inner,
        title_hovered,
        app.pane_has_navigation(Focus::Tree),
    );
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
        let (letters, mut tail) = tree_marks(app, row, theme, style, &circles);
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
/// comparison counts, and the thread circle.
#[expect(
    clippy::too_many_lines,
    reason = "the files row keeps Git status and comparison facts aligned"
)]
fn tree_marks<'a>(
    app: &App,
    row: &fathomable_core::tree::Row,
    theme: &Theme,
    style: Style,
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
    let git = if app.diff_mode() == fathomable_core::config::DiffMode::Off {
        None
    } else if row.is_dir() {
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
    let layout = expanded_header(app, thread, false, content_width);
    let leading = vec![if marked {
        Span::styled(CURSOR_BAR, theme.header.patch(theme.thread_cursor))
    } else {
        Span::styled(" ", theme.header)
    }];
    let line = summary_line(theme, &layout, leading);
    with_gutter(app, theme, line, row, digits)
}

/// The rows of `stub`'s block expanded in place (ADR 0049), at the text
/// width: a shared factual header, then every message as
/// the pane drew them,
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
            // The header's bar says this is the thread the keys act on
            // (ADR 0071); a message's bar waits for the text cursor to
            // be on the thread's own rows, so from its lines above no
            // message reads as the one under the cursor.
            let current = app.thread_cursor().thread() == Some(id);
            let selected = app
                .expanded_row_message(app.view().cursor().row)
                .filter(|(on, _)| on == id)
                .map(|(_, message)| message);
            let layout = expanded_header(app, thread, true, width);
            let leading = vec![if current {
                Span::styled(CURSOR_BAR, theme.header.patch(theme.thread_cursor))
            } else {
                Span::styled(" ", theme.header)
            }];
            let mut lines = vec![summary_line(theme, &layout, leading)];
            let Some(body_layout) = app.expanded_layout(id) else {
                tracing::error!(%id, "expanded thread has no prepared message layout");
                return lines;
            };
            lines.extend(expanded_lines(
                theme,
                thread,
                body_layout,
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
    let endpoints_rendered =
        app.menu_bar_shown() && menu_bar::bar_tail(app, width).endpoints_rendered();
    let badges: Vec<&String> = parts
        .badges
        .iter()
        .filter(|badge| !endpoints_rendered || badge.contains("stale") || badge.contains("error"))
        .collect();
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
        if directory.is_none()
            && view.changed()
            && app.diff_mode() != fathomable_core::config::DiffMode::Off
        {
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
        NoticeTone::Warning => theme.warning,
        NoticeTone::Error => theme.statusline_error,
    }
}

/// The status line's words (ADR 0010, amended by 0046's session): the
/// pill says one thing, the mode or the focused pane; the badges after
/// the path describe an active diff and its base (ADR 0060); the right block is
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
        Focus::Tree => "FILE LIST",
        Focus::Review => "THREADS",
        Focus::ThreadsPane => "THREAD LIST",
        Focus::View => match view.mode() {
            Mode::Normal | Mode::Select => "FILE",
            Mode::Command => "CMD",
            Mode::Search { .. } => "SRCH",
        },
    };
    let mut badges = Vec::new();
    if directory.is_none() {
        if let Some(diff) = view.diff() {
            badges.push(diff.badge.clone());
        }
        badges.push(app.comparison_badge());
    }
    let mut right = Vec::new();
    if directory.is_none() {
        let (line, col) = view.source_position();
        let mut position = format!(" {line}:{col}  {}%", view.percent());
        let counts = if app.diff_mode() == fathomable_core::config::DiffMode::Off {
            None
        } else if view.diff_view() {
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
pub(crate) fn which_key_grid(app: &App, sections: &[bindings::MenuSection]) -> HintGrid {
    HintGrid::bottom(
        sections,
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
    grid: &HintGrid,
    title: &str,
    sections: &[bindings::MenuSection],
    enabled: &[bool],
    hover: Option<usize>,
) {
    if grid.height < 2 || sections.is_empty() {
        return;
    }
    let area = hint_grid_rect(grid);
    let first_width = grid
        .columns
        .first()
        .map_or(grid.width.saturating_sub(2), |column| column.width);
    let title = if first_width >= 2 {
        let title = fit_ellipsis(title, first_width - 2).trim_end().to_owned();
        format!(" {title} ")
    } else {
        String::new()
    };
    let block = rounded_block(theme, Span::styled(title, theme.info), theme.menu);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    if grid.insufficient_space {
        let message = fit_ellipsis("Terminal too small", grid.width.saturating_sub(2));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(message, theme.info)))
                .alignment(Alignment::Center)
                .style(theme.menu),
            Rect {
                x: area.x.saturating_add(1),
                y: area.y.saturating_add(1),
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(2),
            },
        );
        return;
    }

    let entries = sections
        .iter()
        .flat_map(bindings::MenuSection::entries)
        .collect::<Vec<_>>();
    let border_style = theme.menu.patch(theme.info).add_modifier(Modifier::DIM);
    for row in 0..grid.rows {
        let cells = grid
            .columns
            .iter()
            .map(|column| column.cells[row])
            .collect::<Vec<_>>();
        let mut spans = Vec::with_capacity(grid.columns.len() * 4 + 1);
        spans.push(Span::styled(
            if matches!(cells.first(), Some(HintCell::Rule)) {
                "├"
            } else {
                "│"
            },
            border_style,
        ));
        for (column_index, (column, cell)) in grid.columns.iter().zip(&cells).enumerate() {
            spans.extend(hint_cell_spans(
                theme,
                column,
                *cell,
                &entries,
                enabled,
                hover,
                border_style,
            ));
            let next = cells.get(column_index + 1);
            let junction = hint_junction(*cell, next.copied());
            spans.push(Span::styled(junction, border_style));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(theme.menu),
            Rect {
                x: area.x,
                y: area.y.saturating_add(1 + u16_of(row)),
                width: area.width,
                height: 1,
            },
        );
    }
    for column in 1..grid.columns.len() {
        let Some(x) = grid.column_x(column).map(u16_of) else {
            continue;
        };
        for (y, glyph) in [(area.y, "┬"), (area.bottom().saturating_sub(1), "┴")] {
            frame.render_widget(
                Paragraph::new(Span::styled(glyph, border_style)),
                Rect {
                    x: x.saturating_sub(1),
                    y,
                    width: 1,
                    height: 1,
                },
            );
        }
    }
}

fn hint_cell_spans(
    theme: &Theme,
    column: &crate::app::input::menu::HintColumn,
    cell: HintCell,
    entries: &[&bindings::MenuEntry],
    enabled: &[bool],
    hover: Option<usize>,
    border_style: Style,
) -> Vec<Span<'static>> {
    match cell {
        HintCell::Rule => vec![Span::styled("─".repeat(column.width), border_style)],
        HintCell::Empty => vec![Span::styled(" ".repeat(column.width), theme.menu)],
        HintCell::Entry(index) => {
            let entry = entries[index];
            let row_style = if hover == Some(index) {
                theme.list_hover
            } else {
                Style::default()
            };
            let dim = if enabled.get(index).copied().unwrap_or(true) {
                Modifier::empty()
            } else {
                Modifier::DIM
            };
            let surface = theme.menu.patch(row_style).add_modifier(dim);
            let key = entry.key();
            let marker_style = if entry.active() {
                on_surface(surface, theme.popup_key).add_modifier(Modifier::BOLD)
            } else {
                surface
            };
            let mut spans = if column.label_width > 0 && entry.choice() {
                vec![Span::styled(
                    if entry.active() { "▌ " } else { "  " },
                    marker_style,
                )]
            } else {
                Vec::new()
            };
            spans.push(Span::styled(
                format!(
                    "{}{}",
                    " ".repeat(column.key_width.saturating_sub(display_width(&key))),
                    key
                ),
                theme
                    .menu
                    .patch(theme.info)
                    .patch(row_style)
                    .add_modifier(dim),
            ));
            if column.label_width > 0 {
                spans.push(Span::styled("  ", surface));
                spans.push(Span::styled(
                    fit_ellipsis(entry.label(), column.label_width),
                    surface,
                ));
            }
            let used = column.key_width
                + usize::from(column.label_width > 0)
                    * (2 + column.label_width + usize::from(entry.choice()) * 2);
            spans.push(Span::styled(
                " ".repeat(column.width.saturating_sub(used)),
                surface,
            ));
            spans
        }
    }
}

const fn hint_junction(cell: HintCell, next: Option<HintCell>) -> &'static str {
    match (matches!(cell, HintCell::Rule), next) {
        (true, Some(HintCell::Rule)) => "┼",
        (true, _) => "┤",
        (false, Some(HintCell::Rule)) => "├",
        (false, _) => "│",
    }
}

fn hint_grid_rect(grid: &HintGrid) -> Rect {
    Rect {
        x: u16_of(grid.x),
        y: u16_of(grid.y),
        width: u16_of(grid.width),
        height: u16_of(grid.height),
    }
}

fn grid_rect(grid: Grid) -> Rect {
    Rect {
        x: u16_of(grid.x),
        y: u16_of(grid.y),
        width: u16_of(grid.width),
        height: u16_of(grid.height),
    }
}

/// The context menu at the pointer (ADR 0050): its action labels at the
/// left and compact, subdued shortcuts at the right.
fn draw_context_menu(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    menu: &Menu,
    selected: Option<usize>,
) {
    let (width, _) = app.size();
    let grid = menu.grid_in(width, app.pane_top(), app.pane_rows());
    let hover = app
        .pointer()
        .and_then(|(column, row)| grid.entry_at(column, row));
    if grid.height < 2 || menu.entries().is_empty() {
        return;
    }
    let checkable = menu
        .entries()
        .iter()
        .any(|entry| entry.checked().is_some() || entry.active());
    let inner_width = grid.width.saturating_sub(2);
    let lines = menu
        .entries()
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if entry.is_separator() {
                return Line::from(Span::styled(
                    "─".repeat(inner_width),
                    theme.info.add_modifier(Modifier::DIM),
                ))
                .style(theme.menu);
            }
            let surface = if hover == Some(index) || selected == Some(index) {
                theme.menu.patch(theme.list_hover)
            } else {
                theme.menu
            };
            let check = if checkable {
                if entry.active() {
                    "▌ "
                } else if entry.checked() == Some(true) {
                    "✓ "
                } else {
                    "  "
                }
            } else {
                ""
            };
            let gap = inner_width
                .saturating_sub(
                    display_width(check)
                        + display_width(entry.label())
                        + display_width(entry.key()),
                )
                .max(1);
            let content = if entry.enabled() {
                surface
            } else {
                surface.patch(theme.info).add_modifier(Modifier::DIM)
            };
            let mark = if entry.active() {
                content.add_modifier(Modifier::BOLD)
            } else {
                content
            };
            Line::from(vec![
                Span::styled(check.to_owned(), mark),
                Span::styled(entry.label().to_owned(), content),
                Span::styled(" ".repeat(gap), surface),
                Span::styled(
                    entry.key().to_owned(),
                    on_surface(surface, theme.info).add_modifier(if entry.enabled() {
                        Modifier::empty()
                    } else {
                        Modifier::DIM
                    }),
                ),
            ])
            .style(surface)
        })
        .collect::<Vec<_>>();
    let area = grid_rect(grid);
    let block = rounded_block(
        theme,
        Span::styled(format!(" {} ", menu.title()), theme.info),
        theme.menu,
    );
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).style(theme.menu), inner);
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
            | super::PickerKind::ComparisonCommit
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
    rename: Option<Rect>,
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

    pub(crate) fn rename_at(self, column: usize, row: usize) -> bool {
        self.rename
            .is_some_and(|area| inside_rect(area, column, row))
    }
}

fn picker_layout_in(area: Rect, picker: &PickerState) -> PickerLayout {
    let width = area.width.saturating_sub(4).clamp(22, 90);
    let height = area.height.saturating_sub(2).clamp(3, 20);
    let popup = centred(area, width, height);
    let mut body = Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let rename = (picker.kind() == super::PickerKind::ComparisonReviewPoints).then(|| {
        let width = body.width.min(15);
        Rect {
            x: body.right().saturating_sub(width),
            y: body.bottom().saturating_sub(1),
            width,
            height: u16::from(body.height > 0),
        }
    });
    if rename.is_some() {
        body.height = body.height.saturating_sub(1);
    }
    let rows = usize::from(body.height);
    PickerLayout {
        popup,
        body,
        first: picker.first_visible(rows),
        rows,
        rename,
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
        super::PickerKind::ComparisonBase => "comparison base".to_owned(),
        super::PickerKind::ComparisonTarget => "comparison target".to_owned(),
        super::PickerKind::ComparisonCommit => "commit to compare with its parent".to_owned(),
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
        super::PickerKind::ReviewPointName => {
            "capture working tree and use as Base (name optional)".to_owned()
        }
        super::PickerKind::ReviewPointManage => "manage review points".to_owned(),
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
    if let Some(rename) = layout.rename
        && rename.height > 0
    {
        let hovered = app
            .and_then(App::pointer)
            .is_some_and(|(column, row)| layout.rename_at(column, row));
        frame.render_widget(
            Paragraph::new(fit("Ctrl-r rename", usize::from(rename.width))).style(if hovered {
                theme.popup_key
            } else {
                theme.info
            }),
            rename,
        );
    }
    let col = (2 + display_width(&title) + 3 + display_width(picker.input()))
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
    let layout = board_confirmation_layout(app, changed);
    let popup = layout.popup;
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
    ]);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
    draw_confirmation_controls(frame, theme, app, &layout);
}

fn draw_quit_confirmation(frame: &mut Frame<'_>, theme: &Theme, app: &App) {
    let layout = quit_confirmation_layout(app);
    let block = rounded_block(theme, " Quit ", theme.popup);
    let inner = block.inner(layout.popup);
    frame.render_widget(Clear, layout.popup);
    frame.render_widget(block, layout.popup);
    frame.render_widget(
        Paragraph::new(vec![Line::from("Quit Fathomable?"), Line::default()]).style(theme.popup),
        inner,
    );
    draw_confirmation_controls(frame, theme, app, &layout);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewPointActionHit {
    Rename,
    Delete,
    Back,
    Submit,
}

/// Geometry shared by the review-point action card and rename editor.
pub(crate) struct ReviewPointPopupLayout {
    pub(crate) popup: Rect,
    rename: Rect,
    delete: Rect,
    back: Rect,
    submit: Rect,
    input: Rect,
}

impl ReviewPointPopupLayout {
    pub(crate) fn contains(&self, column: usize, row: usize) -> bool {
        inside_rect(self.popup, column, row)
    }

    pub(crate) fn action_at(&self, column: usize, row: usize) -> Option<ReviewPointActionHit> {
        [
            (self.rename, ReviewPointActionHit::Rename),
            (self.delete, ReviewPointActionHit::Delete),
            (self.back, ReviewPointActionHit::Back),
            (self.submit, ReviewPointActionHit::Submit),
        ]
        .into_iter()
        .find_map(|(area, hit)| inside_rect(area, column, row).then_some(hit))
    }
}

pub(crate) fn review_point_action_layout(app: &App) -> ReviewPointPopupLayout {
    let popup = centred(confirmation_area(app), 72, 9);
    let inner = Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let controls_y = inner.bottom().saturating_sub(1);
    ReviewPointPopupLayout {
        popup,
        rename: Rect::new(
            inner.x,
            controls_y,
            inner.width.min(10),
            u16::from(inner.height > 0),
        ),
        delete: Rect::new(
            inner.x.saturating_add(12),
            controls_y,
            inner.width.saturating_sub(12).min(10),
            u16::from(inner.height > 0),
        ),
        back: Rect::new(
            inner.x.saturating_add(24),
            controls_y,
            inner.width.saturating_sub(24).min(10),
            u16::from(inner.height > 0),
        ),
        submit: Rect::default(),
        input: Rect::default(),
    }
}

pub(crate) fn review_point_rename_layout(app: &App) -> ReviewPointPopupLayout {
    let popup = centred(confirmation_area(app), 72, 6);
    let inner = Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let controls_y = inner.bottom().saturating_sub(1);
    ReviewPointPopupLayout {
        popup,
        rename: Rect::default(),
        delete: Rect::default(),
        back: Rect::new(
            inner.x.saturating_add(15),
            controls_y,
            inner.width.saturating_sub(15).min(10),
            u16::from(inner.height > 0),
        ),
        submit: Rect::new(
            inner.x,
            controls_y,
            inner.width.min(13),
            u16::from(inner.height > 0),
        ),
        input: Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            u16::from(inner.height > 1),
        ),
    }
}

fn draw_review_point_action(
    frame: &mut Frame<'_>,
    theme: &Theme,
    app: &App,
    action: &super::ReviewPointActionState,
) {
    let layout = review_point_action_layout(app);
    let point = action.point();
    let block = rounded_block(theme, " Review point ", theme.popup);
    let inner = block.inner(layout.popup);
    let name = super::review_points::review_point_name(point.name());
    let id = point.id().chars().take(12).collect::<String>();
    let head = point
        .head()
        .map(ToString::to_string)
        .map_or_else(|| "none".to_owned(), |head| head.chars().take(12).collect());
    let lines = vec![
        Line::from(format!(
            "Name     {}",
            fit_ellipsis(&name, usize::from(inner.width.saturating_sub(9)))
        )),
        Line::from(format!("ID       {id}")),
        Line::from(format!("Created  {}", format_time(point.created()))),
        Line::from(format!("HEAD     {head}")),
        Line::from(format!("Files    {}", point.entries().len())),
    ];
    frame.render_widget(Clear, layout.popup);
    frame.render_widget(block, layout.popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
    draw_review_point_actions(frame, theme, app, &layout, false);
}

fn draw_review_point_rename(
    frame: &mut Frame<'_>,
    theme: &Theme,
    app: &App,
    rename: &super::ReviewPointRenameState,
) {
    let layout = review_point_rename_layout(app);
    let block = rounded_block(theme, " Rename review point ", theme.popup);
    let inner = block.inner(layout.popup);
    let short = rename.expected().id().chars().take(8).collect::<String>();
    frame.render_widget(Clear, layout.popup);
    frame.render_widget(block, layout.popup);
    frame.render_widget(
        Paragraph::new(Line::from(format!("{short} · blank clears the name"))).style(theme.info),
        Rect {
            height: u16::from(inner.height > 0),
            ..inner
        },
    );
    if layout.input.height > 0 {
        let width = usize::from(layout.input.width);
        let cursor = rename.editor().cursor_cell(width);
        let rows = rename.editor().rows(width);
        let visible = rows
            .get(cursor.row)
            .map_or("", |row| rename.editor().row_text(*row));
        frame.render_widget(
            Paragraph::new(fit(visible, width)).style(theme.list_active),
            layout.input,
        );
        let column = cursor
            .column
            .min(usize::from(layout.input.width.saturating_sub(1)));
        frame.set_cursor_position((layout.input.x + u16_of(column), layout.input.y));
    }
    draw_review_point_actions(frame, theme, app, &layout, true);
}

fn draw_review_point_actions(
    frame: &mut Frame<'_>,
    theme: &Theme,
    app: &App,
    layout: &ReviewPointPopupLayout,
    rename: bool,
) {
    let hovered = app
        .pointer()
        .and_then(|(column, row)| layout.action_at(column, row));
    let mut draw = |area: Rect, text: &str, hit| {
        if area.height > 0 {
            frame.render_widget(
                Paragraph::new(fit(text, usize::from(area.width))).style(if hovered == Some(hit) {
                    theme.popup_key
                } else {
                    theme.info
                }),
                area,
            );
        }
    };
    if rename {
        draw(layout.submit, "Enter rename", ReviewPointActionHit::Submit);
        draw(layout.back, "Esc back", ReviewPointActionHit::Back);
    } else {
        draw(layout.rename, "r rename", ReviewPointActionHit::Rename);
        draw(layout.delete, "d delete", ReviewPointActionHit::Delete);
        draw(layout.back, "Esc back", ReviewPointActionHit::Back);
    }
}

fn draw_review_point_delete_confirmation(
    frame: &mut Frame<'_>,
    theme: &Theme,
    app: &App,
    id: &str,
    name: Option<&str>,
    created: u64,
) {
    let layout = review_point_delete_confirmation_layout(app);
    let block = rounded_block(theme, " Delete review point ", theme.popup);
    let inner = block.inner(layout.popup);
    let label = super::review_points::review_point_name(name);
    let short = id.chars().take(8).collect::<String>();
    let lines = vec![
        Line::from("Delete this repository-wide review point?"),
        Line::from(format!("{short} · {}", format_time(created))),
        Line::from(fit_ellipsis(&label, usize::from(inner.width))),
        Line::from("Threads keep their origin evidence; the snapshot will not."),
    ];
    frame.render_widget(Clear, layout.popup);
    frame.render_widget(block, layout.popup);
    frame.render_widget(Paragraph::new(lines).style(theme.popup), inner);
    draw_confirmation_controls(frame, theme, app, &layout);
}

fn draw_confirmation_controls(
    frame: &mut Frame<'_>,
    theme: &Theme,
    app: &App,
    layout: &ConfirmationLayout,
) {
    if layout.controls.height == 0 {
        return;
    }
    let hovered = app
        .pointer()
        .and_then(|(column, row)| layout.action_at(column, row));
    frame.render_widget(
        Paragraph::new(
            layout
                .spec
                .line(theme, usize::from(layout.controls.width), hovered),
        ),
        layout.controls,
    );
}

/// Geometry and shared hit logic for a compact confirmation.
pub(crate) struct ConfirmationLayout {
    pub(crate) popup: Rect,
    controls: Rect,
    spec: header::ConfirmationControls,
}

impl ConfirmationLayout {
    /// Whether the pointer is within the popup, including its frame.
    pub(crate) fn contains(&self, column: usize, row: usize) -> bool {
        inside_rect(self.popup, column, row)
    }

    /// The visible confirmation control under the pointer.
    pub(crate) fn action_at(&self, column: usize, row: usize) -> Option<Action> {
        if !inside_rect(self.controls, column, row) {
            return None;
        }
        self.spec.action_at(
            usize::from(self.controls.width),
            column - usize::from(self.controls.x),
        )
    }
}

/// Current compact Quit confirmation geometry.
pub(crate) fn quit_confirmation_layout(app: &App) -> ConfirmationLayout {
    confirmation_layout(app, 33, 5, 2, "quit")
}

/// Current Clear board confirmation geometry.
pub(crate) fn board_confirmation_layout(app: &App, changed: bool) -> ConfirmationLayout {
    let area = confirmation_area(app);
    let width = area.width.saturating_sub(4).clamp(42, 90);
    confirmation_layout(app, width, 7, usize::from(changed) + 2, "clear")
}

/// Current review-point deletion confirmation geometry.
pub(crate) fn review_point_delete_confirmation_layout(app: &App) -> ConfirmationLayout {
    let mut layout = confirmation_layout(app, 72, 7, 4, "delete");
    layout.spec = header::ConfirmationControls::with_key("y", "delete");
    layout
}

fn confirmation_layout(
    app: &App,
    width: u16,
    height: u16,
    controls_row: usize,
    verb: &'static str,
) -> ConfirmationLayout {
    let popup = centred(confirmation_area(app), width, height);
    let inner = Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let controls = if controls_row < usize::from(inner.height) {
        Rect {
            y: inner.y + u16_of(controls_row),
            height: 1,
            ..inner
        }
    } else {
        Rect {
            y: inner.y.saturating_add(inner.height),
            height: 0,
            ..inner
        }
    };
    ConfirmationLayout {
        popup,
        controls,
        spec: header::ConfirmationControls::new(verb),
    }
}

fn confirmation_area(app: &App) -> Rect {
    Rect {
        x: 0,
        y: u16_of(app.pane_top()),
        width: u16_of(app.size().0),
        height: u16_of(app.pane_rows()),
    }
}

fn inside_rect(area: Rect, column: usize, row: usize) -> bool {
    column >= usize::from(area.x)
        && column < usize::from(area.x.saturating_add(area.width))
        && row >= usize::from(area.y)
        && row < usize::from(area.y.saturating_add(area.height))
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
    let mut lines = vec![Line::default()];
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
    // The header, the entries between, and the key bar on the last row while
    // Threads retains focus (ADR 0059); one row shows the header alone.
    let header = review_header(app);
    let title_hovered = app.review().view == ReviewView::Board
        && app.pointer().is_some_and(|(column, row)| {
            row == app.pane_top()
                && column >= app.sidebar_width()
                && column - app.sidebar_width() < header.left_width()
        });
    let control_hovered = app.pointer().and_then(|(column, row)| {
        (row == app.pane_top() && column >= app.sidebar_width())
            .then(|| column - app.sidebar_width())
            .and_then(|column| header.control_at(width, column))
    });
    let mut lines = vec![header.line_with_header_hovers(
        theme,
        width,
        title_hovered,
        control_hovered,
        app.pane_has_navigation(Focus::Review),
    )];
    let footer_shown = app.focus() == Focus::Review;
    let body = rows.saturating_sub(1 + usize::from(footer_shown));
    let scroll = list.scroll().min(all.len().saturating_sub(body));
    for row in all.iter().skip(scroll).take(body) {
        let context = ListRender {
            theme,
            now,
            width,
            navigation: Navigation::for_pane(app, Focus::Review),
        };
        lines.push(list_row(&context, row));
    }
    if footer_shown && rows >= 2 {
        lines.resize_with(rows - 1, Line::default);
        let footer = review_footer(app, &entries);
        let footer_row = usize::from(area.y) + rows - 1;
        let hovered = app
            .pointer()
            .filter(|(_, row)| *row == footer_row)
            .and_then(|(column, _)| column.checked_sub(usize::from(area.x)))
            .and_then(|column| footer.action_at(width, column));
        lines.push(footer.line_with_action_hover(theme, width, hovered));
    }
    frame.render_widget(Paragraph::new(lines).style(theme.text), area);
}

struct ListRender<'a> {
    theme: &'a Theme,
    now: u64,
    width: usize,
    navigation: Navigation,
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
            let layout = entry_header(summary, now, true, width);
            let line = summary_line(theme, &layout, vec![selection.marker(theme), nest_span()]);
            selection.paint(theme, line)
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
    let Row::Stub {
        summary, selected, ..
    } = row
    else {
        return Line::from("");
    };
    let selection = navigation.selection(*selected);
    let layout = entry_header(summary, now, false, width);
    let line = summary_line(theme, &layout, vec![selection.marker(theme), nest_span()]);
    selection.paint(theme, line)
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
    use std::path::PathBuf;

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use fathomable_core::config::DiffMode;
    use fathomable_core::highlight::Highlighter;
    use fathomable_core::layout::{Layout, display_width};
    use fathomable_testing::git;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};

    use crate::app::Popup;
    use crate::app::input::bindings::Action;
    use crate::app::testing;
    use crate::app::threads::list::Row;
    use crate::app::view::Effect;

    use super::{
        ABOUT_ANCHOR, ListRender, Theme, about_area, fit, fit_ellipsis, format_age,
        format_age_short, format_time, list_row, picker_row_cells, quit_confirmation_layout,
        status_message_style,
    };

    fn toast_buffer(
        app: &crate::app::App,
        theme: &Theme,
        width: u16,
        height: u16,
    ) -> anyhow::Result<Buffer> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
        terminal.draw(|frame| {
            super::draw_toasts(frame, app, theme, Rect::new(0, 0, width, height));
        })?;
        Ok(terminal.backend().buffer().clone())
    }

    fn buffer_row(buffer: &Buffer, row: u16) -> String {
        (0..buffer.area.width)
            .map(|column| buffer[(column, row)].symbol())
            .collect()
    }

    fn welcome_screen(app: &crate::app::App) -> anyhow::Result<Vec<String>> {
        let (width, height) = app.size();
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
            u16::try_from(width)?,
            u16::try_from(height)?,
        ))?;
        terminal.draw(|frame| super::draw(frame, app, &theme))?;
        let buffer = terminal.backend().buffer();
        Ok((0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect())
    }

    #[test]
    fn too_small_warning_replaces_panes_and_resize_restores_them() -> anyhow::Result<()> {
        let dir = testing::workspace("pane-size-warning", testing::README)?;
        let mut app = testing::source_app(&dir)?;
        app.window_files();
        let focus = app.focus();
        let cursor = app.tree().map(fathomable_core::tree::Tree::cursor);
        let (minimum_width, minimum_height) = app.minimum_pane_size();
        app.resize(minimum_width.max(60), minimum_height.saturating_sub(1));
        assert!(!app.panes_fit());

        let buffer = testing::buffer(&app)?;
        let pane_end = u16::try_from(app.size().1)?.saturating_sub(1);
        let screen = (u16::try_from(app.pane_top())?..pane_end)
            .map(|row| buffer_row(&buffer, row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("Terminal too small"), "{screen:?}");
        assert!(!screen.contains("README.md"), "{screen:?}");
        assert_eq!(app.focus(), focus);
        assert_eq!(app.tree().map(fathomable_core::tree::Tree::cursor), cursor);

        app.resize(100, 30);
        assert!(app.panes_fit());
        let restored = testing::buffer(&app)?;
        let screen = (0..restored.area.height)
            .map(|row| buffer_row(&restored, row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!screen.contains("Terminal too small"), "{screen:?}");
        assert!(screen.contains("README.md"), "{screen:?}");
        assert_eq!(app.focus(), focus);
        assert_eq!(app.tree().map(fathomable_core::tree::Tree::cursor), cursor);

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        for menu_shown in [false, true] {
            if app.menu_bar_shown() != menu_shown {
                app.toggle_menu_bar();
            }
            app.resize(1, 1);
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(1, 1))?;
            terminal.draw(|frame| super::draw(frame, &app, &theme))?;
            assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "T");
            terminal.draw(|frame| {
                super::draw_size_warning(frame, &app, &theme, Rect::new(0, 0, 0, 0));
            })?;
        }
        Ok(())
    }

    fn confirmation_cells(
        layout: &super::ConfirmationLayout,
        action: Action,
    ) -> Vec<(usize, usize)> {
        let mut cells = Vec::new();
        for row in usize::from(layout.popup.y)..usize::from(layout.popup.bottom()) {
            for column in usize::from(layout.popup.x)..usize::from(layout.popup.right()) {
                if layout.action_at(column, row) == Some(action) {
                    cells.push((column, row));
                }
            }
        }
        cells
    }

    #[test]
    fn file_edit_toast_uses_popup_surface_and_diff_colours() -> anyhow::Result<()> {
        let dir = testing::workspace("file-toast-style", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.push_file_edit_toast(PathBuf::from("README +9 -8.md"), (3, 1));
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let buffer = toast_buffer(&app, &theme, 30, 5)?;

        assert_eq!(buffer_row(&buffer, 4), "      README +9 -8.md  +3  -1 ");
        assert_eq!(Some(buffer[(6, 4)].fg), theme.popup.fg);
        assert_eq!(
            Some(buffer[(13, 4)].fg),
            theme.popup.fg,
            "count-like filename text stays part of the path"
        );
        assert_eq!(Some(buffer[(23, 4)].fg), theme.diff_plus.fg);
        assert_eq!(Some(buffer[(27, 4)].fg), theme.diff_minus.fg);
        for column in 5..30 {
            assert_eq!(Some(buffer[(column, 4)].bg), theme.popup.bg);
        }
        Ok(())
    }

    #[test]
    fn file_edit_toasts_omit_zero_counts() -> anyhow::Result<()> {
        let dir = testing::workspace("file-toast-zero", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.push_file_edit_toast(PathBuf::from("README.md"), (0, 2));
        app.push_file_edit_toast(PathBuf::from("README.md"), (4, 0));
        app.push_file_edit_toast(PathBuf::from("README.md"), (0, 0));
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let buffer = toast_buffer(&app, &theme, 24, 3)?;
        let rows: Vec<String> = (0..3)
            .map(|row| buffer_row(&buffer, row).trim().to_owned())
            .collect();

        assert_eq!(rows, ["README.md  -2", "README.md  +4", "README.md"]);
        assert!(
            rows.iter()
                .all(|row| !row.contains("+0") && !row.contains("-0"))
        );
        Ok(())
    }

    #[test]
    fn file_edit_toast_clips_to_the_pane_width() -> anyhow::Result<()> {
        let dir = testing::workspace("file-toast-clip", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.push_file_edit_toast(PathBuf::from("very-long-file.rs"), (30, 12));
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let buffer = toast_buffer(&app, &theme, 12, 2)?;

        assert_eq!(buffer_row(&buffer, 0), "            ");
        assert_eq!(buffer_row(&buffer, 1), " very-long-f");
        Ok(())
    }

    #[test]
    fn off_hides_retained_file_edit_toasts_but_not_plain_notices() -> anyhow::Result<()> {
        let dir = testing::workspace("file-toast-off", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.push_file_edit_toast(PathBuf::from("live-secret.rs"), (7, 4));
        app.push_toast("ordinary notice".to_owned());
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);

        app.select_diff_mode(DiffMode::Off);
        app.settle_background();
        let buffer = toast_buffer(&app, &theme, 40, 4)?;
        let off = (0..4)
            .map(|row| buffer_row(&buffer, row))
            .collect::<String>();
        assert!(off.contains("ordinary notice"), "{off:?}");
        assert!(!off.contains("live-secret"), "{off:?}");

        app.select_diff_mode(DiffMode::Standard);
        app.settle_background();
        let buffer = toast_buffer(&app, &theme, 40, 4)?;
        let active = (0..4)
            .map(|row| buffer_row(&buffer, row))
            .collect::<String>();
        assert!(active.contains("live-secret.rs"), "{active:?}");
        Ok(())
    }

    #[test]
    fn status_provenance_follows_actual_endpoint_layout() -> anyhow::Result<()> {
        let dir = testing::workspace("status-endpoint-layout", testing::README)?;
        let root = testing::root(&dir);
        git::init(&root)?;
        git::commit_and_stage(&root, &[("README.md", testing::README)])?;
        let long_tag =
            "target-name-too-long-for-the-menu-bar-control-even-when-only-target-is-visible";
        git::tag(&root, long_tag)?;
        let mut app = testing::AppBuilder::new(&dir)
            .options(|mut options| {
                options.menu_bar = true;
                options
            })
            .build()?;

        let screen = testing::screen(&app)?;
        assert!(!screen.last().unwrap_or(&String::new()).contains("CMP"));
        app.view_mut().toggle_source_view();
        let screen = testing::screen(&app)?;
        assert!(!screen.last().unwrap_or(&String::new()).contains("SRC"));
        assert!(!screen.last().unwrap_or(&String::new()).contains("CMP"));
        app.view_mut().toggle_source_view();

        app.toggle_menu_bar();
        let screen = testing::screen(&app)?;
        assert!(
            screen.last().is_some_and(|row| row.contains("CMP ")),
            "{screen:?}"
        );

        app.toggle_menu_bar();
        let target = app
            .resolve_comparison_endpoint(&format!("refs/tags/{long_tag}"))
            .map_err(anyhow::Error::msg)?;
        app.set_comparison_target_aliased(
            target,
            Some(crate::app::comparison::EndpointAlias::Tag(
                long_tag.to_owned(),
            )),
        );
        app.settle_background();
        assert!(
            !crate::app::menu_bar::bar_tail(&app, app.size().0).endpoints_rendered(),
            "the long endpoint must exercise the status fallback"
        );
        let screen = testing::screen(&app)?;
        assert!(
            screen.last().is_some_and(|row| row.contains("CMP ")),
            "{screen:?}"
        );

        app.select_diff_mode(DiffMode::Off);
        app.settle_background();
        let screen = testing::screen(&app)?;
        assert!(
            screen
                .last()
                .is_some_and(|row| row.contains("OFF Target Tag")),
            "{screen:?}"
        );
        Ok(())
    }

    #[test]
    fn generic_toast_stays_plain_even_when_it_looks_like_counts() -> anyhow::Result<()> {
        let dir = testing::workspace("generic-toast", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.push_toast("settings +3 -1".to_owned());
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let buffer = toast_buffer(&app, &theme, 24, 2)?;

        assert_eq!(buffer_row(&buffer, 1), "         settings +3 -1 ");
        for column in 8..24 {
            assert_eq!(Some(buffer[(column, 1)].fg), theme.popup.fg);
        }
        Ok(())
    }

    #[test]
    fn toasts_stack_oldest_to_newest_and_keep_only_three() -> anyhow::Result<()> {
        let dir = testing::workspace("toast-stack", testing::README)?;
        let mut app = testing::app(&dir)?;
        for text in ["first", "second", "third", "fourth"] {
            app.push_toast(text.to_owned());
        }
        assert_eq!(
            app.toasts()
                .iter()
                .map(crate::app::Toast::text)
                .collect::<Vec<_>>(),
            ["second", "third", "fourth"]
        );
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let buffer = toast_buffer(&app, &theme, 16, 5)?;
        let rows: Vec<String> = (2..5)
            .map(|row| buffer_row(&buffer, row).trim().to_owned())
            .collect();

        assert_eq!(rows, ["second", "third", "fourth"]);
        Ok(())
    }

    #[test]
    fn about_anchor_renders_exactly_in_the_subdued_info_style() -> anyhow::Result<()> {
        let dir = testing::workspace("about-anchor", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.command("about");
        let area = about_area(&app);
        assert_eq!((area.width, area.height), (64, 12));
        let buffer = testing::buffer(&app)?;
        let actual = (area.y..area.y + 11)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let expected = [
            "╭ About ──────────────────────────────────────────────────)────╮",
            "│Fathomable 0.1.0                                        (     │",
            "│A read-only workspace viewer for reviewing               )    │",
            "│diffs and interactive comment threads with agents       (     │",
            "│via MCP.                                               _|_    │",
            "│                                                      (   )   │",
            "│License  MIT (Fathomable)                          ====`|'====│",
            "│Source   https://github.com/hbeberman/fathomable        |     │",
            "│Third-party notices: Help > Licenses               |\\   |   /|│",
            "│                                                   \\'-._|_.-'/│",
            "│Esc close                                            `-\\|/-'  │",
        ];
        assert_eq!(actual, expected);

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let anchor_x = area.x + 1 + 50;
        for (row, anchor) in ABOUT_ANCHOR.iter().enumerate() {
            for (column, _ch) in anchor.chars().enumerate().filter(|(_, ch)| *ch != ' ') {
                let cell = &buffer[(
                    anchor_x + u16::try_from(column)?,
                    area.y + 1 + u16::try_from(row)?,
                )];
                assert_eq!(Some(cell.fg), theme.info.fg);
            }
        }
        assert_eq!(Some(buffer[(area.x + 58, area.y)].fg), theme.info.fg);
        Ok(())
    }

    #[test]
    fn quit_confirmation_renders_and_updates_whole_control_hover() -> anyhow::Result<()> {
        let dir = testing::workspace("quit-confirmation", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.request_quit();
        let layout = quit_confirmation_layout(&app);
        assert_eq!((layout.popup.width, layout.popup.height), (33, 5));
        let buffer = testing::buffer(&app)?;
        let actual = (layout.popup.y..layout.popup.bottom())
            .map(|y| {
                (layout.popup.x..layout.popup.right())
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                "╭ Quit ─────────────────────────╮",
                "│Quit Fathomable?               │",
                "│                               │",
                "│quit Enter · cancel Esc        │",
                "╰───────────────────────────────╯",
            ]
        );

        let confirm = confirmation_cells(&layout, Action::Confirm);
        let cancel = confirmation_cells(&layout, Action::Escape);
        assert_eq!(confirm.len(), "quit Enter".len());
        assert_eq!(cancel.len(), "cancel Esc".len());

        let moved = |column: usize, row: usize| MouseEvent {
            kind: MouseEventKind::Moved,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        };
        crate::app::input::mouse::handle_mouse(&mut app, moved(confirm[0].0, confirm[0].1));
        let hovered = testing::buffer(&app)?;
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        for &(column, row) in &confirm {
            assert_eq!(
                Some(hovered[(u16::try_from(column)?, u16::try_from(row)?)].bg),
                theme.list_hover.bg
            );
        }
        for &(column, row) in &cancel {
            assert_ne!(
                Some(hovered[(u16::try_from(column)?, u16::try_from(row)?)].bg),
                theme.list_hover.bg
            );
        }
        crate::app::input::mouse::handle_mouse(
            &mut app,
            moved(
                usize::from(layout.popup.x) + 1,
                usize::from(layout.popup.y) + 1,
            ),
        );
        let cleared = testing::buffer(&app)?;
        for &(column, row) in confirm.iter().chain(&cancel) {
            assert_ne!(
                Some(cleared[(u16::try_from(column)?, u16::try_from(row)?)].bg),
                theme.list_hover.bg
            );
        }
        Ok(())
    }

    #[test]
    fn quit_confirmation_controls_click_and_outside_click_is_consumed() -> anyhow::Result<()> {
        let dir = testing::workspace("quit-confirmation-mouse", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.request_quit();
        let layout = quit_confirmation_layout(&app);
        let confirm = confirmation_cells(&layout, Action::Confirm);
        let click = |column: usize, row: usize| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            crate::app::input::mouse::handle_mouse(&mut app, click(confirm[0].0, confirm[0].1)),
            Effect::Quit
        );

        app.request_quit();
        let layout = quit_confirmation_layout(&app);
        let cancel = (usize::from(layout.popup.x)..usize::from(layout.popup.right()))
            .find_map(|column| {
                (usize::from(layout.popup.y)..usize::from(layout.popup.bottom()))
                    .find(|&row| layout.action_at(column, row) == Some(Action::Escape))
                    .map(|row| (column, row))
            })
            .ok_or_else(|| anyhow::anyhow!("visible cancel control"))?;
        assert_eq!(
            crate::app::input::mouse::handle_mouse(&mut app, click(cancel.0, cancel.1)),
            Effect::None
        );
        assert!(app.popup().is_none());

        app.window_files();
        assert_eq!(app.focus(), crate::app::Focus::Tree);
        app.request_quit();
        let layout = quit_confirmation_layout(&app);
        assert_eq!(
            crate::app::input::mouse::handle_mouse(
                &mut app,
                click(
                    usize::from(layout.popup.x) + 1,
                    usize::from(layout.popup.y) + 1
                )
            ),
            Effect::None
        );
        assert!(matches!(app.popup(), Some(Popup::ConfirmQuit)));
        assert_eq!(
            crate::app::input::mouse::handle_mouse(
                &mut app,
                click(
                    usize::from(layout.popup.right()),
                    usize::from(layout.popup.y)
                )
            ),
            Effect::None
        );
        assert!(app.popup().is_none());
        assert_eq!(
            app.focus(),
            crate::app::Focus::Tree,
            "outside click is consumed"
        );
        Ok(())
    }

    #[test]
    fn clear_board_controls_share_action_first_hover_and_hit_regions() -> anyhow::Result<()> {
        let dir = testing::workspace("clear-board-controls", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.start_new_comment();
        app.compose_insert("thread");
        app.compose_submit();
        app.request_clear_board();
        let layout = super::board_confirmation_layout(&app, false);
        let confirm = confirmation_cells(&layout, Action::Confirm);
        let cancel = confirmation_cells(&layout, Action::Escape);
        assert_eq!(confirm.len(), "clear Enter".len());
        assert_eq!(cancel.len(), "cancel Esc".len());

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let move_to = |(column, row): (usize, usize)| MouseEvent {
            kind: MouseEventKind::Moved,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        };
        for (hovered_action, cells, other) in [
            (Action::Confirm, &confirm, &cancel),
            (Action::Escape, &cancel, &confirm),
        ] {
            crate::app::input::mouse::handle_mouse(&mut app, move_to(cells[0]));
            let buffer = testing::buffer(&app)?;
            assert_eq!(
                layout.action_at(cells[0].0, cells[0].1),
                Some(hovered_action)
            );
            for &(column, row) in cells {
                assert_eq!(
                    Some(buffer[(u16::try_from(column)?, u16::try_from(row)?)].bg),
                    theme.list_hover.bg
                );
            }
            for &(column, row) in other {
                assert_ne!(
                    Some(buffer[(u16::try_from(column)?, u16::try_from(row)?)].bg),
                    theme.list_hover.bg
                );
            }
        }

        let buffer = testing::buffer(&app)?;
        let row = (layout.popup.x..layout.popup.right())
            .map(|column| buffer[(column, layout.popup.y + 3)].symbol().to_owned())
            .collect::<String>();
        assert!(row.starts_with("│clear Enter · cancel Esc"));
        assert!(row.ends_with('│'));
        Ok(())
    }

    #[test]
    fn narrow_confirmations_clip_without_phantom_hit_regions() -> anyhow::Result<()> {
        let dir = testing::workspace("quit-confirmation-narrow", testing::README)?;
        let mut seeded = testing::app(&dir)?;
        seeded.start_new_comment();
        seeded.compose_insert("thread");
        seeded.compose_submit();
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        for width in [1_usize, 2, 10, 24, 26, 33] {
            let mut app = testing::AppBuilder::new(&dir).width(width).build()?;
            app.request_quit();
            let layout = quit_confirmation_layout(&app);
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
                u16::try_from(width)?,
                30,
            ))?;
            terminal.draw(|frame| super::draw(frame, &app, &theme))?;
            for row in 0..30 {
                for column in 0..width {
                    if layout.action_at(column, row).is_some() {
                        assert!(layout.contains(column, row));
                    }
                }
            }
            assert_eq!(layout.action_at(width, 0), None);

            app.close_popup();
            app.request_clear_board();
            let layout = super::board_confirmation_layout(&app, false);
            terminal.draw(|frame| super::draw(frame, &app, &theme))?;
            for row in 0..30 {
                for column in 0..width {
                    if layout.action_at(column, row).is_some() {
                        assert!(layout.contains(column, row));
                    }
                }
            }
            assert_eq!(layout.action_at(width, 0), None);
        }
        Ok(())
    }

    #[test]
    fn about_source_opens_and_an_outside_click_dismisses() -> anyhow::Result<()> {
        let dir = testing::workspace("about-mouse", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.command("about");
        let area = about_area(&app);
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            crate::app::input::mouse::handle_mouse(&mut app, click(area.x + 10, area.y + 7),),
            Effect::Open("https://github.com/hbeberman/fathomable".to_owned())
        );
        assert!(app.popup().is_none());

        app.command("about");
        assert!(matches!(app.popup(), Some(Popup::About)));
        assert_eq!(
            crate::app::input::mouse::handle_mouse(&mut app, click(area.x - 1, area.y),),
            Effect::None
        );
        assert!(app.popup().is_none());
        Ok(())
    }

    #[test]
    fn narrow_about_omits_anchor_without_covering_source_link() -> anyhow::Result<()> {
        let dir = testing::workspace("about-anchor-narrow", testing::README)?;
        let mut app = testing::AppBuilder::new(&dir).width(56).build()?;
        app.command("about");
        let area = about_area(&app);
        assert_eq!((area.width, area.height), (52, 12));

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(56, 30))?;
        terminal.draw(|frame| super::draw(frame, &app, &theme))?;
        let buffer = terminal.backend().buffer();
        let actual = (area.y + 1..area.y + 11)
            .map(|y| {
                (area.x + 1..area.x + area.width - 1)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        let expected = [
            "Fathomable 0.1.0",
            "A read-only workspace viewer for reviewing",
            "diffs and interactive comment threads with agents",
            "via MCP.",
            "",
            "License  MIT (Fathomable)",
            "Source   https://github.com/hbeberman/fathomable",
            "Third-party notices: Help > Licenses",
            "",
            "Esc close",
        ];
        assert_eq!(actual, expected);

        let former_anchor_column = area.x + 46;
        let source_row = area.y + 7;
        assert_eq!(buffer[(former_anchor_column, source_row)].symbol(), "b");
        assert_eq!(
            Some(buffer[(former_anchor_column, source_row)].fg),
            theme.link.fg
        );
        assert_eq!(buffer[(former_anchor_column, area.y)].symbol(), "─");

        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: former_anchor_column,
            row: source_row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            crate::app::input::mouse::handle_mouse(&mut app, click),
            Effect::Open("https://github.com/hbeberman/fathomable".to_owned())
        );
        Ok(())
    }

    fn assert_complete_welcome(app: &crate::app::App) -> anyhow::Result<()> {
        let rows = welcome_screen(app)?;
        let rows = rows
            .iter()
            .map(|row| row.trim())
            .filter(|row| !row.is_empty())
            .collect::<Vec<_>>();
        let root = app.workspace().root().display().to_string();
        let root = super::truncate_left(&root, 55.min(app.size().0));
        let expected = [
            "Fathomable",
            root.as_str(),
            "A read-only workspace viewer for reviewing diffs and",
            "interactive comment threads with agents via MCP.",
            "Usable with both mouse (right/left click) and keyboard.",
            "Space opens a hotkey list.",
            "Alt-Space moves keyboard focus to the menu bar.",
            "Pane navigation",
            "f          File pane",
            "F          File list",
            "t          Threads pane",
            "T          Threads list",
            "w/W        cycle pane focus",
            "q          quit",
            "Diff controls",
            "J/K        next/previous change",
            "L/H        next/previous changed file",
            "Space d d  show uncommitted changes",
            "Space d l  show latest commit",
            "Space d c  show a specific commit",
            "Comment controls",
            "Tab/⇧Tab   next/previous comment thread",
            "c          add a comment",
            "r          resolve/reopen thread",
        ];
        let start = rows
            .iter()
            .position(|row| *row == expected[0])
            .ok_or_else(|| anyhow::anyhow!("welcome was not rendered"))?;
        assert_eq!(&rows[start..start + expected.len()], expected);

        let screen = rows.join("\n");
        assert!(!screen.contains("Space opens a hotkey list. Alt-Space"));
        assert!(!screen.contains(" / "));
        assert!(!screen.contains(&app.viewer_label()));
        Ok(())
    }

    #[test]
    fn startup_welcome_renders_every_shortcut_at_normal_size() -> anyhow::Result<()> {
        let dir = testing::workspace("welcome-shortcuts", testing::README)?;
        let app = testing::AppBuilder::new(&dir).unopened().build()?;
        assert_complete_welcome(&app)
    }

    #[test]
    fn getting_started_welcome_renders_every_shortcut_at_normal_size() -> anyhow::Result<()> {
        let dir = testing::workspace("getting-started-shortcuts", testing::README)?;
        let mut app = testing::AppBuilder::new(&dir).build()?;
        app.command("help");
        assert!(app.getting_started());
        assert_complete_welcome(&app)
    }

    #[test]
    fn short_wide_welcome_reflows_control_sections_without_clipping() -> anyhow::Result<()> {
        let dir = testing::workspace("welcome-short-wide", testing::README)?;
        let mut app = testing::AppBuilder::new(&dir).unopened().build()?;
        app.resize(100, 20);
        let rows = welcome_screen(&app)?;
        let pane_row = rows
            .iter()
            .position(|row| row.contains("Pane navigation"))
            .ok_or_else(|| anyhow::anyhow!("Pane navigation was not rendered"))?;
        assert!(rows[pane_row].contains("Diff controls"), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains("Comment controls")));
        for text in [
            "Space d d  show uncommitted changes",
            "Space d l  show latest commit",
            "Space d c  show a specific commit",
            "Tab/⇧Tab  next/previous comment thread",
            "r         resolve/reopen thread",
        ] {
            assert!(
                rows.iter().any(|row| row.contains(text)),
                "{text:?} was clipped: {rows:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn narrow_welcome_wraps_intro_and_keeps_single_column_controls() -> anyhow::Result<()> {
        let dir = testing::workspace("welcome-narrow", testing::README)?;
        let mut app = testing::AppBuilder::new(&dir).unopened().build()?;
        app.resize(60, 36);
        let rows = welcome_screen(&app)?;
        let headings = ["Pane navigation", "Diff controls", "Comment controls"]
            .map(|heading| {
                rows.iter()
                    .position(|row| row.contains(heading))
                    .ok_or_else(|| anyhow::anyhow!("{heading} was not rendered"))
            })
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()?;
        assert!(headings.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(rows.iter().any(|row| row.contains("and keyboard.")));
        assert!(rows.iter().any(|row| row.contains("to the menu bar.")));
        assert!(
            rows.iter()
                .any(|row| row.contains("Tab/⇧Tab   next/previous comment thread")),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("r          resolve/reopen thread")),
            "{rows:?}"
        );
        Ok(())
    }

    fn assert_welcome_too_small(app: &crate::app::App) -> anyhow::Result<()> {
        let rows = welcome_screen(app)?;
        assert!(
            rows.iter().any(|row| row.contains("Terminal too small")),
            "sidebar={}, column={}: {rows:?}",
            app.sidebar_width(),
            app.column_width()
        );
        assert!(rows.iter().any(|row| row.contains("Resize, use Layout")));
        assert!(!rows.iter().any(|row| row.contains("Pane navigation")));
        assert!(!rows.iter().any(|row| row.contains("Comment controls")));
        Ok(())
    }

    #[test]
    fn narrow_tall_startup_welcome_uses_terminal_too_small_treatment() -> anyhow::Result<()> {
        let dir = testing::workspace("welcome-narrow-tall-startup", testing::README)?;
        let mut app = testing::AppBuilder::new(&dir).unopened().build()?;
        app.window_files();
        app.resize(40, 40);
        assert!(app.panes_fit(), "exercise the welcome-specific fallback");
        assert_welcome_too_small(&app)
    }

    #[test]
    fn narrow_tall_getting_started_welcome_uses_terminal_too_small_treatment() -> anyhow::Result<()>
    {
        let dir = testing::workspace("welcome-narrow-tall-help", testing::README)?;
        let mut app = testing::AppBuilder::new(&dir).build()?;
        app.window_files();
        app.resize(40, 40);
        app.command("help");
        assert!(app.getting_started());
        assert!(app.panes_fit(), "exercise the welcome-specific fallback");
        assert_welcome_too_small(&app)
    }

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
    fn status_messages_use_their_tone_faces() -> anyhow::Result<()> {
        let dir = testing::workspace("status-error", testing::README)?;
        let mut app = testing::app(&dir)?;
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let mut theme = Theme::from_core(&core);
        theme.info = Style::default().fg(Color::Blue);
        theme.warning = Style::default().fg(Color::Yellow);
        theme.statusline_error = Style::default().fg(Color::Red);

        app.notice("ordinary");
        assert_eq!(status_message_style(&app, &theme), theme.info);
        app.startup_warning("shared");
        assert_eq!(status_message_style(&app, &theme), theme.warning);
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
