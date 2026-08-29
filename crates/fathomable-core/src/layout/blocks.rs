// @okf-doc: /decisions/0004-markdown-rendering.md
//! Block tree built from `pulldown-cmark` events with source offsets.

use std::iter::Peekable;
use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use super::{Breaks, Face, Style};

/// Source-mapped inline content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Inline {
    Text {
        text: String,
        style: Style,
        source: Range<usize>,
    },
    HardBreak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

/// Horizontal alignment of a table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Left,
    Center,
    Right,
}

impl From<Alignment> for Align {
    fn from(alignment: Alignment) -> Self {
        match alignment {
            Alignment::Center => Self::Center,
            Alignment::Right => Self::Right,
            Alignment::None | Alignment::Left => Self::Left,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Table {
    pub align: Vec<Align>,
    pub head: Vec<Vec<Inline>>,
    pub rows: Vec<Vec<Vec<Inline>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Block {
    Paragraph(Vec<Inline>),
    /// A tight list item's text: inline content with no paragraph tag.
    Tight(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    Code {
        text: String,
        source: Range<usize>,
        /// The first word of the fence info string, empty when absent.
        lang: String,
    },
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Quote(Vec<Block>),
    Table(Table),
    Rule(Range<usize>),
    Footnote {
        label: String,
        blocks: Vec<Block>,
    },
    Html {
        text: String,
        source: Range<usize>,
    },
}

/// Parse `text` into a block tree, treating a single newline as `breaks`
/// says.
pub(super) fn parse(text: &str, breaks: Breaks) -> Vec<Block> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let events = Parser::new_ext(text, options).into_offset_iter();
    let mut parser = BlockParser {
        events: events.peekable(),
        breaks,
    };
    parser.blocks()
}

struct BlockParser<'a, I: Iterator<Item = (Event<'a>, Range<usize>)>> {
    events: Peekable<I>,
    breaks: Breaks,
}

impl<'a, I: Iterator<Item = (Event<'a>, Range<usize>)>> BlockParser<'a, I> {
    /// Parse blocks until an unowned `End` tag, which is left unconsumed.
    fn blocks(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some((event, _)) = self.events.peek() {
            match event {
                Event::End(_) => break,
                Event::Start(tag) if is_block(tag) => {
                    let block = self.block();
                    blocks.extend(block);
                }
                Event::Rule => {
                    if let Some((_, range)) = self.events.next() {
                        blocks.push(Block::Rule(range));
                    }
                }
                Event::Html(_) => {
                    if let Some((Event::Html(html), range)) = self.events.next() {
                        blocks.push(Block::Html {
                            text: html.trim_end().to_owned(),
                            source: range,
                        });
                    }
                }
                _ => {
                    // Tight list items carry inline events without a paragraph.
                    let inlines = self.inlines();
                    if !inlines.is_empty() {
                        blocks.push(Block::Tight(inlines));
                    }
                }
            }
        }
        blocks
    }

    /// The text events of a code or metadata block, trailing newlines trimmed.
    fn code_text(&mut self) -> String {
        let mut text = String::new();
        while let Some((Event::Text(_), _)) = self.events.peek() {
            if let Some((Event::Text(part), _)) = self.events.next() {
                text.push_str(&part);
            }
        }
        text.trim_end_matches('\n').to_owned()
    }

    fn block(&mut self) -> Option<Block> {
        let (Event::Start(tag), range) = self.events.next()? else {
            return None;
        };
        let block = match tag {
            Tag::Paragraph => Block::Paragraph(self.inlines()),
            Tag::Heading { level, .. } => Block::Heading(level as u8, self.inlines()),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                let text = self.code_text();
                Block::Code {
                    text,
                    source: range,
                    lang,
                }
            }
            Tag::MetadataBlock(_) => Block::Code {
                text: self.code_text(),
                source: range,
                lang: String::new(),
            },
            Tag::List(start) => Block::List {
                start,
                items: self.items(),
            },
            Tag::BlockQuote(_) => Block::Quote(self.blocks()),
            Tag::Table(align) => {
                Block::Table(self.table(align.into_iter().map(Align::from).collect()))
            }
            Tag::FootnoteDefinition(label) => Block::Footnote {
                label: label.into_string(),
                blocks: self.blocks(),
            },
            Tag::HtmlBlock => {
                let mut text = String::new();
                while let Some((Event::Html(_), _)) = self.events.peek() {
                    if let Some((Event::Html(part), _)) = self.events.next() {
                        text.push_str(&part);
                    }
                }
                Block::Html {
                    text: text.trim_end().to_owned(),
                    source: range,
                }
            }
            _ => {
                // Unsupported container (definition lists, metadata): flatten.
                let blocks = self.blocks();
                self.end();
                return blocks.into_iter().next();
            }
        };
        self.end();
        Some(block)
    }

