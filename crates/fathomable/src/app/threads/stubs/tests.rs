use crossterm::event::KeyCode;
use std::fs;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::testing::{self, click, press, press_key, screen, source_app};

use crate::app::threads::ComposeTarget;
use crate::app::{App, Focus, Popup};
use fathomable_core::annotations::{Author, MAX_MESSAGE_BYTES, Reply, Store};
use fathomable_core::highlight::Highlighter;

use super::ExpandedLayout;

fn annotate(app: &mut App, from: usize, to: usize, text: &str) {
    app.view_mut().goto_source_line(from);
    if to > from {
        app.view_mut().select_lines();
        app.view_mut().move_down(to - from);
    }
    app.start_new_comment();
    app.compose_insert(text);
    app.compose_submit();
}

fn barred_rows(app: &App, rows: &[usize]) -> anyhow::Result<Vec<usize>> {
    let shown = screen(app)?;
    let gutter = crate::app::draw::gutter_width(app.view());
    Ok(rows
        .iter()
        .copied()
        .filter(|row| shown[*row].chars().nth(gutter) == Some('▎'))
        .collect())
}

#[test]
fn repeated_expanded_row_lookups_reuse_markdown_layouts() -> anyhow::Result<()> {
    let dir = testing::workspace("stubs-layout-cache", testing::README)?;
    let mut app = source_app(&dir)?;
    app.highlighter = Arc::new(Highlighter::new("base16-ocean.dark")?);
    let markdown = format!(
        "```markdown\n{}\n```",
        "[reference](https://example.com/path) **strong** `inline-code` ".repeat(7)
    );
    annotate(&mut app, 3, 3, &markdown);
    let id = app.file_threads()[0].clone();
    app.expand_thread(id.clone());

    let rows = app.view().layout().lines().len();
    let started = Instant::now();
    for _ in 0..20 {
        app.place_stub_rows();
        for row in 0..rows {
            black_box(app.stub_on_row(row));
        }
    }
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cached placement and lookup took {:?}",
        started.elapsed()
    );
    let previous_width = app
        .expanded_layout(&id)
        .ok_or_else(|| anyhow::anyhow!("expanded layout"))?
        .width();
    app.toggle_tree_shown();
    let current_width = app.view().layout().width();
    assert_ne!(current_width, previous_width);
    assert_eq!(
        app.expanded_layout(&id).map(ExpandedLayout::width),
        Some(current_width),
        "showing the sidebar reflows expanded messages"
    );
    Ok(())
}

#[test]
fn a_failed_draft_write_refreshes_externally_imported_messages() -> anyhow::Result<()> {
    let dir = testing::workspace("stubs-failed-write-refresh", testing::README)?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, 3, "question");
    let id = app.file_threads()[0].clone();
    app.expand_thread(id.clone());
    app.thread_reply();
    app.compose_insert(&"x".repeat(MAX_MESSAGE_BYTES + 1));

    let path = app
        .store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("thread store"))?
        .path()
        .to_path_buf();
    let mut writer = Store::open(path)?;
    writer.reply(
        &id,
        Reply::new(Author::agent("reviewer"), 1, "external answer"),
    )?;

    app.compose_submit();
    assert!(matches!(app.popup(), Some(Popup::Compose(_))));
    assert_eq!(
        app.thread(&id).map(|thread| thread.replies().len()),
        Some(1)
    );
    let width = app.view().layout().width();
    let thread = app
        .thread(&id)
        .ok_or_else(|| anyhow::anyhow!("imported thread"))?;
    assert!(
        app.expanded_layout(&id)
            .is_some_and(|layout| layout.matches(thread, width))
    );
    let _ = screen(&app)?;
    Ok(())
}

/// Stub rows sit under the last row of their thread, carry no line
/// number, and stacked threads come one after another in line order;
/// the cursor stops on each (ADR 0076) and the hint marks only the
/// cursor's.
/// Two threads stacked under L5: `outer` on L3-5 and `inner` on L5
/// with one reply, `reply`, the inner one folded back to a stub
/// after the reply expanded it.
fn stacked_threads(
    app: &mut crate::app::App,
    reply: &str,
) -> (
    fathomable_core::annotations::ThreadId,
    fathomable_core::annotations::ThreadId,
) {
    annotate(app, 3, 5, "outer thread");
    annotate(app, 5, 5, "inner point");
    app.thread_reply();
    app.compose_insert(reply);
    app.compose_submit();
    let outer = app.file_threads()[0].clone();
    let inner = app.file_threads()[1].clone();
    assert!(app.is_expanded(&inner), "a reply from the text expands");
    app.fold_thread(&inner);
    (outer, inner)
}

