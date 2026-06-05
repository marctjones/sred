//! The pluggable-backend abstraction: every supported document format implements
//! [`DocumentFormat`]. The internal model is a superset, so each backend declares
//! which constructs it can represent via [`Caps`] and serializes lossily against
//! that — anything outside its capabilities is carried in `Raw` nodes.

use crate::model::{Document, Format};

bitflags::bitflags! {
    /// What a backend can faithfully represent. Used to decide when to fall back
    /// to a `Raw` node rather than silently dropping content.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Caps: u32 {
        const HEADINGS    = 1 << 0;
        const EMPHASIS    = 1 << 1;
        const CODE        = 1 << 2;
        const LISTS       = 1 << 3;
        const TABLES      = 1 << 4;
        const MATH        = 1 << 5;
        const IMAGES      = 1 << 6;
        const BLOCKQUOTE  = 1 << 7;
        const STRIKE      = 1 << 8;
    }
}

#[derive(Debug)]
pub enum FormatError {
    Parse(String),
    Serialize(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Parse(m) => write!(f, "parse error: {m}"),
            FormatError::Serialize(m) => write!(f, "serialize error: {m}"),
        }
    }
}

impl std::error::Error for FormatError {}

pub trait DocumentFormat {
    fn id(&self) -> Format;
    fn capabilities(&self) -> Caps;
    fn parse(&self, src: &str) -> Result<Document, FormatError>;
    fn serialize(&self, doc: &Document) -> Result<String, FormatError>;
}

/// Resolve a backend by format id.
pub fn backend_for(format: Format) -> Box<dyn DocumentFormat> {
    match format {
        Format::Markdown => Box::new(crate::markdown::Markdown),
        Format::Typst => Box::new(crate::typst_fmt::Typst),
    }
}
