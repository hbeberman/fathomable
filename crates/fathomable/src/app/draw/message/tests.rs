use std::path::Path;

use fathomable_core::annotations::{Draft, Store};
use fathomable_testing::TempDir;
use ratatui::style::Modifier;

use super::*;

fn theme() -> anyhow::Result<Theme> {
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    Ok(Theme::from_core(&core))
}

fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

/// The same rows without the padding that fills each one out.
fn trimmed(lines: &[Line<'_>]) -> Vec<String> {
    texts(lines)
        .into_iter()
        .map(|line| line.trim_end().to_owned())
        .collect()
}

fn render(body: &str, badge: Option<&str>, width: usize) -> anyhow::Result<Vec<Line<'static>>> {
    let author = Author::agent("Copilot");
    let message = Message {
        author: &author,
        name: "Copilot",
        created: 0,
        badge,
    };
    let body = body_layout(body, width, &Highlighter::plain());
    Ok(message_lines(&theme()?, &message, &body, 0, width, false))
}

#[test]
fn plain_sentence_renders_as_itself_under_the_header() -> anyhow::Result<()> {
    let lines = render("please check this", None, 40)?;
    assert_eq!(
        trimmed(&lines),
        ["  Copilot  just now", "    please check this"]
    );
    Ok(())
}

#[test]
fn a_newline_stays_a_line_break() -> anyhow::Result<()> {
    let lines = render("first\nsecond", None, 40)?;
    assert_eq!(trimmed(&lines)[1..], ["    first", "    second"]);
    Ok(())
}

#[test]
fn markdown_blocks_render_and_wrap_to_the_indented_width() -> anyhow::Result<()> {
    let body = "Two **points**:\n\n- first\n- second `x`\n\n```rust\nfn a() {}\n```\n";
    let lines = render(body, Some("proposes resolving"), 30)?;
    let rows = trimmed(&lines);
    assert_eq!(rows[0], "  Copilot  just now  [proposes resolving]");
    assert!(rows.iter().any(|t| t == "    Two points:"), "{rows:?}");
    assert!(rows.iter().any(|t| t.contains("• first")), "{rows:?}");
    assert!(rows.iter().any(|t| t.contains("fn a() {}")), "{rows:?}");
    let bold = lines[1].spans.iter().find(|s| s.content == "points");
    assert!(
        bold.is_some_and(|s| s.style.add_modifier.contains(Modifier::BOLD)),
        "strong span"
    );
    let long = "word ".repeat(20);
    let wrapped = render(&long, None, 30)?;
    assert!(wrapped.len() > 3, "wrapped: {}", wrapped.len());
    assert!(texts(&wrapped).iter().all(|t| display_width(t) == 30));
    Ok(())
}

#[test]
fn every_row_fills_the_width_so_the_background_reaches_the_edge() -> anyhow::Result<()> {
    let lines = render("short", None, 24)?;
    assert!(texts(&lines).iter().all(|line| display_width(line) == 24));
    Ok(())
}

/// ADR 0071: the cursor's message keeps its author's stripe and
/// gets the bar down its rows and a bold name; another author's
/// message sits on its own stripe with no bar.
#[test]
fn a_selected_message_keeps_its_stripe_and_gets_the_bar() -> anyhow::Result<()> {
    let theme = theme()?;
    let message = Message {
        author: &Author::User,
        name: "User",
        created: 0,
        badge: None,
    };
    let body = body_layout("selected\nrows", 24, &Highlighter::plain());
    let lines = message_lines(&theme, &message, &body, 0, 24, true);
    assert!(
        lines
            .iter()
            .all(|line| line.style.bg == theme.thread_user.bg)
    );
    assert!(texts(&lines).iter().all(|line| display_width(line) == 24));
    assert!(
        texts(&lines).iter().all(|line| line.starts_with("▎")),
        "{:?}",
        texts(&lines)
    );
    let name = lines[0].spans.iter().find(|s| s.content == "User");
    assert!(
        name.is_some_and(
            |s| s.style.add_modifier.contains(Modifier::BOLD) && s.style.fg == theme.thread_user.fg
        ),
        "bold in the user's colour"
    );
    let agent = Author::agent("coder");
    let other = Message {
        author: &agent,
        name: "coder",
        created: 0,
        badge: None,
    };
    let body = body_layout("theirs", 24, &Highlighter::plain());
    let lines = message_lines(&theme, &other, &body, 0, 24, false);
    assert!(
        lines
            .iter()
            .all(|line| line.style.bg == theme.thread_agent.bg)
    );
    assert!(texts(&lines).iter().all(|line| line.starts_with(' ')));
    Ok(())
}