#[test]
fn stubs_hang_under_their_lines_and_the_cursor_stops_on_them() -> anyhow::Result<()> {
    let dir = testing::workspace("stubs-rows", testing::README)?;
    let mut app = source_app(&dir)?;
    let (outer, inner) = stacked_threads(&mut app, "agent-free reply");
    app.view_mut().goto_source_line(1);

    // Row model: 8 source rows, plus 1 + 1 rows under L5 and 1 under L7.
    annotate(&mut app, 7, 7, "seven");
    let rows = app.view().layout().lines().len();
    assert_eq!(rows, 11);
    assert_eq!(app.view().source_line_of_row(4), Some(5));
    assert!(
        app.view().stub_slot_of_row(5).is_some(),
        "outer's stub under L5"
    );
    assert!(
        app.view().stub_slot_of_row(6).is_some(),
        "inner's stub, its newest message"
    );
    assert_eq!(app.view().source_line_of_row(7), Some(6));
    let (stub, index, last) = app
        .stub_on_row(5)
        .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
    assert_eq!(stub.messages(), [0]);
    assert_eq!((index, last), (0, true));
    let (stub, index, last) = app
        .stub_on_row(6)
        .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
    assert_eq!(stub.messages(), [1], "the reply, not the comment");
    assert_eq!((index, last), (0, true));

    // The screen: the stub rows show the author, age, and text, no
    // number; the text of the thread under the cursor and the hint.
    app.view_mut().goto_source_line(5);
    let shown = screen(&app)?;
    assert!(shown[5].contains("gamma"), "{:?}", shown[5]);
    assert!(
        shown[6].contains("User") && shown[6].contains("outer thread"),
        "{:?}",
        shown[6]
    );
    assert!(
        !shown[6].contains(" 5 ") && !shown[6].contains(" 6 "),
        "no line number: {:?}",
        shown[6]
    );
    assert!(shown[7].contains("agent-free reply"), "{:?}", shown[7]);
    assert!(!shown[7].contains("inner point"), "{:?}", shown[7]);
    assert_eq!(shown[8].trim(), "6", "L6 follows: {:?}", shown[8]);
    // The cursor is on L5, which starts the inner thread: its stub
    // row carries the cursor bar, the outer's does not.
    assert_eq!(barred_rows(&app, &[6, 7])?, [7], "the inner thread's row");
    assert!(
        shown[app.text_bar_row()].contains("folding z/Z"),
        "{:?}",
        shown[app.text_bar_row()]
    );
    assert!(!shown[6].contains("(z expand)"), "{:?}", shown[6]);
    // On L4 only the outer thread covers the cursor.
    app.view_mut().goto_source_line(4);
    assert_eq!(barred_rows(&app, &[6, 7])?, [6], "the outer thread's row");

    // `j` from L5 stops on the outer stub, then the inner, then L6
    // (ADR 0076), each stub's thread the cursor's; `k` comes back
    // the same way.
    app.view_mut().goto_source_line(5);
    press(&mut app, "j");
    assert_eq!(app.view().cursor().row, 5);
    assert_eq!(app.thread_cursor().thread(), Some(&outer));
    press(&mut app, "j");
    assert_eq!(app.view().cursor().row, 6);
    assert_eq!(app.thread_cursor().thread(), Some(&inner));
    press(&mut app, "j");
    assert_eq!(app.view().cursor_source_line(), Some(6));
    assert_eq!(app.view().cursor().row, 7);
    press(&mut app, "kkk");
    assert_eq!(app.view().cursor().row, 4);
    press(&mut app, "G");
    assert_eq!(
        app.view().source_line_of_row(app.view().cursor().row),
        Some(8)
    );
    press(&mut app, "gg");
    assert_eq!(app.view().cursor().row, 0);
    // `x` selects lines, never a stub: from L5 the second press
    // steps over both stubs to L6, and from a stub the selection
    // anchors on the line the stub hangs under.
    app.view_mut().goto_source_line(5);
    press(&mut app, "xx");
    assert_eq!(
        app.view().selected_lines().map(|r| (r.start(), r.end())),
        Some((5, 6))
    );
    press_key(&mut app, KeyCode::Esc);
    app.view_mut().goto_source_line(5);
    press(&mut app, "j");
    assert_eq!(app.view().cursor().row, 5, "on the outer stub");
    press(&mut app, "x");
    assert_eq!(
        app.view().selected_lines().map(|r| (r.start(), r.end())),
        Some((5, 5)),
        "the line it hangs under"
    );
    press_key(&mut app, KeyCode::Esc);
    Ok(())
}

