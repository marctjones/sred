//! Typst backend.
//!
//! Typst is a *programming* typesetting language, so full WYSIWYG of arbitrary
//! Typst is out of scope. The contract is: WYSIWYG-edit the **markup** layer
//! (`= heading`, `*strong*`, `_emph_`, lists, ` ```code``` `, `$ math $`) and
//! **preserve** the code layer (`#let`, `#figure(...)`, function calls) verbatim
//! as `Raw` nodes so it round-trips losslessly.
//!
//! Parsing uses the real Typst parser (`typst-syntax`) — we never hand-roll
//! Typst grammar — and walks its lossless green tree.

use typst_syntax::{SyntaxKind, SyntaxNode};

use crate::format::{Caps, DocumentFormat, FormatError};
use crate::model::{Block, Document, Format, Inline, MarkSet};

pub struct Typst;

impl DocumentFormat for Typst {
    fn id(&self) -> Format {
        Format::Typst
    }

    fn capabilities(&self) -> Caps {
        Caps::HEADINGS | Caps::EMPHASIS | Caps::CODE | Caps::LISTS | Caps::MATH
    }

    fn parse(&self, src: &str) -> Result<Document, FormatError> {
        let root = typst_syntax::parse(src);
        let mut blocks = Vec::new();
        let mut para: Vec<Inline> = Vec::new();

        flush_markup(&root, &mut blocks, &mut para);
        if !para.is_empty() {
            blocks.push(Block::Paragraph(std::mem::take(&mut para)));
        }

        Ok(Document {
            blocks,
            meta: crate::model::DocMeta {
                origin: Some(Format::Typst),
            },
        })
    }

    fn serialize(&self, doc: &Document) -> Result<String, FormatError> {
        let mut out = String::new();
        for (i, b) in doc.blocks.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            write_block(b, &mut out);
        }
        Ok(out)
    }
}

// ---- parse: green tree -> model -------------------------------------------

/// Walk a Markup node's children, accumulating inline content into `para` and
/// pushing block-level constructs (headings, code, lists) into `blocks`.
fn flush_markup(node: &SyntaxNode, blocks: &mut Vec<Block>, para: &mut Vec<Inline>) {
    for child in node.children() {
        match child.kind() {
            SyntaxKind::Markup => flush_markup(child, blocks, para),
            SyntaxKind::Parbreak => {
                if !para.is_empty() {
                    blocks.push(Block::Paragraph(std::mem::take(para)));
                }
            }
            SyntaxKind::Heading => {
                if !para.is_empty() {
                    blocks.push(Block::Paragraph(std::mem::take(para)));
                }
                let (level, content) = parse_heading(child);
                blocks.push(Block::Heading { level, content });
            }
            SyntaxKind::ListItem | SyntaxKind::EnumItem => {
                if !para.is_empty() {
                    blocks.push(Block::Paragraph(std::mem::take(para)));
                }
                let ordered = child.kind() == SyntaxKind::EnumItem;
                let mut inner = Vec::new();
                let mut item_para = Vec::new();
                flush_markup(child, blocks, &mut item_para);
                if !item_para.is_empty() {
                    inner.push(Block::Paragraph(item_para));
                }
                // Merge consecutive list items into one List block.
                push_list_item(blocks, ordered, inner);
            }
            SyntaxKind::Raw => {
                // Could be inline ``code`` or a fenced block. Treat multi-line as block.
                let text = node_text(child);
                let (lang, code, is_block) = parse_raw(&text);
                if is_block {
                    if !para.is_empty() {
                        blocks.push(Block::Paragraph(std::mem::take(para)));
                    }
                    blocks.push(Block::CodeBlock { lang, code });
                } else {
                    para.push(Inline::Styled {
                        marks: MarkSet::CODE,
                        link: None,
                        content: vec![Inline::Text(code)],
                    });
                }
            }
            _ => {
                if let Some(inl) = parse_inline(child) {
                    para.push(inl);
                }
            }
        }
    }
}

fn parse_inline(child: &SyntaxNode) -> Option<Inline> {
    match child.kind() {
        SyntaxKind::Text => Some(Inline::Text(child.text().to_string())),
        SyntaxKind::Space => Some(Inline::Text(" ".into())),
        SyntaxKind::Linebreak => Some(Inline::LineBreak),
        SyntaxKind::Strong => Some(wrap_marks(child, MarkSet::BOLD)),
        SyntaxKind::Emph => Some(wrap_marks(child, MarkSet::ITALIC)),
        SyntaxKind::Equation => {
            let text = node_text(child);
            let inner = text.trim_matches('$').trim().to_string();
            Some(Inline::Math {
                display: text.contains("$ ") || text.contains("\n"),
                src: inner,
            })
        }
        // Code-mode constructs (`#...`) and anything else: preserve verbatim.
        SyntaxKind::Hash | SyntaxKind::FuncCall | SyntaxKind::LetBinding => Some(Inline::Raw {
            format: Format::Typst,
            src: node_text(child),
        }),
        _ => {
            let t = node_text(child);
            if t.is_empty() {
                None
            } else {
                Some(Inline::Text(t))
            }
        }
    }
}

