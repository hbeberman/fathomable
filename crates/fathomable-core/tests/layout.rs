//! Behaviour tests for the Markdown layout engine (ADR 0004, ADR 0010).

use std::error::Error;

use fathomable_core::layout::{Face, Layout, Line, LineIndex, wrap_text};

type TestResult = Result<(), Box<dyn Error>>;

fn texts(layout: &Layout) -> Vec<String> {
    layout.lines().iter().map(Line::text).collect()
}

fn numbers(layout: &Layout) -> Vec<Option<usize>> {
    layout.lines().iter().map(Line::source_line).collect()
}

#[test]
fn paragraph_wraps_at_words_and_records_source() -> TestResult {
    let src = "one two three four five\n";
    let layout = Layout::render(src, 9);
    assert_eq!(texts(&layout), ["one two", "three", "four five"]);
    let line = &layout.lines()[2];
    let range = line.source().ok_or("line has no source")?;
    assert_eq!(&src[range], "four five");
    // Column 5 of "four five" is the 'f' of "five".
    let offset = line.source_at(5).ok_or("no source at column")?;
    assert_eq!(&src[offset..offset + 4], "five");
    // Only the first line of the paragraph carries a gutter number.
    assert_eq!(numbers(&layout), [Some(1), None, None]);
    Ok(())
}

#[test]
fn wrapping_counts_wide_and_combining_characters() {
    // Each CJK character is two cells; "é" written as e + U+0301 is one cell.
    let src = "日本語 日本語 e\u{301}e\u{301}\n";
    let layout = Layout::render(src, 9);
    assert_eq!(texts(&layout), ["日本語", "日本語 e\u{301}e\u{301}"]);
    for line in layout.lines() {
        assert!(line.width() <= 9, "{:?} is wider than 9", line.text());
    }
    // A single word wider than the pane breaks between clusters, not inside one.
    let layout = Layout::render("日本語日本語\n", 5);
    assert_eq!(texts(&layout), ["日本", "語日", "本語"]);
}

#[test]
fn headings_and_inline_styles_carry_faces() -> TestResult {
    let layout = Layout::render("## Title\n\nplain *em* **strong** `code` ~~gone~~\n", 80);
    let heading = &layout.lines()[0];
    assert_eq!(heading.text(), "Title");
    assert_eq!(heading.spans()[0].style().face, Face::Heading(2));
    let body = &layout.lines()[2];
    let find = |needle: &str| {
        body.spans()
            .iter()
            .find(|span| span.text() == needle)
            .ok_or(format!("no span {needle:?} in {:?}", body.text()))
    };
    assert!(find("em")?.style().emphasis);
    assert!(find("strong")?.style().strong);
    assert_eq!(find("code")?.style().face, Face::Code);
    assert!(find("gone")?.style().strikethrough);
    Ok(())
}

#[test]
fn links_keep_their_url_and_source() -> TestResult {
    let src = "see [the docs](https://example.com/x) now\n";
    let layout = Layout::render(src, 80);
    let span = layout.lines()[0]
        .spans()
        .iter()
        .find(|span| span.text() == "the docs")
        .ok_or("no link span")?;
    assert_eq!(
        span.style().face,
        Face::Link("https://example.com/x".to_owned())
    );
    let range = span.source().ok_or("link has no source")?;
    assert_eq!(&src[range], "the docs");
    Ok(())
}

#[test]
fn nested_and_task_lists_indent_and_number() {
    let src = "- a\n  - b\n  - c\n- [x] done\n- [ ] todo\n\n1. one\n2. two\n";
    let layout = Layout::render(src, 40);
    assert_eq!(
        texts(&layout),
        [
            "• a",
            "  • b",
            "  • c",
            "• [x] done",
            "• [ ] todo",
            "",
            "1. one",
            "2. two",
        ]
    );
    assert_eq!(
        numbers(&layout),
        [
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
            None,
            Some(7),
            Some(8)
        ]
    );
}

#[test]
fn list_item_continuation_aligns_under_text() {
    let layout = Layout::render("1. alpha beta gamma\n", 12);
    assert_eq!(texts(&layout), ["1. alpha", "   beta", "   gamma"]);
}