/// `threads { stubs #false }` and `Space v t` draw no stubs; resolved
/// threads have none until `Space v r`.
#[test]
fn the_toggles_hide_stubs_and_resolved_ones() -> anyhow::Result<()> {
    let dir = testing::workspace("stubs-toggles", testing::README)?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, 3, "three");
    annotate(&mut app, 7, 7, "seven");
    assert_eq!(app.view().layout().lines().len(), 10);
    app.view_mut().goto_source_line(3);
    press(&mut app, "r");
    assert_eq!(app.view().layout().lines().len(), 9, "resolved: no stub");
    assert!(app.stubs().iter().all(|stub| stub.messages().len() == 1));
    press(&mut app, " vx");
    assert_eq!(
        app.view().layout().lines().len(),
        9,
        "the retired shortcut does nothing"
    );
    assert!(!app.stubs_resolved());
    press(&mut app, " vr");
    assert_eq!(app.view().layout().lines().len(), 10);
    assert!(app.stubs_resolved());
    press(&mut app, " vr");
    assert_eq!(app.view().layout().lines().len(), 9);
    press(&mut app, " vt");
    assert!(!app.stubs_shown());
    assert_eq!(app.view().layout().lines().len(), 8);
    assert!(app.note_on_row(2).is_some(), "the gutter mark stays");
    Ok(())
}

/// A detached thread's stub hangs under its own blank row.
#[test]
fn a_detached_thread_keeps_its_stub_under_its_row() -> anyhow::Result<()> {
    let dir = testing::workspace("stubs-detached", testing::README)?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 4, 4, "on beta");
    fs::write(
        dir.0.join("ws/README.md"),
        "# Readme\n\nalpha\ngamma\n\n- one\n- two\n",
    )?;
    app.on_changes(vec![dir.0.join("ws/README.md")]);
    assert!(app.marks()[0].is_detached());
    let detached = (0..app.view().layout().lines().len())
        .find(|&row| app.view().detached_anchor_of_row(row).is_some())
        .ok_or_else(|| anyhow::anyhow!("a detached row"))?;
    assert!(app.view().stub_slot_of_row(detached + 1).is_some());
    assert_eq!(app.view().source_line_of_row(detached + 2), Some(4));
    Ok(())
}

/// `z` expands the thread under the cursor in place without moving
/// the view; `j`/`k` walk its messages and `c` replies with the
/// cursor landing on the reply; `z` folds it again.
/// Opening a thread from its chevron and closing it again leaves the
/// view where it was: the row the stub hangs under keeps its place on
/// screen, even though the cursor sat on a message row that went.
#[test]
fn opening_and_closing_a_thread_leaves_the_view_still() -> anyhow::Result<()> {
    let lines: Vec<String> = (1..=80).map(|n| format!("line {n}")).collect();
    let text = lines.join("\n") + "\n";
    let dir = testing::workspace("stubs-fold-still", &text)?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 40, 40, "a point\nwith a second line");
    app.thread_reply();
    app.compose_insert("a reply");
    app.compose_submit();
    let id = app.file_threads()[0].clone();
    app.fold_thread(&id);
    app.view_mut().goto_source_line(40);
    // The wheel brings the cursor to mid-screen, where the opened
    // thread fits below it without a scroll.
    app.view_mut().scroll_by(10);
    let scroll = app.view().scroll();
    assert!(scroll > 0, "the view has scrolled down to L40");
    assert_eq!(app.view().cursor_source_line(), Some(40));
    for _ in 0..3 {
        let newest = app.newest_message(&id);
        app.goto_message(id.clone(), newest);
        assert!(app.expanded_row_message(app.view().cursor().row).is_some());
        app.fold_thread(&id);
        assert_eq!(app.view().scroll(), scroll, "the view stays still");
        assert_eq!(app.view().cursor_source_line(), Some(40));
    }
    // `z` then `j` onto a message, and `z` to fold, the same.
    press(&mut app, "z");
    press(&mut app, "j");
    press(&mut app, "z");
    assert!(!app.is_expanded(&id));
    assert_eq!(app.view().scroll(), scroll, "the view stays still");
    Ok(())
}

