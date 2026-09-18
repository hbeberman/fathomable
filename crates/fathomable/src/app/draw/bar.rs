// @okf-doc: /decisions/0067-the-texts-key-bar.md
//! The text column's key bar (ADR 0067).
//!
//! A key bar on `ui.header` replaces the bottom text row while it has
//! something to say, as the threads pane's bar replaces its bottom row
//! (ADR 0066); the text never moves for it, and with nothing to say the
//! row is text. It has something to say while a draft is open, a
//! thread is under the cursor, or the file has a thread to fold: then,
//! while another pane has the keys, it says how to focus the text, and
//! a click on it does; with the keys it reads the draft's keys, else
//! the diff's keys while a diff is open (ADR 0069), the thread cursor's
//! reply/edit/lifecycle/fold keys, then `Z` for the file's threads.

use crate::app::draw::header::{Header, HintOf, draft_hints};
use crate::app::input::bindings::{Action, Where};
use crate::app::threads::words::Words;
use crate::app::{App, Focus};

/// The text's key bar: the draft's keys, else the diff's keys, the
/// thread cursor's keys, and `Z` for the file while the text has focus;
/// the focus tip otherwise.
pub(crate) fn text_bar(app: &App) -> Header {
    if app.focus() != Focus::View {
        return Header::bar(vec![HintOf::new("", "click or Space w l to focus", &[])]);
    }
    if let Some(compose) = app.draft() {
        return Header::bar(draft_hints(compose));
    }
    let place = Where::View;
    let mut hints = crate::app::diff_keys::diff_hints(app);
    if let Some(id) = app.thread_cursor().thread()
        && app.threads_at_cursor().contains(id)
        && let Some(thread) = app.thread(id)
    {
        let mark = app.mark_of(id);
        let words = Words::of(mark.map(crate::app::threads::Mark::placement), thread);
        let expanded = app.is_expanded(id);
        let on_thread_row = app.cursor_on_thread_row(id);
        hints.extend(thread_hints(
            app,
            place,
            words,
            thread.is_archived(),
            expanded,
            on_thread_row,
        ));
    }
    let stubs = app.stubs();
    if !stubs.is_empty() {
        let any_expanded = stubs
            .iter()
            .filter_map(|stub| stub.thread())
            .any(|id| app.is_expanded(id));
        let what = if any_expanded {
            "fold all"
        } else {
            "unfold all"
        };
        hints.push(HintOf::keyed(place, Action::FoldAll, what));
    }
    Header::bar(hints)
}