#[test]
fn tables_align_columns_and_wrap_to_width() {
    let src = "| Name | Qty |\n|:-----|----:|\n| apple pie | 3 |\n| kiwi | 12 |\n";
    let layout = Layout::render(src, 80);
    assert_eq!(
        texts(&layout),
        [
            "┏━━━━━━━━━━━┯━━━━━┓",
            "┃ Name      │ Qty ┃",
            "┣━━━━━━━━━━━┿━━━━━┫",
            "┃ apple pie │   3 ┃",
            "┠───────────┼─────┨",
            "┃ kiwi      │  12 ┃",
            "┗━━━━━━━━━━━┷━━━━━┛",
        ]
    );
    // Header cells are bold, body cells map to source.
    assert!(layout.lines()[1].spans()[1].style().strong);
    let apple = layout.lines()[3].source().expect_source();
    assert_eq!(&src[apple], "apple pie | 3");

    let narrow = Layout::render(src, 14);
    for line in narrow.lines() {
        assert!(line.width() <= 14, "{:?} exceeds width", line.text());
    }
    assert!(
        narrow.lines().len() > 7,
        "cells should wrap onto extra rows"
    );
}

trait ExpectSource {
    fn expect_source(self) -> std::ops::Range<usize>;
}

impl ExpectSource for Option<std::ops::Range<usize>> {
    fn expect_source(self) -> std::ops::Range<usize> {
        match self {
            Some(range) => range,
            None => panic_no_source(),
        }
    }
}

#[track_caller]
fn panic_no_source() -> ! {
    // Tests are allowed to fail loudly; production code is not.
    unreachable!("line has no source range")
}

#[test]
fn footnotes_render_reference_and_definition() {
    let src = "text[^1]\n\n[^1]: the note\n";
    let layout = Layout::render(src, 80);
    assert_eq!(texts(&layout), ["text[^1]", "", "[^1]: the note"]);
    assert_eq!(numbers(&layout), [Some(1), None, Some(3)]);
}

#[test]
fn code_blocks_are_not_wrapped_and_map_lines() -> TestResult {
    let src = "para\n\n```rust\nlet x = 1;\n\nlet y = a_very_long_identifier_that_exceeds_the_width;\n```\n";
    let layout = Layout::render(src, 20);
    let lines = layout.lines();
    assert_eq!(lines[2].text(), "let x = 1;");
    assert_eq!(lines[2].spans()[0].style().face, Face::CodeBlock);
    assert_eq!(lines[3].text(), "");
    assert!(lines[4].width() > 20, "code must not wrap");
    assert_eq!(numbers(&layout)[2..5], [Some(4), Some(5), Some(6)]);
    let range = lines[2].source().ok_or("no source")?;
    assert_eq!(&src[range], "let x = 1;");
    Ok(())
}

#[test]
fn blockquotes_prefix_every_line() {
    let layout = Layout::render("> quoted text here\n>\n> more\n", 12);
    assert_eq!(texts(&layout), ["│ quoted", "│ text here", "│ ", "│ more"]);
}

#[test]
fn source_view_is_verbatim_and_never_wraps() -> TestResult {
    let src = "# Title\n\n- item with **bold**\n";
    let layout = Layout::source(src, 10);
    assert_eq!(texts(&layout), ["# Title", "", "- item with **bold**"]);
    assert_eq!(numbers(&layout), [Some(1), Some(2), Some(3)]);
    let range = layout.lines()[2].source().ok_or("no source")?;
    assert_eq!(&src[range], "- item with **bold**");
    assert!(layout.lines().iter().all(Line::is_unwrapped));
    assert_eq!(layout.unwrapped_width(), 20);
    Ok(())
}

