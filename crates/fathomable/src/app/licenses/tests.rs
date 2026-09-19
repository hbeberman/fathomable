use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use super::{Licenses, content_size};
use crate::app::input::{keys, mouse};
use crate::app::menu_bar::{self, Root, Submenu, Target};
use crate::app::{App, Popup, testing};

fn pane(app: &App) -> anyhow::Result<&Licenses> {
    match app.popup() {
        Some(Popup::Licenses(licenses)) => Ok(licenses),
        _ => anyhow::bail!("the Licenses pane is not open"),
    }
}

#[test]
fn licenses_display_preserves_the_document() -> anyhow::Result<()> {
    let dir = testing::workspace("licenses-command", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    testing::press(&mut app, "jj");
    let path = app.current_path().to_owned();
    let position = app.view().source_position();
    let focus = app.focus();
    app.open_licenses();
    let screen = testing::screen(&app)?.join("\n");
    assert!(screen.contains("Licenses"));
    assert!(screen.contains("Fathomable"));
    assert_eq!(pane(&app)?.scroll, 0);

    testing::press(&mut app, "ddcq");
    assert!(matches!(app.popup(), Some(Popup::Licenses(_))));
    assert_eq!(app.current_path(), path);
    assert_eq!(app.view().text(), testing::README);
    assert_eq!(app.view().source_position(), position);
    testing::press_key(&mut app, KeyCode::Esc);
    assert!(app.popup().is_none());
    assert_eq!(app.focus(), focus);
    assert_eq!(app.view().source_position(), position);
    Ok(())
}

#[test]
fn help_menu_exposes_licenses_and_runs_its_displayed_command() -> anyhow::Result<()> {
    let dir = testing::workspace("licenses-menu", testing::README)?;
    let mut app = testing::app(&dir)?;
    for width in [100, 25] {
        app.resize(width, 30);
        let rows = menu_bar::submenu_rows(&app, Submenu::Help);
        let item = rows
            .iter()
            .filter_map(menu_bar::Row::item)
            .find(|item| item.target == Target::Licenses)
            .context("Licenses is missing from Help")?;
        assert_eq!(item.label, "Licenses");
        assert!(item.hint.is_empty());
        assert!(item.enabled);
        app.open_title_menu(Root::App);
        let before_help = menu_bar::rows(&app, Root::App)
            .iter()
            .filter_map(menu_bar::Row::item)
            .take_while(|item| item.target != Target::Submenu(Submenu::Help))
            .count();
        for _ in 0..=before_help {
            testing::press_key(&mut app, KeyCode::Down);
        }
        testing::press_key(&mut app, KeyCode::Right);
        testing::press(&mut app, "jjj");
        testing::press_key(&mut app, KeyCode::Enter);
        assert!(matches!(app.popup(), Some(Popup::Licenses(_))));
        assert!(!app.title_menu_open());
        app.close_popup();
    }
    Ok(())
}

#[test]
fn licenses_scrolls_by_keyboard_and_mouse_and_clamps_at_both_ends() -> anyhow::Result<()> {
    let dir = testing::workspace("licenses-scroll", testing::README)?;
    let mut app = testing::app(&dir)?;
    app.open_licenses();
    let (_, height) = content_size(&app);
    testing::press_key(&mut app, KeyCode::PageDown);
    assert_eq!(pane(&app)?.scroll, height);
    mouse::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 4,
            row: 5,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(pane(&app)?.scroll, height + 3);
    keys::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    assert_eq!(pane(&app)?.scroll, height + 3 - (height / 2).max(1));
    testing::press_key(&mut app, KeyCode::End);
    let end = pane(&app)?.scroll;
    assert_eq!(end, pane(&app)?.layout.lines().len() - height);
    assert_eq!(pane(&app)?.visible_lines(height).count(), height);
    testing::press(&mut app, "jjj");
    assert_eq!(pane(&app)?.scroll, end);
    testing::press_key(&mut app, KeyCode::Home);
    testing::press(&mut app, "kkk");
    assert_eq!(pane(&app)?.scroll, 0);
    Ok(())
}

#[test]
fn licenses_resize_keeps_the_reading_location_and_remains_drawable() -> anyhow::Result<()> {
    let dir = testing::workspace("licenses-resize", testing::README)?;
    let mut app = testing::app(&dir)?;
    app.open_licenses();
    testing::press_key(&mut app, KeyCode::PageDown);
    let before = pane(&app)?
        .visible_lines(1)
        .next()
        .and_then(fathomable_core::layout::Line::source)
        .context("top notice source range")?
        .start;
    app.resize(35, 18);
    let top = pane(&app)?
        .visible_lines(1)
        .next()
        .and_then(fathomable_core::layout::Line::source)
        .context("resized notice source range")?;
    assert!(top.contains(&before) || top.start == before);
    for (width, height) in [(35, 18), (8, 5), (1, 1), (100, 30)] {
        app.resize(width, height);
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
            u16::try_from(width)?,
            u16::try_from(height)?,
        ))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
        let licenses = pane(&app)?;
        assert!(
            licenses.scroll
                <= licenses
                    .layout
                    .lines()
                    .len()
                    .saturating_sub(content_size(&app).1)
        );
    }
    Ok(())
}

#[test]
fn licenses_closes_outside_and_the_menu_bar_remains_reachable() -> anyhow::Result<()> {
    let dir = testing::workspace("licenses-dismiss", testing::README)?;
    let mut app = testing::AppBuilder::new(&dir)
        .options(|mut options| {
            options.menu_bar = true;
            options
        })
        .build()?;
    app.open_licenses();
    testing::click(&mut app, 5, 5);
    assert!(matches!(app.popup(), Some(Popup::Licenses(_))));
    testing::click(&mut app, 0, 10);
    assert!(app.popup().is_none());
    app.open_licenses();
    testing::click(&mut app, 1, 0);
    assert!(app.title_menu_open());
    assert!(app.popup().is_none());
    Ok(())
}
