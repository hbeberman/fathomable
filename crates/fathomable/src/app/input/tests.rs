use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keys::handle_key;
use crate::app::testing;
use crate::app::view::{Effect, Mode};
use crate::app::{App, Focus, PickerKind, Popup};

fn annotate(app: &mut App) {
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("thread");
    app.compose_submit();
}

fn open_and_cancel(app: &mut App) {
    assert_eq!(handle_key(app, testing::key('q')), Effect::None);
    assert!(matches!(app.popup(), Some(Popup::ConfirmQuit)));
    assert_eq!(handle_key(app, testing::key('y')), Effect::None);
    assert!(matches!(app.popup(), Some(Popup::ConfirmQuit)));
    assert_eq!(
        handle_key(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Effect::None
    );
    assert!(app.popup().is_none());
}

fn command(app: &mut App, text: &str) -> Effect {
    assert_eq!(handle_key(app, testing::key(':')), Effect::None);
    assert_eq!(app.view().mode(), Mode::Command);
    for ch in text.chars() {
        assert_eq!(handle_key(app, testing::key(ch)), Effect::None);
        assert!(!matches!(app.popup(), Some(Popup::ConfirmQuit)));
    }
    assert_eq!(app.view().input(), text);
    handle_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

#[test]
fn bare_q_is_guarded_in_panes_and_owned_by_input_and_popups() -> anyhow::Result<()> {
    let dir = testing::workspace("guarded-quit", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    annotate(&mut app);

    open_and_cancel(&mut app);
    app.window_files();
    assert_eq!(app.focus(), Focus::Tree);
    open_and_cancel(&mut app);
    app.open_review();
    assert_eq!(app.focus(), Focus::Review);
    open_and_cancel(&mut app);
    app.open_file_view();
    app.focus_threads_pane();
    assert_eq!(app.focus(), Focus::ThreadsPane);
    open_and_cancel(&mut app);

    app.open_file_view();
    handle_key(&mut app, testing::key(':'));
    handle_key(&mut app, testing::key('q'));
    assert_eq!(app.view().mode(), Mode::Command);
    assert_eq!(app.view().input(), "q");
    assert!(!matches!(app.popup(), Some(Popup::ConfirmQuit)));
    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    handle_key(&mut app, testing::key('/'));
    handle_key(&mut app, testing::key('q'));
    assert!(matches!(app.view().mode(), Mode::Search { .. }));
    assert_eq!(app.view().input(), "q");
    assert!(!matches!(app.popup(), Some(Popup::ConfirmQuit)));
    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    app.start_new_comment();
    handle_key(&mut app, testing::key('q'));
    assert_eq!(app.compose_draft(), Some("q"));
    assert!(matches!(app.popup(), Some(Popup::Compose(_))));
    app.compose_cancel();

    app.open_picker(PickerKind::Files);
    handle_key(&mut app, testing::key('q'));
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.input() == "q"
    ));
    app.close_popup();

    app.open_file_menu(app.pane_top());
    assert!(matches!(app.popup(), Some(Popup::Menu(_))));
    handle_key(&mut app, testing::key('q'));
    assert!(!matches!(app.popup(), Some(Popup::ConfirmQuit)));

    app.open_help();
    handle_key(&mut app, testing::key('q'));
    assert!(matches!(app.popup(), Some(Popup::Help(_))));
    app.close_popup();

    app.request_clear_board();
    handle_key(&mut app, testing::key('q'));
    assert!(matches!(app.popup(), Some(Popup::ConfirmBoard { .. })));
    app.cancel_clear_board();

    handle_key(&mut app, testing::key('q'));
    assert_eq!(
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Effect::Quit
    );
    assert!(app.popup().is_none());
    Ok(())
}

#[test]
fn command_input_owns_q_from_reviews_and_threads() -> anyhow::Result<()> {
    let dir = testing::workspace("pane-command-quit", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    annotate(&mut app);

    for focus in [Focus::Review, Focus::ThreadsPane] {
        match focus {
            Focus::Review => app.open_review(),
            Focus::ThreadsPane => {
                app.open_file_view();
                app.focus_threads_pane();
            }
            _ => unreachable!(),
        }
        assert_eq!(app.focus(), focus);

        for text in ["q", "quit", "q!", "quit!"] {
            assert_eq!(
                command(&mut app, text),
                Effect::Quit,
                ":{text} from {focus:?}"
            );
            assert!(app.popup().is_none());
            assert_eq!(app.focus(), focus);
        }

        assert_eq!(
            command(&mut app, "quitx"),
            Effect::Command("quitx".to_owned())
        );
        assert!(app.popup().is_none());
        assert_eq!(app.focus(), focus);
    }
    Ok(())
}
