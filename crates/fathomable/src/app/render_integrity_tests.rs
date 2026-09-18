use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::{Position, Rect};

use super::draw::Theme;
use super::testing::{press_key, source_app, workspace};
use super::{App, Popup, draw};

type TestTerminal = Terminal<TestBackend>;

fn content() -> String {
    (0..180)
        .map(|row| match row % 4 {
            0 => format!("short row {row:03}\n"),
            1 => format!("Unicode row {row:03}: λ e\u{301} café Ελληνικά\n"),
            2 => format!(
                "long row {row:03}: {}\n",
                "wrapping-content-with-punctuation ".repeat(4)
            ),
            _ => format!("\tindented row {row:03}: mixed ASCII and Ελληνικά\n"),
        })
        .collect()
}

fn theme() -> anyhow::Result<Theme> {
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    Ok(Theme::from_core(&core))
}

fn assert_matches_fresh(
    accumulated: &mut TestTerminal,
    app: &App,
    theme: &Theme,
    stage: &str,
) -> anyhow::Result<()> {
    accumulated.draw(|frame| draw::draw(frame, app, theme))?;

    let area = accumulated.backend().buffer().area;
    let mut fresh = Terminal::new(TestBackend::new(area.width, area.height))?;
    fresh.draw(|frame| draw::draw(frame, app, theme))?;

    assert_eq!(
        accumulated.backend().buffer(),
        fresh.backend().buffer(),
        "differential buffer diverged from a fresh full render at {stage}"
    );
    assert_eq!(
        accumulated.backend().cursor_visible(),
        fresh.backend().cursor_visible(),
        "cursor visibility diverged at {stage}"
    );
    if fresh.backend().cursor_visible() {
        assert_eq!(
            accumulated.backend().cursor_position(),
            fresh.backend().cursor_position(),
            "visible cursor position diverged at {stage}"
        );
    }
    Ok(())
}

fn resize(
    terminal: &mut TestTerminal,
    app: &mut App,
    width: u16,
    height: u16,
) -> anyhow::Result<()> {
    app.resize(usize::from(width), usize::from(height));
    terminal.backend_mut().resize(width, height);
    terminal.resize(Rect::new(0, 0, width, height))?;
    Ok(())
}

#[test]
fn differential_rendering_matches_fresh_render_through_navigation_and_popups() -> anyhow::Result<()>
{
    const WIDTH: u16 = 52;
    const HEIGHT: u16 = 14;

    let dir = workspace("differential-render-integrity", &content())?;
    let mut app = source_app(&dir)?;
    let theme = theme()?;
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT))?;
    app.resize(usize::from(WIDTH), usize::from(HEIGHT));
    assert_matches_fresh(&mut terminal, &app, &theme, "initial frame")?;

    let initial_row = app.view().cursor().row;
    for step in 0..90 {
        press_key(&mut app, KeyCode::Down);
        assert_matches_fresh(
            &mut terminal,
            &app,
            &theme,
            &format!("down navigation {step}"),
        )?;
    }
    let down_row = app.view().cursor().row;
    let down_scroll = app.view().scroll();
    assert!(
        down_row >= initial_row + 90,
        "every synthetic Down key should move to another rendered row"
    );
    assert!(
        down_scroll > 0,
        "sustained Down keys should cross the lower viewport boundary"
    );

    for step in 0..45 {
        press_key(&mut app, KeyCode::Up);
        assert_matches_fresh(
            &mut terminal,
            &app,
            &theme,
            &format!("up navigation {step}"),
        )?;
    }
    assert!(
        app.view().cursor().row <= down_row - 45,
        "every synthetic Up key should move to another rendered row"
    );
    assert!(
        app.view().scroll() < down_scroll,
        "sustained Up keys should move the viewport back toward the start"
    );
    assert!(
        terminal.backend().cursor_visible(),
        "file view should expose the real terminal cursor before a popup"
    );

    press_key(&mut app, KeyCode::Char(' '));
    assert_matches_fresh(&mut terminal, &app, &theme, "help prefix")?;
    press_key(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.popup(), Some(Popup::Help(_))));
    assert_matches_fresh(&mut terminal, &app, &theme, "help popup visible")?;
    assert!(
        !terminal.backend().cursor_visible(),
        "help popup should hide the file view's real cursor"
    );

    press_key(&mut app, KeyCode::Esc);
    assert!(app.popup().is_none());
    assert_matches_fresh(&mut terminal, &app, &theme, "help popup dismissed")?;
    assert!(
        terminal.backend().cursor_visible(),
        "dismissing help should restore the file view's real cursor"
    );

    resize(&mut terminal, &mut app, 43, 11)?;
    assert_matches_fresh(&mut terminal, &app, &theme, "narrow resize")?;
    let narrow_row = app.view().cursor().row;
    for step in 0..24 {
        press_key(&mut app, KeyCode::Down);
        assert_matches_fresh(
            &mut terminal,
            &app,
            &theme,
            &format!("post-resize down navigation {step}"),
        )?;
    }
    assert!(
        app.view().cursor().row >= narrow_row + 24,
        "post-resize navigation should continue moving through the file"
    );
    resize(&mut terminal, &mut app, 61, 16)?;
    assert_matches_fresh(&mut terminal, &app, &theme, "wide resize")?;

    let cursor = app.view().cursor();
    let expected = Position::new(
        u16::try_from(app.sidebar_width() + draw::gutter_width(app.view()) + cursor.col)?,
        u16::try_from(app.text_top() + cursor.row - app.view().scroll())?,
    );
    assert_eq!(
        terminal.backend().cursor_position(),
        expected,
        "the final stationary cursor should occupy the selected file cell"
    );
    assert!(terminal.backend().cursor_visible());
    Ok(())
}