// ADR 0029: only unwrapped lines take part in horizontal scroll.
#[test]
fn code_blocks_and_wide_tables_are_unwrapped_but_prose_is_not() -> TestResult {
    let src = "some prose that wraps around\n\n> ```\n> let x = 1234567890;\n> ```\n\n\
               | a | b |\n| --- | --- |\n| 1 | 2 |\n";
    let layout = Layout::render(src, 12);
    let lines = layout.lines();
    assert!(!lines[0].is_unwrapped(), "prose wraps");
    let code = lines
        .iter()
        .find(|line| line.text().contains("let x"))
        .ok_or("no code line")?;
    assert!(code.is_unwrapped());
    assert_eq!(code.fixed_cells(), 2, "the quote bar stays put");
    assert_eq!(code.text(), "│ let x = 1234567890;");
    assert!(
        lines
            .iter()
            .filter(|line| line.text().starts_with('┃'))
            .all(|line| !line.is_unwrapped()),
        "a table that fits its pane wraps like prose"
    );
    // The widest shiftable width is the code line minus its prefix.
    assert_eq!(layout.unwrapped_width(), 19);
    assert_eq!(Layout::render("plain prose only\n", 8).unwrapped_width(), 0);

    let wide = "| alpha | beta | gamma | delta |\n| --- | --- | --- | --- |\n| 1 | 2 | 3 | 4 |\n";
    let squeezed = Layout::render(wide, 10);
    assert!(
        squeezed.lines().iter().all(Line::is_unwrapped),
        "a table wider than the pane scrolls as a whole"
    );
    assert!(squeezed.unwrapped_width() > 10);
    Ok(())
}

#[test]
fn line_index_maps_offsets_and_columns() -> TestResult {
    let src = "ab\ncd\n\nef";
    let index = LineIndex::new(src);
    assert_eq!(index.line_count(), 4);
    assert_eq!(index.line_of(0), 1);
    assert_eq!(index.line_of(3), 2);
    assert_eq!(index.line_of(6), 3);
    assert_eq!(index.line_of(7), 4);
    assert_eq!(index.line_of(999), 4);
    assert_eq!(index.range_of(2).ok_or("no line 2")?, 3..5);
    assert_eq!(index.range_of(3).ok_or("no line 3")?, 6..6);
    assert_eq!(index.range_of(5), None);
    assert_eq!(index.column_of(src, 4), 1);
    assert_eq!(index.offset_at(src, 2, 1).ok_or("no offset")?, 4);
    assert_eq!(index.offset_at(src, 2, 9).ok_or("no offset")?, 5);
    assert_eq!(index.offset_at(src, 9, 0), None);
    let wide = "日本語";
    assert_eq!(
        LineIndex::new(wide)
            .offset_at(wide, 1, 2)
            .ok_or("no offset")?,
        3
    );
    Ok(())
}

#[test]
fn line_at_offset_finds_containing_or_nearest_line() {
    let src = "first\n\nsecond\n";
    let layout = Layout::render(src, 80);
    assert_eq!(layout.line_at_offset(2), Some(0));
    assert_eq!(layout.line_at_offset(9), Some(2));
    // Offset inside the blank gap snaps to the nearest sourced line.
    assert_eq!(layout.line_at_offset(6), Some(2));
    assert_eq!(layout.line_at_offset(1000), Some(2));
    assert_eq!(Layout::render("", 80).line_at_offset(0), None);
}

#[test]
fn layout_is_deterministic_for_a_width() {
    let src = include_str!("../../../README.md");
    assert_eq!(Layout::render(src, 72), Layout::render(src, 72));
    assert_ne!(Layout::render(src, 72), Layout::render(src, 40));
}

#[test]
fn yaml_front_matter_renders_as_a_code_block() {
    let src = "---\ntitle: X\n---\n\nbody\n";
    let layout = Layout::render(src, 40);
    assert_eq!(texts(&layout)[..3], ["title: X", "", "body"]);
    assert_eq!(layout.lines()[0].spans()[0].style().face, Face::CodeBlock);
    assert_eq!(numbers(&layout), [Some(2), None, Some(5)]);
}

#[test]
fn list_items_ending_with_links_stay_separate() {
    let src = "- [one](https://a)\n- [two](https://b) (note)\n- [three](https://c)\n";
    let layout = Layout::render(src, 40);
    assert_eq!(texts(&layout), ["• one", "• two (note)", "• three"]);
}

// ADR 0016: highlighted code blocks and source files.

