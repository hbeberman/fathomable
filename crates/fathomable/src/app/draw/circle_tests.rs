use std::path::Path;

use anyhow::Context as _;
use fathomable_core::annotations::{Author, AutoResolve, Draft, LineRange, Store, ThreadId};
use fathomable_core::clock::now;
use fathomable_testing::TempDir;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

use crate::app::App;
use crate::app::testing;
use crate::app::threads::list::Row;

use super::selection::Navigation;
use super::{ListRender, Theme, header, list_row, note_cell};

#[derive(Debug, Clone, Copy)]
enum Speaker {
    Human,
    Agent,
}

fn fixture(name: &str, author: Author) -> anyhow::Result<(TempDir, App, ThreadId)> {
    let dir = testing::workspace(name, testing::README)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let draft = testing::at_working_tree(
        &testing::root(&dir),
        Draft::new(
            author,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "opening",
        ),
        testing::README,
    )?;
    let id = store.annotate(draft, testing::README, now())?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.show_threads_pane();
    app.review_toggle_resolved();
    app.toggle_resolved_stubs();
    Ok((dir, app, id))
}

fn render(app: &App, theme: &Theme) -> anyhow::Result<Buffer> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
    terminal.draw(|frame| super::draw(frame, app, theme))?;
    Ok(terminal.backend().buffer().clone())
}

fn assert_cards(buffer: &Buffer, glyph: &str, colour: Color) {
    let mut cards = 0;
    for row in 0..buffer.area.height {
        let text: String = (0..buffer.area.width)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        if !text.contains("L3") {
            continue;
        }
        for column in 0..buffer.area.width {
            let cell = &buffer[(column, row)];
            if ["●", "◐", "○"].contains(&cell.symbol()) {
                assert_eq!(cell.symbol(), glyph, "{text}");
                assert_eq!(cell.fg, colour, "{text}");
                cards += 1;
            }
        }
    }
    assert!(
        cards >= 2,
        "main and sidebar thread headers must both render"
    );
}

fn assert_surfaces(
    app: &mut App,
    id: &ThreadId,
    glyph: &str,
    speaker: Speaker,
) -> anyhow::Result<()> {
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = Theme::from_core(&core);
    let colour = match speaker {
        Speaker::Human => theme.thread_user,
        Speaker::Agent => theme.thread_agent,
    }
    .fg
    .context("author colour")?;
    let row = (0..app.view().layout().lines().len())
        .find(|&row| app.view().source_line_of_row(row) == Some(3))
        .context("source row")?;
    let note = note_cell(app, &theme, row, theme.text);
    assert_eq!(note.content, glyph);
    assert_eq!(note.style.fg, Some(colour));

    let thread = app.thread(id).context("thread")?;
    for expanded in [false, true] {
        let layout = header::expanded_header(app, thread, expanded, 70);
        let line = header::summary_line(&theme, &layout, Vec::new());
        assert_eq!(line.spans[0].content, glyph);
        assert_eq!(line.spans[0].style.fg, Some(colour));
        assert_eq!(line.spans[0].style.bg, theme.header.bg);
    }
    assert_cards(&render(app, &theme)?, glyph, colour);

    app.open_review();
    assert_cards(&render(app, &theme)?, glyph, colour);
    let rows = app.review_rows(70);
    let mut anchors = 0;
    for row in &rows.rows {
        if let Row::OriginContext { glyph: Some(_), .. } = row {
            let line = list_row(
                &ListRender {
                    theme: &theme,
                    now: now(),
                    width: 70,
                    navigation: Navigation::Active,
                },
                row,
            );
            assert_eq!(line.spans[1].content, glyph);
            assert_eq!(line.spans[1].style.fg, Some(colour));
            assert_eq!(line.spans[1].style.bg, theme.thread_bracket.bg);
            anchors += 1;
        }
    }
    assert!(anchors > 0, "stored origin context must have an anchor");
    let file_mark = app
        .file_circles()
        .into_iter()
        .next()
        .context("file mark")?
        .1;
    assert_eq!(file_mark.glyph(), glyph);
    assert_eq!(super::circle_style(&theme, file_mark).fg, Some(colour));
    app.close_review();
    app.view_mut().goto_source_line(3);
    Ok(())
}

