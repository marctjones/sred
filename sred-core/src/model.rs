//! The format-agnostic document model.
//!
//! This is the *superset* AST that both backends serialize against. Marks are a
//! bitset carried on inline runs (so bold+italic is one run with two bits, which
//! maps 1:1 onto a `cosmic_text::Attrs`). Anything a backend can parse but the
//! model can't represent is preserved verbatim in a [`Inline::Raw`] / [`Block::Raw`]
//! escape hatch so round-trips stay lossless.

use bitflags::bitflags;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Typst,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Markdown => "markdown",
            Format::Typst => "typst",
        }
    }

    // Returns `Option` (not the `FromStr` trait) so unknown formats are a `None`,
    // not an error type — the call sites want the option.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "markdown" | "md" => Some(Format::Markdown),
            "typst" | "typ" => Some(Format::Typst),
            _ => None,
        }
    }
}

bitflags! {
    /// Inline character-level styling. One run carries the union of its marks.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct MarkSet: u16 {
        const BOLD   = 1 << 0;
        const ITALIC = 1 << 1;
        const CODE   = 1 << 2;
        const STRIKE = 1 << 3;
        const LINK   = 1 << 4;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocMeta {
    /// The format this document was opened from (drives default-save target).
    pub origin: Option<Format>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub meta: DocMeta,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    List(List),
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    BlockQuote(Vec<Block>),
    ThematicBreak,
    /// Format-specific block we can preserve but not structurally edit
    /// (e.g. a Typst `#figure(...)` call, or an HTML block in Markdown).
    Raw {
        format: Format,
        src: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    pub tight: bool,
    pub items: Vec<Vec<Block>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    /// A run of inline content carrying a set of marks. `link` holds the href
    /// when `marks` contains `LINK`.
    Styled {
        marks: MarkSet,
        link: Option<String>,
        content: Vec<Inline>,
    },
    LineBreak,
    Image {
        src: String,
        alt: String,
    },
    Math {
        display: bool,
        src: String,
    },
    Raw {
        format: Format,
        src: String,
    },
}

impl Document {
    pub fn empty() -> Self {
        Document::default()
    }

    /// Convenience for tests / quick construction.
    pub fn paragraph(text: &str) -> Self {
        Document {
            blocks: vec![Block::Paragraph(vec![Inline::Text(text.to_string())])],
            meta: DocMeta::default(),
        }
    }

    /// Flatten the document to plain text (used as a cheap layout/debug view and
    /// as the basis for the editing rope in later phases).
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for (i, b) in self.blocks.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            block_plain(b, &mut out);
        }
        out
    }
}

fn block_plain(b: &Block, out: &mut String) {
    match b {
        Block::Paragraph(inl) => inlines_plain(inl, out),
        Block::Heading { content, .. } => inlines_plain(content, out),
        Block::CodeBlock { code, .. } => out.push_str(code),
        Block::BlockQuote(blocks) => {
            for (i, b) in blocks.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                block_plain(b, out);
            }
        }
        Block::List(list) => {
            for item in &list.items {
                for b in item {
                    block_plain(b, out);
                    out.push('\n');
                }
            }
        }
        Block::ThematicBreak => out.push_str("---"),
        Block::Raw { src, .. } => out.push_str(src),
    }
}

fn inlines_plain(inl: &[Inline], out: &mut String) {
    for i in inl {
        match i {
            Inline::Text(t) => out.push_str(t),
            Inline::Styled { content, .. } => inlines_plain(content, out),
            Inline::LineBreak => out.push('\n'),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::Math { src, .. } => out.push_str(src),
            Inline::Raw { src, .. } => out.push_str(src),
        }
    }
}
