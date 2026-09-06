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
//! the thread cursor's keys, then `Z` for the file's threads. Every
//! hint drawn works now (ADR 0064); the rest are left out.

use crate::app::draw::header::{Header, HintOf, draft_hints};
use crate::app::input::bindings::{Action, Where};
use crate::app::threads::words::Words;
use crate::app::{App, Focus};

/// The text's key bar: the draft's keys, the thread cursor's keys and
/// `Z` for the file while the text has focus, else the focus tip.
pub(crate) fn text_bar(app: &App) -> Header {
    if app.focus() != Focus::View {
        return Header::bar(vec![HintOf::new("", "click or Space w l to focus", &[])]);
    }
    if let Some(compose) = app.draft() {
        return Header::bar(draft_hints(compose));
    }
    let place = Where::View;
    let mut hints = Vec::new();
    if let Some(id) = app.thread_cursor().thread()
        && app.threads_at_cursor().contains(id)
        && let Some(thread) = app.thread(id)
    {
        let mark = app.mark_of(id);
        let words = Words::of(mark.map(crate::app::threads::Mark::placement), thread);
        let expanded = app.is_expanded(id);
        hints = thread_hints(app, place, words, expanded);
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

/// The keys that act on the thread cursor's thread: reply, edit when
/// the cursor's message is the user's, resolve or reopen, and `z` to
/// fold an expanded thread or expand a stub.
fn thread_hints(app: &App, place: Where, words: Words, expanded: bool) -> Vec<HintOf> {
    let resolve = if words.is_resolved() {
        "reopen"
    } else {
        "resolve"
    };
    let mut hints = vec![HintOf::keyed(place, Action::Reply, "reply")];
    if app.thread_message_editable() {
        hints.push(HintOf::keyed(place, Action::EditMessage, "edit"));
    }
    hints.push(HintOf::keyed(place, Action::ToggleResolved, resolve));
    hints.push(HintOf::keyed(
        place,
        Action::Fold,
        if expanded { "fold" } else { "expand" },
    ));
    hints
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use fathomable_core::annotations::{Author, LineRange};
    use fathomable_core::session::{Request, Response};

    use crate::app::Focus;
    use crate::app::draw::header::expanded_header;
    use crate::app::testing::{self, click, press_key, screen};

    /// The bar's row on the 100×30 test screen: the bottom text row,
    /// past the sidebar.
    fn bar(app: &crate::app::App) -> anyhow::Result<String> {
        Ok(screen(app)?[app.text_bar_row()]
            .chars()
            .skip(app.sidebar_width())
            .collect())
    }

    /// The bar carries the thread cursor's keys and only those that work
    /// (ADR 0064): `e edit` on the user's own message, `z expand` on a
    /// stub and `z fold` on an expanded thread, `Z` for the file; the
    /// thread header is its words alone; another pane's focus leaves the
    /// focus tip.
    #[test]
    fn the_bar_reads_the_cursor_threads_keys() -> anyhow::Result<()> {
        let dir = testing::workspace("text-bar", testing::README)?;
        // No toasts: the agent's comment would raise one over the bar.
        let mut app = testing::AppBuilder::new(&dir)
            .source_view()
            .options(|o| crate::app::Options {
                jump: fathomable_core::config::JumpConfig {
                    toast: std::time::Duration::ZERO,
                    ..o.jump
                },
                ..o
            })
            .build()?;
        assert!(!app.text_bar_shown(), "no thread, no draft: no bar");
        assert_eq!(app.text_rows(), 30 - 1, "the bar takes no row");

        // The user's thread on L3, an agent's on L5.
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        assert!(
            bar(&app)?.contains("Enter submit"),
            "the draft's keys: {:?}",
            bar(&app)?
        );
        app.compose_insert("mine");
        app.compose_submit();
        let started = app.handle_request(Request::ThreadStart {
            path: std::path::PathBuf::from("README.md"),
            range: Some(LineRange::new(5, 5)),
            author: Author::agent("reviewer").subscribed("s-1", "coder"),
            body: "theirs".to_owned(),
        });
        let Response::Threads(started) = started else {
            anyhow::bail!("{started:?}");
        };
        let mine = app.file_threads()[0].clone();
        let theirs = started[0].id().clone();
        app.expand_thread(mine.clone());
        app.goto_message(theirs.clone(), 0);
        assert_eq!(app.thread_cursor().thread(), Some(&theirs));

        assert_eq!(
            bar(&app)?.trim(),
            "r reply · o resolve · z fold · Z fold all",
            "the agent's thread: no edit"
        );
        let rows = screen(&app)?;
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("waiting") && row.contains("z fold")),
            "the header is words alone: {rows:?}"
        );
        let mine_thread = app
            .thread(&mine)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("mine"))?;
        assert_eq!(
            expanded_header(&app, &mine_thread).action_at(80, 60),
            None,
            "a click on a header runs nothing"
        );

        // On the user's own message `e edit` joins the keys.
        app.goto_message(mine.clone(), 0);
        assert_eq!(
            bar(&app)?.trim(),
            "r reply · e edit · o resolve · z fold · Z fold all"
        );

        // A stub reads `z expand`; with none expanded `Z` unfolds.
        app.fold_thread(&mine);
        app.fold_thread(&theirs);
        app.view_mut().goto_source_line(3);
        assert_eq!(
            bar(&app)?.trim(),
            "r reply · e edit · o resolve · z expand · Z unfold all"
        );
        assert!(
            !screen(&app)?.iter().any(|row| row.contains("(z expand)")),
            "the stub carries no hint"
        );
        // A line no thread covers keeps `Z` alone.
        app.view_mut().goto_source_line(1);
        assert_eq!(bar(&app)?.trim(), "Z unfold all");

        // Another pane's focus: the tip. A click on the tip focuses the
        // text, and one on a hint runs it.
        app.toggle_tree_focus();
        assert_eq!(bar(&app)?.trim(), "click or Space w l to focus");
        let row = app.text_bar_row();
        let sidebar = app.sidebar_width();
        click(&mut app, sidebar + 3, row);
        assert_eq!(app.focus(), Focus::View);
        app.view_mut().goto_source_line(3);
        click(&mut app, sidebar + 1, row);
        assert!(app.draft().is_some(), "`r reply` on the bar starts a reply");
        press_key(&mut app, KeyCode::Esc);
        assert!(app.draft().is_none());

        // The bar replaces the bottom text row; the text has as many rows
        // as it had with no bar (ADR 0067).
        assert!(app.text_bar_shown());
        assert_eq!(app.text_rows(), 30 - 1, "the text did not move");
        Ok(())
    }
}
