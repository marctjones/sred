//! Markdown backend: CommonMark + GFM (tables/strikethrough/tasklists) parse via
//! `pulldown-cmark`, and a direct model→Markdown serializer for exact output
//! control.

use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::format::{Caps, DocumentFormat, FormatError};
use crate::model::{Block, Document, Format, Inline, List, MarkSet};

pub struct Markdown;

impl DocumentFormat for Markdown {
    fn id(&self) -> Format {
        Format::Markdown
    }

    fn capabilities(&self) -> Caps {
        Caps::HEADINGS
            | Caps::EMPHASIS
            | Caps::CODE
            | Caps::LISTS
            | Caps::MATH
            | Caps::IMAGES
            | Caps::BLOCKQUOTE
            | Caps::STRIKE
    }

    fn parse(&self, src: &str) -> Result<Document, FormatError> {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_TASKLISTS);
        let parser = Parser::new_ext(src, opts);
        let mut p = TreeBuilder {
            iter: parser.peekable(),
        };
        let blocks = p.parse_blocks(None);
        Ok(Document {
            blocks,
            meta: crate::model::DocMeta {
                origin: Some(Format::Markdown),
            },
        })
    }

    fn serialize(&self, doc: &Document) -> Result<String, FormatError> {
        let mut out = String::new();
        for (i, b) in doc.blocks.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            write_block(b, &mut out, 0);
        }
        Ok(out)
    }
}

// ---- parse: events -> model ------------------------------------------------

type PeekParser<'a> = std::iter::Peekable<Parser<'a>>;

struct TreeBuilder<'a> {
    iter: PeekParser<'a>,
}

impl<'a> TreeBuilder<'a> {
    /// Parse blocks until we hit the `End` matching `stop` (or run out).
    fn parse_blocks(&mut self, stop: Option<&TagEnd>) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some(ev) = self.iter.peek() {
            if let (Event::End(end), Some(stop)) = (ev, stop) {
                if end == stop {
                    break;
                }
            }
            let ev = self.iter.next().unwrap();
            match ev {
                Event::Start(Tag::Paragraph) => {
                    let content = self.parse_inlines(&TagEnd::Paragraph);
                    self.expect_end();
                    blocks.push(Block::Paragraph(content));
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    let content = self.parse_inlines(&TagEnd::Heading(level));
                    self.expect_end();
                    blocks.push(Block::Heading {
                        level: heading_num(level),
                        content,
                    });
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(s) if !s.is_empty() => Some(s.to_string()),
                        _ => None,
                    };
                    let mut code = String::new();
                    while let Some(ev) = self.iter.peek() {
                        if matches!(ev, Event::End(TagEnd::CodeBlock)) {
                            break;
                        }
                        if let Some(Event::Text(t)) = self.iter.next() {
                            code.push_str(&t);
                        }
                    }
                    self.expect_end();
                    if code.ends_with('\n') {
                        code.pop();
                    }
                    blocks.push(Block::CodeBlock { lang, code });
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    let inner = self.parse_blocks(Some(&TagEnd::BlockQuote(None)));
                    // BlockQuote end may carry a kind; consume whatever End is next.
                    self.iter.next();
                    blocks.push(Block::BlockQuote(inner));
                }
                Event::Start(Tag::List(first)) => {
                    let ordered = first.is_some();
                    let end = TagEnd::List(ordered);
                    let mut items = Vec::new();
                    while let Some(ev) = self.iter.peek() {
                        if matches!(ev, Event::End(e) if *e == end) {
                            break;
                        }
                        if matches!(ev, Event::Start(Tag::Item)) {
                            self.iter.next();
                            let item = self.parse_blocks(Some(&TagEnd::Item));
                            self.expect_end();
                            items.push(item);
                        } else {
                            self.iter.next();
                        }
                    }
                    self.expect_end();
                    blocks.push(Block::List(List {
                        ordered,
                        tight: true,
                        items,
                    }));
                }
                Event::Rule => blocks.push(Block::ThematicBreak),
                Event::Html(h) | Event::InlineHtml(h) => blocks.push(Block::Raw {
                    format: Format::Markdown,
                    src: h.to_string(),
                }),
                // Anything else at block level: wrap loose inline content into a paragraph.
                other => {
                    if let Some(inl) = inline_from_event(&other) {
                        blocks.push(Block::Paragraph(vec![inl]));
                    }
                }
            }
        }
        blocks
    }

    fn parse_inlines(&mut self, stop: &TagEnd) -> Vec<Inline> {
        let mut out = Vec::new();
        while let Some(ev) = self.iter.peek() {
            if matches!(ev, Event::End(e) if e == stop) {
                break;
            }
            let ev = self.iter.next().unwrap();
            match ev {
                Event::Text(t) => out.push(Inline::Text(t.to_string())),
                Event::Code(c) => out.push(Inline::Styled {
                    marks: MarkSet::CODE,
                    link: None,
                    content: vec![Inline::Text(c.to_string())],
                }),
                Event::SoftBreak => out.push(Inline::Text(" ".into())),
                Event::HardBreak => out.push(Inline::LineBreak),
                Event::Start(Tag::Emphasis) => {
                    let content = self.parse_inlines(&TagEnd::Emphasis);
                    self.expect_end();
                    out.push(styled(MarkSet::ITALIC, None, content));
                }
                Event::Start(Tag::Strong) => {
                    let content = self.parse_inlines(&TagEnd::Strong);
                    self.expect_end();
                    out.push(styled(MarkSet::BOLD, None, content));
                }
                Event::Start(Tag::Strikethrough) => {
                    let content = self.parse_inlines(&TagEnd::Strikethrough);
                    self.expect_end();
                    out.push(styled(MarkSet::STRIKE, None, content));
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    let content = self.parse_inlines(&TagEnd::Link);
                    self.expect_end();
                    out.push(styled(MarkSet::LINK, Some(dest_url.to_string()), content));
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    // alt text is the inline content until the image end
                    let alt_inl = self.parse_inlines(&TagEnd::Image);
                    self.expect_end();
                    let mut alt = String::new();
                    crate::model::Document {
                        blocks: vec![Block::Paragraph(alt_inl)],
                        ..Default::default()
                    }
                    .plain_text()
                    .clone_into(&mut alt);
                    out.push(Inline::Image {
                        src: dest_url.to_string(),
                        alt,
                    });
                }
                Event::InlineMath(m) => out.push(Inline::Math {
                    display: false,
                    src: m.to_string(),
                }),
                Event::DisplayMath(m) => out.push(Inline::Math {
                    display: true,
                    src: m.to_string(),
                }),
                Event::Html(h) | Event::InlineHtml(h) => out.push(Inline::Raw {
                    format: Format::Markdown,
                    src: h.to_string(),
                }),
                _ => {}
            }
        }
        out
    }

    fn expect_end(&mut self) {
        // consume the matching End event
        let _ = self.iter.next();
    }
}