#[test]
fn z_expands_in_place_and_walks_messages() -> anyhow::Result<()> {
    let dir = testing::workspace("stubs-expand", testing::README)?;
    let mut app = source_app(&dir)?;
    let (outer, inner) = stacked_threads(&mut app, "first reply\nwith a second line");

    // On L5 the thread cursor is the inner thread; `z` expands it.
    app.view_mut().goto_source_line(5);
    let scroll = app.view().scroll();
    press(&mut app, "z");
    assert!(app.is_expanded(&inner));
    assert!(!app.is_expanded(&outer));
    assert_eq!(app.view().scroll(), scroll, "the view stays still");
    assert_eq!(app.view().cursor_source_line(), Some(5), "the cursor too");
    assert_eq!(app.thread_cursor().message(), 1, "the newest message");
    let shown = screen(&app)?;
    // Outer's collapsed stub, then inner's header, comment, reply.
    assert!(shown[6].contains("outer thread"), "{:?}", shown[6]);
    assert!(
        shown[7].contains("● ▾") && !shown[7].contains("Resolve"),
        "the header carries facts but no lifecycle controls: {:?}",
        shown[7]
    );
    assert!(
        shown[app.text_bar_row()].contains("resolve r")
            && shown[app.text_bar_row()].contains("folding z/Z"),
        "the bar names the keys: {:?}",
        shown[app.text_bar_row()]
    );
    assert!(
        shown[8].contains("User") && shown[9].contains("inner point"),
        "{:?}",
        &shown[8..10]
    );
    assert!(
        shown[11].contains("first reply") && shown[12].contains("second line"),
        "{:?}",
        &shown[11..13]
    );
    assert_eq!(shown[13].trim(), "6", "L6 follows: {:?}", shown[13]);

    // `j` from L5 stops on the outer stub (ADR 0076), the comment,
    // then the reply, then L6.
    press(&mut app, "j");
    assert_eq!(app.view().cursor().row, 5);
    assert_eq!(app.thread_cursor().thread(), Some(&outer));
    press(&mut app, "j");
    assert_eq!(app.view().cursor().row, 7);
    assert_eq!(app.thread_cursor().message(), 0);
    press(&mut app, "j");
    assert_eq!(app.view().cursor().row, 9);
    assert_eq!(app.thread_cursor().message(), 1);
    press(&mut app, "j");
    assert_eq!(app.view().cursor_source_line(), Some(6));
    press(&mut app, "kk");
    assert_eq!(app.view().cursor().row, 7);
    assert_eq!(app.expanded_row_message(7), Some((inner.clone(), 0)));

    // `c` on a message row replies to its thread; the cursor lands on
    // the reply and the keys stay with the text.
    press(&mut app, "c");
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(c)) if *c.target() == ComposeTarget::Reply(inner.clone())
    ));
    app.compose_insert("second reply");
    app.compose_submit();
    assert_eq!(app.focus(), Focus::View);
    assert_eq!(app.thread_cursor().message(), 2);
    assert_eq!(
        app.expanded_row_message(app.view().cursor().row),
        Some((inner.clone(), 2))
    );
    // `e` edits the message under the cursor.
    press(&mut app, "e");
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(c)) if matches!(c.target(), ComposeTarget::Edit { .. })
    ));
    app.compose_cancel();

    // `z` on an expanded row folds it without opening another thread.
    press(&mut app, "z");
    assert!(!app.is_expanded(&inner));
    assert!(!app.is_expanded(&outer));
    assert_eq!(app.view().cursor_source_line(), Some(5));

    // Expanding every stub, then folding them all.
    app.toggle_expand_all();
    assert!(app.is_expanded(&inner) && app.is_expanded(&outer));
    app.toggle_expand_all();
    assert!(!app.is_expanded(&inner) && !app.is_expanded(&outer));

    // A click on a stub's chevron column expands it with the cursor
    // on it (ADR 0073); the stub shows inner's newest two messages.
    let row = screen(&app)?
        .iter()
        .position(|line| line.contains("second reply") && !line.contains("inner point"))
        .ok_or_else(|| anyhow::anyhow!("inner's stub"))?;
    let chevron = app.sidebar_width() + crate::app::draw::gutter_width(app.view()) + 3;
    click(&mut app, chevron, row);
    assert!(app.is_expanded(&inner));
    assert_eq!(app.thread_cursor().thread(), Some(&inner));
    assert_eq!(
        app.expanded_row_message(app.view().cursor().row)
            .map(|(id, _)| id),
        Some(inner.clone())
    );
    // `dd` on its rows deletes the thread (ADR 0034).
    press(&mut app, "dd");
    assert_eq!(app.file_threads().len(), 1);
    assert_eq!(app.message(), Some("deleted"));
    Ok(())
}