#[test]
fn fenced_block_is_coloured_by_its_info_string() -> TestResult {
    use fathomable_core::highlight::Highlighter;
    let highlighter = Highlighter::new("base16-ocean.dark")?;
    let text = "Intro\n\n```rust\nfn main() {}\n```\n";
    let plain = Layout::render(text, 40);
    let coloured = Layout::render_with(text, 40, &highlighter);
    assert_eq!(
        texts(&plain),
        texts(&coloured),
        "colour never changes the text"
    );
    let code = coloured
        .lines()
        .iter()
        .find(|line| line.text() == "fn main() {}")
        .ok_or("code line missing")?;
    assert!(
        code.spans().len() > 1,
        "keyword and body are separate spans"
    );
    assert!(
        code.spans()
            .iter()
            .all(|s| s.style().face == Face::CodeBlock)
    );
    assert!(code.spans().iter().any(|s| s.style().fg.is_some()));
    // Every span still maps to its exact source bytes.
    for span in code.spans() {
        let source = span.source().ok_or("span without source")?;
        assert_eq!(&text[source], span.text());
    }
    Ok(())
}

#[test]
fn unknown_language_and_plain_highlighter_leave_code_uncoloured() -> TestResult {
    use fathomable_core::highlight::Highlighter;
    let highlighter = Highlighter::new("base16-ocean.dark")?;
    for text in [
        "```no-such-lang\nx = 1\n```\n",
        "```\nx = 1\n```\n",
        "    x = 1\n",
    ] {
        let layout = Layout::render_with(text, 40, &highlighter);
        let code = layout
            .lines()
            .iter()
            .find(|line| line.text().contains("x = 1"))
            .ok_or("code line missing")?;
        assert!(
            code.spans().iter().all(|s| s.style().fg.is_none()),
            "{text:?}"
        );
    }
    let plain = Layout::render_with("```rust\nfn x() {}\n```\n", 40, &Highlighter::plain());
    assert!(
        plain
            .lines()
            .iter()
            .flat_map(Line::spans)
            .all(|s| s.style().fg.is_none())
    );
    Ok(())
}

#[test]
fn source_layout_colours_by_extension_and_keeps_line_ranges() -> TestResult {
    use fathomable_core::highlight::Highlighter;
    let highlighter = Highlighter::new("base16-ocean.dark")?;
    let text = "fn main() {\n\n    let long_name = 1;\n}\n";
    let plain = Layout::source(text, 12);
    let coloured = Layout::source_with(text, 12, "rs", &highlighter);
    assert_eq!(texts(&plain), texts(&coloured), "colouring changes no text");
    assert_eq!(coloured.lines().len(), 4, "the long line does not wrap");
    for (a, b) in plain.lines().iter().zip(coloured.lines()) {
        assert_eq!(a.source(), b.source(), "source ranges match plain layout");
    }
    // The empty line keeps its zero-length range for the gutter.
    assert_eq!(coloured.lines()[1].source_line(), Some(2));
    assert!(
        coloured.lines()[0]
            .spans()
            .iter()
            .any(|s| s.style().fg.is_some())
    );
    let unknown = Layout::source_with(text, 12, "zzz", &highlighter);
    assert!(
        unknown
            .lines()
            .iter()
            .flat_map(Line::spans)
            .all(|s| s.style().fg.is_none())
    );
    Ok(())
}

#[test]
fn wrap_text_breaks_at_words_and_splits_wide_ones() {
    assert_eq!(wrap_text("aa bb cc", 5), vec!["aa bb", "cc"]);
    assert_eq!(wrap_text("abcdefgh", 3), vec!["abc", "def", "gh"]);
    assert_eq!(wrap_text("", 3), vec![""]);
    // Width is display cells, not chars: two wide graphemes fill four.
    assert_eq!(wrap_text("日本 語", 4), vec!["日本", "語"]);
}

/// A comment's newline is a line break; a file's is a space (ADR 0037).
#[test]
fn messages_keep_newlines_files_join_them() {
    let text = "first\nsecond\n\nthird";
    assert_eq!(
        texts(&Layout::render_message(
            text,
            40,
            &fathomable_core::highlight::Highlighter::plain()
        )),
        ["first", "second", "", "third"]
    );
    assert_eq!(
        texts(&Layout::render(text, 40)),
        ["first second", "", "third"]
    );
}