fn inline_from_event(ev: &Event) -> Option<Inline> {
    match ev {
        Event::Text(t) => Some(Inline::Text(t.to_string())),
        _ => None,
    }
}

fn styled(marks: MarkSet, link: Option<String>, content: Vec<Inline>) -> Inline {
    Inline::Styled {
        marks,
        link,
        content,
    }
}

fn heading_num(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

// ---- serialize: model -> markdown -----------------------------------------

fn write_block(b: &Block, out: &mut String, indent: usize) {
    let pad = "  ".repeat(indent);
    match b {
        Block::Paragraph(inl) => {
            out.push_str(&pad);
            write_inlines(inl, out);
            out.push('\n');
        }
        Block::Heading { level, content } => {
            out.push_str(&"#".repeat((*level).min(6) as usize));
            out.push(' ');
            write_inlines(content, out);
            out.push('\n');
        }
        Block::CodeBlock { lang, code } => {
            out.push_str("```");
            if let Some(l) = lang {
                out.push_str(l);
            }
            out.push('\n');
            out.push_str(code);
            if !code.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
        }
        Block::BlockQuote(blocks) => {
            let mut inner = String::new();
            for (i, b) in blocks.iter().enumerate() {
                if i > 0 {
                    inner.push('\n');
                }
                write_block(b, &mut inner, 0);
            }
            for line in inner.trim_end().split('\n') {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
        }
        Block::List(list) => {
            for (i, item) in list.items.iter().enumerate() {
                let marker = if list.ordered {
                    format!("{}. ", i + 1)
                } else {
                    "- ".to_string()
                };
                out.push_str(&pad);
                out.push_str(&marker);
                // first block inline with the marker, rest indented
                let mut item_str = String::new();
                for (j, b) in item.iter().enumerate() {
                    write_block(b, &mut item_str, if j == 0 { 0 } else { indent + 1 });
                }
                let trimmed = item_str.trim_end_matches('\n');
                out.push_str(trimmed);
                out.push('\n');
            }
        }
        Block::ThematicBreak => out.push_str("---\n"),
        Block::Raw { src, .. } => {
            out.push_str(src);
            if !src.ends_with('\n') {
                out.push('\n');
            }
        }
    }
}

fn write_inlines(inl: &[Inline], out: &mut String) {
    for i in inl {
        write_inline(i, out);
    }
}

fn write_inline(i: &Inline, out: &mut String) {
    match i {
        Inline::Text(t) => out.push_str(t),
        Inline::LineBreak => out.push_str("  \n"),
        Inline::Image { src, alt } => {
            out.push_str("![");
            out.push_str(alt);
            out.push_str("](");
            out.push_str(src);
            out.push(')');
        }
        Inline::Math { display, src } => {
            let d = if *display { "$$" } else { "$" };
            out.push_str(d);
            out.push_str(src);
            out.push_str(d);
        }
        Inline::Raw { src, .. } => out.push_str(src),
        Inline::Styled {
            marks,
            link,
            content,
        } => {
            if marks.contains(MarkSet::CODE) {
                out.push('`');
                write_inlines(content, out);
                out.push('`');
                return;
            }
            let (open, close) = mark_delims(*marks);
            out.push_str(&open);
            if marks.contains(MarkSet::LINK) {
                out.push('[');
                write_inlines(content, out);
                out.push_str("](");
                out.push_str(link.as_deref().unwrap_or(""));
                out.push(')');
            } else {
                write_inlines(content, out);
            }
            out.push_str(&close);
        }
    }
}

fn mark_delims(marks: MarkSet) -> (String, String) {
    let mut open = String::new();
    let mut close = String::new();
    if marks.contains(MarkSet::BOLD) {
        open.push_str("**");
        close.insert_str(0, "**");
    }
    if marks.contains(MarkSet::ITALIC) {
        open.push('*');
        close.insert(0, '*');
    }
    if marks.contains(MarkSet::STRIKE) {
        open.push_str("~~");
        close.insert_str(0, "~~");
    }
    (open, close)
}