fn wrap_marks(node: &SyntaxNode, marks: MarkSet) -> Inline {
    let mut content = Vec::new();
    for child in node.children() {
        // skip the `*` / `_` delimiter leaves
        if matches!(child.kind(), SyntaxKind::Star | SyntaxKind::Underscore) {
            continue;
        }
        if let Some(inl) = parse_inline(child) {
            content.push(inl);
        }
    }
    Inline::Styled {
        marks,
        link: None,
        content,
    }
}

fn parse_heading(node: &SyntaxNode) -> (u8, Vec<Inline>) {
    let mut level = 1u8;
    let mut content = Vec::new();
    for child in node.children() {
        match child.kind() {
            SyntaxKind::HeadingMarker => {
                level = node_text(child)
                    .chars()
                    .filter(|c| *c == '=')
                    .count()
                    .max(1) as u8;
            }
            _ => {
                if let Some(inl) = parse_inline(child) {
                    content.push(inl);
                }
            }
        }
    }
    (level, content)
}

fn push_list_item(blocks: &mut Vec<Block>, ordered: bool, item: Vec<Block>) {
    if let Some(Block::List(list)) = blocks.last_mut() {
        if list.ordered == ordered {
            list.items.push(item);
            return;
        }
    }
    blocks.push(Block::List(crate::model::List {
        ordered,
        tight: true,
        items: vec![item],
    }));
}

/// Reconstruct a node's source by concatenating its leaf text (the green tree is
/// lossless, so this recovers the exact bytes).
fn node_text(node: &SyntaxNode) -> String {
    if node.children().len() == 0 {
        return node.text().to_string();
    }
    let mut s = String::new();
    for c in node.children() {
        s.push_str(&node_text(c));
    }
    s
}

fn parse_raw(text: &str) -> (Option<String>, String, bool) {
    let is_block = text.starts_with("```");
    let trimmed = text.trim_matches('`');
    if is_block {
        let mut lines = trimmed.splitn(2, '\n');
        let first = lines.next().unwrap_or("").trim();
        let lang = if first.is_empty() {
            None
        } else {
            Some(first.to_string())
        };
        let code = lines.next().unwrap_or("").trim_end().to_string();
        (lang, code, true)
    } else {
        (None, trimmed.to_string(), false)
    }
}

// ---- serialize: model -> typst markup -------------------------------------

fn write_block(b: &Block, out: &mut String) {
    match b {
        Block::Paragraph(inl) => {
            write_inlines(inl, out);
            out.push('\n');
        }
        Block::Heading { level, content } => {
            out.push_str(&"=".repeat((*level).max(1) as usize));
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
            // Typst has no native blockquote markup; use #quote.
            out.push_str("#quote[\n");
            for b in blocks {
                write_block(b, out);
            }
            out.push_str("]\n");
        }
        Block::List(list) => {
            for (i, item) in list.items.iter().enumerate() {
                if list.ordered {
                    out.push_str("+ ");
                    let _ = i;
                } else {
                    out.push_str("- ");
                }
                let mut s = String::new();
                for b in item {
                    write_block(b, &mut s);
                }
                out.push_str(s.trim_end());
                out.push('\n');
            }
        }
        Block::ThematicBreak => out.push_str("#line(length: 100%)\n"),
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
        Inline::LineBreak => out.push_str(" \\\n"),
        Inline::Image { src, alt } => {
            out.push_str(&format!("#image(\"{src}\", alt: \"{alt}\")"));
        }
        Inline::Math { src, .. } => {
            out.push_str("$ ");
            out.push_str(src);
            out.push_str(" $");
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
            if marks.contains(MarkSet::LINK) {
                out.push_str(&format!("#link(\"{}\")[", link.as_deref().unwrap_or("")));
                write_inlines(content, out);
                out.push(']');
                return;
            }
            let bold = marks.contains(MarkSet::BOLD);
            let italic = marks.contains(MarkSet::ITALIC);
            if bold {
                out.push('*');
            }
            if italic {
                out.push('_');
            }
            write_inlines(content, out);
            if italic {
                out.push('_');
            }
            if bold {
                out.push('*');
            }
        }
    }
}