#[test]
fn circles_follow_agent_openings_replies_permission_and_resolution() -> anyhow::Result<()> {
    let (_dir, mut app, id) = fixture("agent-circles", Author::agent("bot"))?;
    assert_surfaces(&mut app, &id, "●", Speaker::Agent)?;

    testing::external_agent_reply(&mut app, &id, Author::agent("bot"), "ordinary", false, None)?;
    assert_surfaces(&mut app, &id, "●", Speaker::Agent)?;
    testing::external_agent_reply(&mut app, &id, Author::agent("bot"), "finished", true, None)?;
    assert_surfaces(&mut app, &id, "◐", Speaker::Agent)?;

    app.thread_reply();
    app.compose_insert("please finish");
    app.compose_submit_auto_resolve();
    assert_surfaces(&mut app, &id, "◐", Speaker::Human)?;
    testing::external_agent_reply(&mut app, &id, Author::agent("bot"), "not yet", false, None)?;
    assert_surfaces(&mut app, &id, "●", Speaker::Agent)?;

    app.toggle_auto_resolve(&id);
    assert_surfaces(&mut app, &id, "◐", Speaker::Agent)?;
    app.toggle_auto_resolve(&id);
    assert_surfaces(&mut app, &id, "●", Speaker::Agent)?;
    app.toggle_auto_resolve(&id);
    testing::external_agent_reply(&mut app, &id, Author::agent("bot"), "done", true, None)?;
    assert_surfaces(&mut app, &id, "○", Speaker::Agent)?;

    app.toggle_resolved(&id);
    assert_surfaces(&mut app, &id, "●", Speaker::Agent)?;
    app.thread_reply();
    app.compose_insert("another question");
    app.compose_submit();
    assert_surfaces(&mut app, &id, "●", Speaker::Human)?;
    app.toggle_resolved(&id);
    assert_surfaces(&mut app, &id, "○", Speaker::Human)?;
    Ok(())
}

#[test]
fn human_permission_changes_shape_without_proposing_resolution() -> anyhow::Result<()> {
    let (dir, mut app, id) = fixture("human-circles", Author::User)?;
    assert_surfaces(&mut app, &id, "●", Speaker::Human)?;
    app.toggle_auto_resolve(&id);
    assert_surfaces(&mut app, &id, "◐", Speaker::Human)?;
    assert_eq!(app.review_counts(false).active, 1);
    assert_eq!(app.review_counts(false).proposed, 0);
    app.toggle_auto_resolve(&id);
    assert_surfaces(&mut app, &id, "●", Speaker::Human)?;

    let mut writer = Store::open(testing::store_path(&dir))?;
    let draft = testing::at_working_tree(
        &testing::root(&dir),
        Draft::new(
            Author::agent("bot"),
            Path::new("README.md"),
            LineRange::new(3, 3),
            "other",
        ),
        testing::README,
    )?;
    writer.annotate(draft, testing::README, now())?;
    writer.set_auto_resolve(&id, AutoResolve::Enabled, now())?;
    app.reload_store();
    let row = (0..app.view().layout().lines().len())
        .find(|&row| app.view().source_line_of_row(row) == Some(3))
        .context("source row")?;
    let words = app.note_on_row(row).context("overlapping marks")?.1;
    assert_eq!(words.glyph(), "◐");
    assert!(
        words.author_is_user(),
        "permission takes priority over ordinary active marks"
    );
    let file_mark = app
        .file_circles()
        .into_iter()
        .next()
        .context("file mark")?
        .1;
    assert_eq!(file_mark, words);
    Ok(())
}