#[test]
fn origin_context_is_bounded_keeps_both_ends_and_reuses_highlights() -> anyhow::Result<()> {
    let dir = TempDir::new("origin-context-layout")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let text = (1..=400)
        .map(|line| format!("let value_{line} = {line};"))
        .collect::<Vec<_>>()
        .join("\n");
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("src/lib.rs"),
            LineRange::new(1, 400),
            "review",
        ),
        &text,
        1,
    )?;
    let thread = store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
    let cache = OriginContextCache::default();
    let highlighter = Highlighter::new("base16-ocean.dark")?;

    let narrow = cache
        .layout(thread, 40, &highlighter)
        .ok_or_else(|| anyhow::anyhow!("line origin"))?;
    assert!(narrow.is_truncated());
    assert!(narrow.rows().len() >= MAX_ORIGIN_CONTEXT_ROWS);
    assert_eq!(narrow.rows().iter().filter(|row| row.omitted).count(), 1);
    let rendered = |row: &OriginContextRow| {
        row.line
            .spans()
            .iter()
            .map(fathomable_core::layout::Span::text)
            .collect::<String>()
    };
    assert!(
        narrow
            .rows()
            .iter()
            .any(|row| rendered(row).contains("value_1"))
    );
    assert!(
        narrow
            .rows()
            .iter()
            .any(|row| rendered(row).contains("value_400"))
    );
    assert!(narrow.rows().iter().any(|row| {
        row.line
            .spans()
            .iter()
            .any(|span| span.style().fg.is_some())
    }));
    let entries = cache.entries.borrow();
    let prepared = &entries
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("cached origin"))?
        .prepared;
    assert!(prepared.text.len() <= MAX_ORIGIN_EVIDENCE_BYTES);
    assert!(prepared.lines.len() <= MAX_ORIGIN_CONTEXT_ROWS);
    drop(entries);

    let _wide = cache
        .layout(thread, 80, &highlighter)
        .ok_or_else(|| anyhow::anyhow!("wide line origin"))?;
    let _narrow_again = cache
        .layout(thread, 40, &highlighter)
        .ok_or_else(|| anyhow::anyhow!("cached line origin"))?;
    assert_eq!(cache.preparations(), 1);
    Ok(())
}

#[test]
fn legacy_origin_without_context_uses_the_exact_stored_snippet() -> anyhow::Result<()> {
    let dir = TempDir::new("origin-context-snippet")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("src/lib.rs"),
            LineRange::new(2, 3),
            "review",
        ),
        "outside\nselected one\nselected two\nafter\n",
        1,
    )?;
    let thread = store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
    let mut value = serde_json::to_value(thread)?;
    value
        .get_mut("origin")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("origin object"))?
        .remove("context");
    let legacy: Thread = serde_json::from_value(value)?;
    let layout = OriginContextCache::default()
        .layout(&legacy, 80, &Highlighter::plain())
        .ok_or_else(|| anyhow::anyhow!("line origin"))?;
    let rendered = layout
        .rows()
        .iter()
        .map(|row| {
            row.line
                .spans()
                .iter()
                .map(fathomable_core::layout::Span::text)
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["selected one", "selected two"]);
    assert!(layout.rows().iter().all(|row| row.selected));
    Ok(())
}

