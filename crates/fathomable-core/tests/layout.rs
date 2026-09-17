//! Behaviour tests for the Markdown layout engine (ADR 0004, ADR 0010).

use std::error::Error;

use fathomable_core::layout::{Face, Layout, Line, LineIndex};

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

    let layout = Layout::render("abcdefgh\n", 3);
    assert_eq!(texts(&layout), ["abc", "def", "gh"]);
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
fn code_blocks_wrap_and_map_lines() -> TestResult {
    let src = "para\n\n```rust\nlet x = 1;\n\nlet y = a_very_long_identifier_that_exceeds_the_width;\n```\n";
    let layout = Layout::render(src, 20);
    let lines = layout.lines();
    assert_eq!(lines[2].text(), "let x = 1;");
    assert_eq!(lines[2].spans()[0].style().face, Face::CodeBlock);
    assert_eq!(lines[3].text(), "");
    assert!(lines.iter().all(|line| line.width() <= 20));
    let long: String = lines[4..].iter().map(Line::text).collect();
    assert_eq!(
        long,
        "let y = a_very_long_identifier_that_exceeds_the_width;"
    );
    assert_eq!(numbers(&layout)[2..5], [Some(4), Some(5), Some(6)]);
    assert!(numbers(&layout)[5..].iter().all(Option::is_none));
    let range = lines[2].source().ok_or("no source")?;
    assert_eq!(&src[range], "let x = 1;");
    Ok(())
}

#[test]
fn message_code_blocks_prefer_word_boundaries() -> TestResult {
    use fathomable_core::highlight::Highlighter;

    let line = "Knowledge Relations, Supplied Evidence, Constraints, and Claim Provider Metadata.";
    let source = format!("```markdown\n{line}\n```\n");
    let highlighter = Highlighter::new("base16-ocean.dark")?;
    let message = Layout::render_message(&source, 60, &highlighter);
    assert_eq!(
        texts(&message),
        [
            "Knowledge Relations, Supplied Evidence, Constraints, and",
            "Claim Provider Metadata.",
        ]
    );
    assert!(message.lines().iter().all(|line| line.width() <= 60));

    assert_eq!(
        texts(&Layout::render(&source, 60)),
        [
            "Knowledge Relations, Supplied Evidence, Constraints, and Cla",
            "im Provider Metadata.",
        ],
        "file code remains hard-wrapped"
    );

    let long = Layout::render_message("```\nshort abcdefghijkl\n```\n", 10, &Highlighter::plain());
    assert_eq!(texts(&long), ["short", "abcdefghij", "kl"]);
    Ok(())
}

#[test]
fn blockquotes_prefix_every_line() {
    let layout = Layout::render("> quoted text here\n>\n> more\n", 12);
    assert_eq!(texts(&layout), ["│ quoted", "│ text here", "│ ", "│ more"]);
}

#[test]
fn source_view_wraps_without_changing_source_text() -> TestResult {
    let src = "# Title\n\n- item with **bold**\n";
    let layout = Layout::source(src, 10);
    assert_eq!(texts(&layout), ["# Title", "", "- item wit", "h **bold**"]);
    assert_eq!(numbers(&layout), [Some(1), Some(2), Some(3), None]);
    let range = layout.lines()[2].source().ok_or("no source")?;
    assert_eq!(&src[range], "- item wit");
    assert!(layout.lines().iter().all(|line| line.width() <= 10));
    Ok(())
}

#[test]
fn every_rendered_markdown_line_fits_the_pane() {
    let src = "some prose that wraps around\n\n> ```\n> let x = 1234567890;\n> ```\n\n\
               | a | b |\n| --- | --- |\n| 1 | 2 |\n";
    let layout = Layout::render(src, 12);
    assert!(layout.lines().iter().all(|line| line.width() <= 12));
    assert!(
        texts(&layout)
            .windows(2)
            .any(|lines| lines == ["│ let x = 12", "│ 34567890;"]),
        "quoted code keeps its prefix while wrapping"
    );

    let wide = "| alpha | beta | gamma | delta |\n| --- | --- | --- | --- |\n| 1 | 2 | 3 | 4 |\n";
    let squeezed = Layout::render(wide, 10);
    assert!(
        squeezed.lines().iter().all(|line| line.width() <= 10),
        "a table wider than the pane wraps"
    );
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
    let empty = Layout::render("", 80);
    assert!(empty.lines().is_empty());
    assert_eq!(empty.line_at_offset(0), None);
}

#[test]
fn layout_reflows_text_and_preserves_source_boundaries() -> TestResult {
    let src = "## Title\n\nalpha beta\nsecond line\n\nlast\n";
    let layout = Layout::render(src, 10);
    assert_eq!(
        texts(&layout),
        ["Title", "", "alpha beta", "second", "line", "", "last"]
    );
    assert_eq!(
        numbers(&layout),
        [Some(1), None, Some(3), Some(4), None, None, Some(6)]
    );
    for (line, expected) in [
        (&layout.lines()[0], "Title"),
        (&layout.lines()[2], "alpha beta"),
        (&layout.lines()[3], "second"),
        (&layout.lines()[4], "line"),
        (&layout.lines()[6], "last"),
    ] {
        let range = line.source().ok_or("line has no source")?;
        assert_eq!(&src[range], expected);
    }

    let wide = Layout::render(src, 22);
    assert_eq!(
        texts(&wide),
        ["Title", "", "alpha beta second line", "", "last"]
    );
    assert_eq!(numbers(&wide), [Some(1), None, Some(3), None, Some(6)]);
    Ok(())
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
    let highlights = highlighter.highlight(text, "rs");
    let cached = Layout::source_with_highlights(text, 12, highlights.as_ref());
    assert_eq!(cached, coloured, "cached highlighting is the same layout");
    assert_eq!(texts(&plain), texts(&coloured), "colouring changes no text");
    assert!(
        coloured.lines().iter().all(|line| line.width() <= 12),
        "source lines wrap to the pane"
    );
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