    /// Consume the `End` tag closing the current container, if present.
    fn end(&mut self) {
        if let Some((Event::End(_), _)) = self.events.peek() {
            self.events.next();
        }
    }

    fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while let Some((Event::Start(Tag::Item), _)) = self.events.peek() {
            self.events.next();
            let task = match self.events.peek() {
                Some((Event::TaskListMarker(done), _)) => {
                    let done = *done;
                    self.events.next();
                    Some(done)
                }
                _ => None,
            };
            let blocks = self.blocks();
            self.end();
            items.push(Item { task, blocks });
        }
        items
    }

    fn table(&mut self, align: Vec<Align>) -> Table {
        let mut table = Table {
            align,
            head: Vec::new(),
            rows: Vec::new(),
        };
        loop {
            match self.events.peek() {
                Some((Event::Start(Tag::TableHead), _)) => {
                    self.events.next();
                    table.head = self.cells();
                    self.end();
                }
                Some((Event::Start(Tag::TableRow), _)) => {
                    self.events.next();
                    let row = self.cells();
                    self.end();
                    table.rows.push(row);
                }
                _ => break,
            }
        }
        table
    }

    fn cells(&mut self) -> Vec<Vec<Inline>> {
        let mut cells = Vec::new();
        while let Some((Event::Start(Tag::TableCell), _)) = self.events.peek() {
            self.events.next();
            cells.push(self.inlines());
            self.end();
        }
        cells
    }

    /// Parse inline content until an unowned `End` tag or a block start.
    fn inlines(&mut self) -> Vec<Inline> {
        let mut out = Vec::new();
        let mut stack: Vec<Style> = vec![Style::default()];
        while let Some((event, _)) = self.events.peek() {
            match event {
                Event::End(end) if is_inline_end(*end) => {
                    self.events.next();
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    continue;
                }
                Event::End(_) => break,
                Event::Start(tag) if is_block(tag) => break,
                _ => {}
            }
            let Some((event, range)) = self.events.next() else {
                break;
            };
            let style = stack.last().cloned().unwrap_or_default();
            match event {
                Event::Start(tag) => stack.push(inline_style(&style, &tag)),
                Event::Text(text) => out.push(Inline::Text {
                    text: text.into_string(),
                    style,
                    source: range,
                }),
                Event::Code(code) => out.push(Inline::Text {
                    text: code.into_string(),
                    style: Style {
                        face: Face::Code,
                        ..style
                    },
                    source: range,
                }),
                Event::InlineHtml(html) | Event::Html(html) => out.push(Inline::Text {
                    text: html.into_string(),
                    style,
                    source: range,
                }),
                Event::InlineMath(math) | Event::DisplayMath(math) => out.push(Inline::Text {
                    text: math.into_string(),
                    style: Style {
                        face: Face::Code,
                        ..style
                    },
                    source: range,
                }),
                Event::FootnoteReference(label) => out.push(Inline::Text {
                    text: format!("[^{label}]"),
                    style: Style {
                        face: Face::Marker,
                        ..style
                    },
                    source: range,
                }),
                Event::SoftBreak => out.push(match self.breaks {
                    Breaks::Soft => Inline::Text {
                        text: " ".to_owned(),
                        style,
                        source: range,
                    },
                    Breaks::Hard => Inline::HardBreak,
                }),
                Event::HardBreak => out.push(Inline::HardBreak),
                Event::TaskListMarker(done) => out.push(Inline::Text {
                    text: task_marker(done).to_owned(),
                    style: Style {
                        face: Face::Marker,
                        ..style
                    },
                    source: range,
                }),
                Event::End(_) | Event::Rule => {}
            }
        }
        out
    }
}

/// The rendered form of a task list checkbox.
pub(super) fn task_marker(done: bool) -> &'static str {
    if done { "[x]" } else { "[ ]" }
}

fn inline_style(parent: &Style, tag: &Tag<'_>) -> Style {
    let mut style = parent.clone();
    match tag {
        Tag::Emphasis => style.emphasis = true,
        Tag::Strong => style.strong = true,
        Tag::Strikethrough => style.strikethrough = true,
        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
            style.face = Face::Link(dest_url.to_string());
        }
        _ => {}
    }
    style
}

fn is_block(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::FootnoteDefinition(_)
            | Tag::Table(_)
            | Tag::DefinitionList
            | Tag::MetadataBlock(_)
    )
}

fn is_inline_end(end: TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::Superscript
            | TagEnd::Subscript
    )
}
