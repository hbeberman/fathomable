use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::layout::display_width;
use fathomable_testing::git;

use super::{Scroll, content_size, line_count, status_lines, wrap};
use crate::app::input::mouse;
use crate::app::{App, Popup, draw, testing};

fn scroll(app: &App) -> anyhow::Result<&Scroll> {
    match app.popup() {
        Some(Popup::Status(scroll)) => Ok(scroll),
        Some(Popup::Doctor(doctor)) => Ok(&doctor.scroll),
        _ => anyhow::bail!("a report is not open"),
    }
}

fn send_mouse(app: &mut App, kind: MouseEventKind) {
    mouse::handle_mouse(
        app,
        MouseEvent {
            kind,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        },
    );
}

fn screen(app: &App) -> anyhow::Result<Vec<String>> {
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = draw::Theme::from_core(&core);
    let (width, height) = app.size();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
        u16::try_from(width)?,
        u16::try_from(height)?,
    ))?;
    terminal.draw(|frame| draw::draw(frame, app, &theme))?;
    let buffer = terminal.backend().buffer();
    Ok((0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect())
}

#[test]
fn status_scrolls_with_doctor_navigation_and_shows_overflow() -> anyhow::Result<()> {
    let dir = testing::workspace("status-scroll", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.resize(100, 12);
    app.command("status");
    let (_, height) = content_size(&app);
    let start = screen(&app)?;
    assert!(start.join("\n").contains("Status · j/k/↑↓/wheel scroll"));
    assert!(!start.join("\n").contains("any key closes"));
    assert!(start[app.pane_top() + app.pane_rows() - 1].contains('↓'));
    let position = app.view().source_position();
    let view_scroll = app.view().scroll();

    testing::press(&mut app, "j");
    assert_eq!(scroll(&app)?.offset, 1);
    testing::press_key(&mut app, KeyCode::Down);
    assert_eq!(scroll(&app)?.offset, 2);
    testing::press(&mut app, "k");
    assert_eq!(scroll(&app)?.offset, 1);
    testing::press_key(&mut app, KeyCode::Up);
    assert_eq!(scroll(&app)?.offset, 0);
    send_mouse(&mut app, MouseEventKind::ScrollDown);
    assert_eq!(scroll(&app)?.offset, 3);
    assert_ne!(screen(&app)?, start);
    send_mouse(&mut app, MouseEventKind::ScrollUp);
    assert_eq!(scroll(&app)?.offset, 0);
    testing::press_key(&mut app, KeyCode::PageDown);
    assert_eq!(
        scroll(&app)?.offset,
        height.min(line_count(&app, content_size(&app).0) - height)
    );
    testing::press_key(&mut app, KeyCode::PageUp);
    assert_eq!(scroll(&app)?.offset, 0);
    testing::press_key(&mut app, KeyCode::End);
    let end = scroll(&app)?.offset;
    let bottom = screen(&app)?;
    assert!(bottom.join("\n").contains("thread count"));
    let footer = &bottom[app.pane_top() + app.pane_rows() - 1];
    assert!(footer.contains('↑'));
    assert!(!footer.contains('↓'));
    testing::press(&mut app, "jjjG");
    send_mouse(&mut app, MouseEventKind::ScrollDown);
    assert_eq!(scroll(&app)?.offset, end);
    testing::press_key(&mut app, KeyCode::Home);
    testing::press(&mut app, "kkkg");
    send_mouse(&mut app, MouseEventKind::ScrollUp);
    assert_eq!(scroll(&app)?.offset, 0);
    assert_eq!(app.view().source_position(), position);
    assert_eq!(app.view().scroll(), view_scroll);
    Ok(())
}

#[test]
fn status_dismisses_only_on_escape_or_outside_click_and_keeps_panes_unchanged() -> anyhow::Result<()>
{
    let dir = testing::workspace("status-modal", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    testing::press(&mut app, "jj");
    let path = app.current_path().to_owned();
    let position = app.view().source_position();
    let focus = app.focus();
    app.command("status");
    testing::press(&mut app, "ddcqr:wFt");
    testing::press_key(&mut app, KeyCode::Enter);
    assert!(matches!(app.popup(), Some(Popup::Status(_))));
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        send_mouse(&mut app, kind);
        assert!(matches!(app.popup(), Some(Popup::Status(_))));
    }
    assert_eq!(app.current_path(), path);
    assert_eq!(app.view().text(), testing::README);
    assert_eq!(app.view().source_position(), position);
    assert_eq!(app.focus(), focus);

    testing::click(&mut app, 0, 5);
    assert!(app.popup().is_none());
    assert_eq!(app.focus(), focus);
    assert_eq!(app.view().source_position(), position);
    app.command("status");
    testing::press_key(&mut app, KeyCode::Esc);
    assert!(app.popup().is_none());
    Ok(())
}

#[test]
fn status_rewraps_and_clamps_on_resize_and_keeps_live_values() -> anyhow::Result<()> {
    let dir = testing::workspace("status-resize", testing::README)?;
    let mut app = testing::app(&dir)?;
    app.thread_updates_degraded = Some(
        (0..40)
            .map(|index| format!("report row {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    app.command("status");
    testing::press_key(&mut app, KeyCode::End);
    assert!(scroll(&app)?.offset > 0);
    app.thread_updates_degraded = Some("latest report".to_owned());
    assert!(screen(&app)?.join("\n").contains("latest report"));
    for (width, height) in [(35, 18), (8, 5), (1, 1), (100, 40)] {
        app.resize(width, height);
        let (content_width, content_height) = content_size(&app);
        assert!(
            scroll(&app)?.offset <= line_count(&app, content_width).saturating_sub(content_height)
        );
        screen(&app)?;
        testing::press(&mut app, "q");
        assert!(matches!(app.popup(), Some(Popup::Status(_))));
    }
    assert_eq!(scroll(&app)?.offset, 0);
    assert!(screen(&app)?.join("\n").contains("latest report"));
    Ok(())
}

#[test]
fn status_values_preserve_logical_lines_and_wrap_paths_by_display_width() -> anyhow::Result<()> {
    let dir = testing::workspace("status-wrap", testing::README)?;
    let mut app = testing::app(&dir)?;
    let path = format!("/長い/e\u{301}/{}終", "directory/".repeat(15));
    app.thread_updates_degraded = Some(format!("{path}\nworktree-one\n\nworktree-two"));
    let lines = status_lines(&app, 35);
    assert!(
        lines
            .iter()
            .all(|(label, value)| display_width(label) + display_width(value) <= 35)
    );
    let values = lines
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>();
    assert!(values.concat().contains(&path));
    let first = values.iter().position(|value| *value == "worktree-one");
    let second = values.iter().position(|value| *value == "worktree-two");
    assert!(first.is_some());
    assert_eq!(second, first.map(|index| index + 2));
    assert_eq!(wrap("first\n\nsecond", 100), ["first", "", "second"]);
    Ok(())
}

#[test]
fn status_renders_each_linked_worktree_on_its_own_line() -> anyhow::Result<()> {
    let dir = testing::workspace("status-worktrees", testing::README)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", testing::README)])?;
    for name in ["worktree-alpha", "worktree-beta"] {
        git::worktree_add(&root, &dir.0.join(name), name)?;
    }
    let mut app = testing::app(&dir)?;
    app.resize(200, 30);
    app.command("status");
    let rendered = screen(&app)?;
    let alpha = rendered
        .iter()
        .position(|line| line.contains("worktree-alpha"));
    let beta = rendered
        .iter()
        .position(|line| line.contains("worktree-beta"));
    assert!(alpha.is_some());
    assert!(beta.is_some());
    assert_ne!(
        alpha, beta,
        "worktree records must not be merged into one row"
    );
    Ok(())
}

#[test]
fn doctor_retains_shared_scroll_and_dismissal_behavior() -> anyhow::Result<()> {
    let dir = testing::workspace("doctor-report-scroll", testing::README)?;
    let mut app = testing::app(&dir)?;
    app.resize(100, 12);
    app.command("doctor");
    let top = screen(&app)?;
    testing::press_key(&mut app, KeyCode::Down);
    assert_eq!(scroll(&app)?.offset, 1);
    send_mouse(&mut app, MouseEventKind::ScrollDown);
    assert_eq!(scroll(&app)?.offset, 4);
    assert_ne!(screen(&app)?, top);
    testing::press(&mut app, "r");
    assert_eq!(scroll(&app)?.offset, 0);
    send_mouse(&mut app, MouseEventKind::Down(MouseButton::Left));
    assert!(matches!(app.popup(), Some(Popup::Doctor(_))));
    testing::press_key(&mut app, KeyCode::Esc);
    assert!(app.popup().is_none());
    Ok(())
}
