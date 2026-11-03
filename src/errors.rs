use std::fmt;

/// Stable categories for template and option errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FillErrorId {
    TemplateError, MultiError, MalformedXml, InvalidDelimiter,
    UnclosedTag, UnopenedTag, DuplicateOpenTag, DuplicateCloseTag,
    UnclosedLoop, UnopenedLoop, ClosingTagMismatch, LoopPositionInvalid, UnbalancedLoopTags,
    RawTagNotAlone, ImageTagNotAlone, TableTagNotAlone, SegmentTagNotAlone,
    UndefinedSegment, ScopeParserCompilation, NoXmlTagFound,
    HtmlUnknownTag, HtmlCssInvalid, HtmlBlockInvalid, ChartWorkbook,
    SpreadsheetPackage, SpreadsheetCell, SpreadsheetReference,
}

impl FillErrorId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TemplateError => "template_error", Self::MultiError => "multi_error",
            Self::MalformedXml => "malformed_xml", Self::InvalidDelimiter => "change_delimiters_invalid",
            Self::UnclosedTag => "unclosed_tag", Self::UnopenedTag => "unopened_tag",
            Self::DuplicateOpenTag => "duplicate_open_tag", Self::DuplicateCloseTag => "duplicate_close_tag",
            Self::UnclosedLoop => "unclosed_loop", Self::UnopenedLoop => "unopened_loop",
            Self::ClosingTagMismatch => "closing_tag_does_not_match_opening_tag", Self::LoopPositionInvalid => "loop_position_invalid",
            Self::RawTagNotAlone => "raw_xml_tag_should_be_only_text_in_paragraph", Self::ImageTagNotAlone => "centered_image_should_be_in_paragraph",
            Self::TableTagNotAlone => "table_tag_should_be_only_text_in_paragraph", Self::SegmentTagNotAlone => "segment_tag_should_be_only_text_in_paragraph",
            Self::UnbalancedLoopTags => "unbalanced_loop_tags",
            Self::UndefinedSegment => "undefined_subsegment", Self::ScopeParserCompilation => "scopeparser_compilation_failed",
            Self::NoXmlTagFound => "no_xml_tag_found_at_left", Self::HtmlUnknownTag => "html_tag_name_unsupported",
            Self::HtmlCssInvalid => "style_property_invalid", Self::HtmlBlockInvalid => "html_tag_invalid_type",
            Self::ChartWorkbook => "chart_workbook_error",
            Self::SpreadsheetPackage => "spreadsheet_package_error",
            Self::SpreadsheetCell => "spreadsheet_cell_error",
            Self::SpreadsheetReference => "spreadsheet_reference_error",
        }
    }
}

/// A template error, or an aggregate whose `errors` hold the individual diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillError {
    pub part: Option<String>,
    pub message: String,
    pub id: FillErrorId,
    pub tag: Option<Box<str>>,
    /// Byte offset in the paragraph's joined XML text, before entity decoding.
    pub offset: Option<usize>,
    /// Zero-based paragraph index in the part.
    pub paragraph: Option<usize>,
    pub errors: Box<[FillError]>,
}

impl FillError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self { part: None, message: message.into(), id: FillErrorId::TemplateError, tag: None, offset: None, paragraph: None, errors: Box::new([]) }
    }
    pub(crate) fn with_id(mut self, id: FillErrorId) -> Self { self.id = id; self }
    pub(crate) fn at(mut self, tag: impl Into<String>, offset: usize, paragraph: usize) -> Self {
        self.tag = Some(tag.into().into_boxed_str()); self.offset = Some(offset); self.paragraph = Some(paragraph); self
    }
    pub(crate) fn in_part(mut self, part: &str) -> Self {
        self.part = Some(part.into());
        self.errors = self.errors.into_vec().into_iter().map(|error| error.in_part(part)).collect();
        self
    }
    pub(crate) fn many(errors: Vec<Self>) -> Self {
        let mut errors: Vec<_> = errors.into_iter().flat_map(|error| if error.errors.is_empty() { vec![error] } else { error.errors.into_vec() }).collect();
        if errors.len() == 1 { return errors.pop().expect("one error"); }
        let message = errors.first().map_or_else(|| "Template errors".to_string(), |error| error.message.clone());
        let part = errors.first().and_then(|error| error.part.clone());
        Self { errors: errors.into_boxed_slice(), part, ..Self::new(message).with_id(FillErrorId::MultiError) }
    }
    pub fn diagnostics(&self) -> Vec<&Self> {
        if self.errors.is_empty() { vec![self] } else { self.errors.iter().flat_map(Self::diagnostics).collect() }
    }
}

impl fmt::Display for FillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.part { Some(part) => write!(f, "{part}: {}", self.message), None => f.write_str(&self.message) }
    }
}

impl std::error::Error for FillError {}
