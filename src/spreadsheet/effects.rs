//! Cell effects that update parts beside a worksheet cell.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectKind {
    Style,
    Hyperlink,
    Comment,
    Image,
    Chart,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CellEffect {
    pub kind: EffectKind,
    /// The cell reference after row and column template expansion.
    pub cell: String,
    pub value: Value,
    /// Owning worksheet part, supplied by the package renderer.
    pub part: String,
}