/// The keys that act on the thread cursor's thread: reply from its rows,
/// edit when the cursor's message is the user's, resolve or reopen, and
/// `z` to fold an expanded thread or expand a stub.
fn thread_hints(
    app: &App,
    place: Where,
    words: Words,
    archived: bool,
    expanded: bool,
    on_thread_row: bool,
) -> Vec<HintOf> {
    let resolve = if words.is_resolved() {
        "reopen"
    } else {
        "resolve"
    };
    let mut hints = Vec::new();
    if on_thread_row {
        hints.push(HintOf::keyed(place, Action::Comment, "reply"));
    }
    if app.thread_message_editable() {
        hints.push(HintOf::keyed(place, Action::EditMessage, "edit"));
    }
    if !words.is_resolved() {
        hints.push(HintOf::keyed(
            place,
            Action::ToggleAutoResolve,
            "auto-resolve",
        ));
    }
    hints.push(HintOf::keyed(place, Action::ToggleResolved, resolve));
    if words.is_resolved() && !archived {
        hints.push(HintOf::keyed(place, Action::ArchiveThread, "archive"));
    }
    hints.push(HintOf::keyed(
        place,
        Action::Fold,
        if expanded { "fold" } else { "expand" },
    ));
    hints
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use crossterm::event::KeyCode;
    use fathomable_core::annotations::{Author, Draft, LineRange, Store};

    use super::text_bar;
    use crate::app::Focus;
    use crate::app::input::bindings::Action;
    use crate::app::testing::{self, click, press_key, screen};

    /// The bar's row on the 100×30 test screen: the bottom text row,
    /// past the sidebar.
    fn bar(app: &crate::app::App) -> anyhow::Result<String> {
        Ok(screen(app)?[app.text_bar_row()]
            .chars()
            .skip(app.sidebar_width())
            .collect())
    }

    fn click_bar_action(app: &mut crate::app::App, action: Action) -> anyhow::Result<()> {
        let width = app.column_width();
        let column = (0..width)
            .find(|&column| text_bar(app).action_at(width, column) == Some(action))
            .ok_or_else(|| anyhow::anyhow!("bar action {action:?}"))?;
        click(app, app.sidebar_width() + column, app.text_bar_row());
        Ok(())
    }

    fn archive_resolved_cursor(
        app: &mut crate::app::App,
        id: &fathomable_core::annotations::ThreadId,
    ) -> anyhow::Result<()> {
        testing::press(app, "r");
        assert_eq!(
            bar(app)?.trim(),
            "reply c · edit e · reopen r · archive a · fold z · fold all Z"
        );
        let rows = screen(app)?;
        assert!(
            rows.iter()
                .all(|row| !row.contains("Archive") && !row.contains("Restore")),
            "inline headers are factual: {rows:?}"
        );
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let narrow: String = text_bar(app)
            .line(&theme, 50)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(narrow.contains("archive a"), "{narrow:?}");
        assert!(!narrow.contains("fold all"), "{narrow:?}");
        click_bar_action(app, Action::ArchiveThread)?;
        assert!(
            app.thread(id)
                .is_some_and(fathomable_core::annotations::Thread::is_archived),
            "the text footer archives its cursor thread"
        );
        app.restore_thread(id);
        app.goto_message(id.clone(), 0);
        testing::press(app, "a");
        assert!(
            app.thread(id)
                .is_some_and(fathomable_core::annotations::Thread::is_archived),
            "`a` archives the text cursor thread"
        );
        Ok(())
    }

    #[test]
    fn eof_marker_remains_above_the_bar_with_folded_and_expanded_threads() -> anyhow::Result<()> {
        let text = (1..=60)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let dir = testing::workspace("eof-bar", &text)?;
        let mut app = testing::AppBuilder::new(&dir)
            .source_view()
            .options(|mut options| {
                options.watch.toast = std::time::Duration::ZERO;
                options
            })
            .build()?;
        app.view_mut().goto_source_line(4);
        app.start_new_comment();
        app.compose_insert("a thread");
        app.compose_submit();

        for keys in ["", "Z", "Z", " vs", " vs", " vt"] {
            testing::press(&mut app, keys);
            // Exercise keyboard arrival and wheel clamping separately.
            for wheel in [false, true] {
                testing::press(&mut app, "ge");
                if wheel {
                    app.view_mut().scroll_by(isize::MAX);
                }
                let rows = screen(&app)?;
                let end = app.text_bar_row() - usize::from(app.text_bar_shown());
                let content: String = rows[end].chars().skip(app.sidebar_width()).collect();
                assert_eq!(content.trim(), "~", "{keys:?}: {rows:?}");
                let previous: String = rows[end - 1].chars().skip(app.sidebar_width()).collect();
                assert!(previous.contains("line 60"), "{previous:?}");
                assert_eq!(app.view().cursor_source_line(), Some(119));
                let scroll = app.view().scroll();
                app.view_mut().scroll_by(1);
                assert_eq!(app.view().scroll(), scroll, "only one EOF row");
            }
        }
        Ok(())
    }

    #[test]
    fn eof_marker_remains_above_diff_keys_and_after_resizing() -> anyhow::Result<()> {
        let mut text = String::new();
        for n in 1..=60 {
            writeln!(text, "line {n}")?;
        }
        let dir = testing::workspace("eof-diff-bar", &text)?;
        let mut app = testing::source_app(&dir)?;
        app.view_mut().set_bases(None, Some("old\n".to_owned()));
        app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
        for width in [80, 100] {
            app.resize(width, 30);
            testing::press(&mut app, "ge");
            let rows = screen(&app)?;
            let end = app.text_bar_row() - 1;
            let content: String = rows[end].chars().skip(app.sidebar_width()).collect();
            assert_eq!(content.trim(), "~", "{rows:?}");
            assert!(bar(&app)?.contains("base"));
            assert_eq!(app.view().cursor_source_line(), Some(60));
        }
        Ok(())
    }

    /// The bar carries the thread cursor's keys and only those that work
    /// (ADR 0064): `edit e` on the user's own message, `expand z` on a
    /// stub and `fold z` on an expanded thread, `Z` for the file; the
    /// thread header is its words alone; another pane's focus leaves the
    /// focus tip.
    #[test]
    fn the_bar_reads_the_cursor_threads_keys() -> anyhow::Result<()> {
        let dir = testing::workspace("text-bar", testing::README)?;
        // No toasts: the agent's comment would raise one over the bar.
        let mut app = testing::AppBuilder::new(&dir)
            .source_view()
            .options(|o| crate::app::Options {
                watch: fathomable_core::config::WatchConfig {
                    toast: std::time::Duration::ZERO,
                    ..o.watch
                },
                ..o
            })
            .build()?;
        assert!(!app.text_bar_shown(), "no thread, no draft: no bar");
        assert_eq!(app.text_rows(), 30 - 2, "only the file header takes a row");

        // The user's thread on L3, an agent's on L5.
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        assert!(
            bar(&app)?.contains("submit Enter"),
            "the draft's keys: {:?}",
            bar(&app)?
        );
        app.compose_insert("mine");
        app.compose_submit();
        let theirs = Store::open(testing::store_path(&dir))?.annotate(
            Draft::new(
                Author::agent("reviewer"),
                std::path::Path::new("README.md"),
                LineRange::new(5, 5),
                "theirs",
            ),
            testing::README,
            2,
        )?;
        app.reload_store();
        let mine = app.file_threads()[0].clone();
        app.expand_thread(mine.clone());
        app.goto_message(theirs.clone(), 0);
        assert_eq!(app.thread_cursor().thread(), Some(&theirs));

        assert_eq!(
            bar(&app)?.trim(),
            "reply c · auto-resolve R · resolve r · fold z · fold all Z",
            "the agent's thread: no edit"
        );
        let rows = screen(&app)?;
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("waiting") && row.contains("fold z")),
            "the header is words alone: {rows:?}"
        );
        // On the user's own message `edit e` joins the keys.
        app.goto_message(mine.clone(), 0);
        assert_eq!(
            bar(&app)?.trim(),
            "reply c · edit e · auto-resolve R · resolve r · fold z · fold all Z"
        );

        // A stub reads `expand z`; with none expanded `Z` unfolds.
        app.fold_thread(&mine);
        app.fold_thread(&theirs);
        app.view_mut().goto_source_line(3);
        assert_eq!(
            bar(&app)?.trim(),
            "edit e · auto-resolve R · resolve r · expand z · unfold all Z"
        );
        assert!(
            !screen(&app)?.iter().any(|row| row.contains("(z expand)")),
            "the stub carries no hint"
        );
        // A line no thread covers keeps `Z` alone.
        app.view_mut().goto_source_line(1);
        assert_eq!(bar(&app)?.trim(), "unfold all Z");

        // Another pane's focus: the tip. A click on the tip focuses the
        // text, and one on a hint runs it.
        app.toggle_tree_focus();
        assert_eq!(bar(&app)?.trim(), "click or Space w l to focus");
        let row = app.text_bar_row();
        let sidebar = app.sidebar_width();
        click(&mut app, sidebar + 3, row);
        assert_eq!(app.focus(), Focus::View);
        app.view_mut().goto_source_line(3);
        assert!(
            !bar(&app)?.contains("reply"),
            "the source line has no reply hint"
        );
        let stub_row = (0..app.view().layout().lines().len())
            .find(|&candidate| {
                app.stub_on_row(candidate)
                    .is_some_and(|(stub, _, _)| stub.thread() == Some(&mine))
            })
            .ok_or_else(|| anyhow::anyhow!("stub row"))?;
        app.view_mut().goto_row(stub_row);
        assert_eq!(app.thread_cursor().thread(), Some(&mine));
        click_bar_action(&mut app, Action::Comment)?;
        assert!(app.draft().is_some(), "`reply c` on the bar starts a reply");
        press_key(&mut app, KeyCode::Esc);
        assert!(app.draft().is_none());

        archive_resolved_cursor(&mut app, &mine)?;

        // The bar replaces the bottom text row; the text has as many rows
        // as it had with no bar (ADR 0067).
        assert!(app.text_bar_shown());
        assert_eq!(app.text_rows(), 30 - 2, "the text did not move");
        Ok(())
    }
}