#[test]
fn every_wrapped_selected_and_omission_continuation_keeps_its_kind() -> anyhow::Result<()> {
    let dir = TempDir::new("origin-context-wrapped-kinds")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let text = (1..=400)
        .map(|line| format!("selected source line {line} with a long retained suffix"))
        .collect::<Vec<_>>()
        .join("\n");
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("src/lib.rs"),
            LineRange::new(1, 400),
            "review",
        ),
        &text,
        1,
    )?;
    let thread = store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
    let layout = OriginContextCache::default()
        .layout(thread, 9, &Highlighter::plain())
        .ok_or_else(|| anyhow::anyhow!("layout"))?;
    let rendered = |row: &OriginContextRow| {
        row.line
            .spans()
            .iter()
            .map(fathomable_core::layout::Span::text)
            .collect::<String>()
    };
    let omission: Vec<_> = layout
        .rows()
        .iter()
        .filter(|row| rendered(row).contains("omitted") || row.omitted)
        .collect();
    assert!(omission.len() > 1, "the omission marker must wrap");
    assert!(omission.iter().all(|row| row.omitted));
    assert!(
        layout
            .rows()
            .iter()
            .filter(|row| !row.omitted)
            .all(|row| row.selected),
        "every continuation of each retained selected line keeps its tint"
    );
    Ok(())
}

#[test]
fn eof_blank_origin_lines_remain_selected_rows_without_truncation() -> anyhow::Result<()> {
    let dir = TempDir::new("origin-context-eof-blanks")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("src/lib.rs"),
            LineRange::new(2, 2),
            "review",
        ),
        "alpha\n\n",
        1,
    )?;
    let thread = store
        .thread(&id)
        .ok_or_else(|| anyhow::anyhow!("thread"))?
        .clone();
    let cache = OriginContextCache::default();
    let layout = cache
        .layout(&thread, 80, &Highlighter::plain())
        .ok_or_else(|| anyhow::anyhow!("line origin"))?;
    assert_eq!(
        layout
            .rows()
            .iter()
            .map(|row| row.line.text())
            .collect::<Vec<_>>(),
        ["alpha", ""]
    );
    assert!(!layout.rows()[0].selected);
    assert!(layout.rows()[1].selected);
    assert!(!layout.is_truncated());

    let multiple = store.annotate(
        Draft::new(
            Author::User,
            Path::new("src/lib.rs"),
            LineRange::new(2, 3),
            "review",
        ),
        "alpha\n\n\n",
        2,
    )?;
    let thread = store
        .thread(&multiple)
        .ok_or_else(|| anyhow::anyhow!("thread"))?;
    let layout = cache
        .layout(thread, 80, &Highlighter::plain())
        .ok_or_else(|| anyhow::anyhow!("line origin"))?;
    let rendered = layout
        .rows()
        .iter()
        .map(|row| row.line.text())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["alpha", "", ""]);
    assert_eq!(
        layout
            .rows()
            .iter()
            .map(|row| row.selected)
            .collect::<Vec<_>>(),
        [false, true, true]
    );
    assert!(!layout.is_truncated());
    Ok(())
}

#[test]
fn eof_blank_row_costs_no_extra_byte_at_the_evidence_bound() -> anyhow::Result<()> {
    let dir = TempDir::new("origin-context-eof-blank-bound")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let text = format!("{}\n\n", "a".repeat(MAX_ORIGIN_EVIDENCE_BYTES - 2));
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("src/lib.rs"),
            LineRange::new(2, 2),
            "review",
        ),
        &text,
        1,
    )?;
    let thread = store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
    let cache = OriginContextCache::default();
    let layout = cache
        .layout(thread, 80, &Highlighter::plain())
        .ok_or_else(|| anyhow::anyhow!("line origin"))?;
    let entries = cache.entries.borrow();
    let prepared = &entries
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("cached origin"))?
        .prepared;

    assert!(prepared.text.len() <= MAX_ORIGIN_EVIDENCE_BYTES);
    assert!(!layout.is_truncated());
    assert!(
        layout
            .rows()
            .last()
            .is_some_and(|row| row.selected && row.line.text().is_empty())
    );
    Ok(())
}
