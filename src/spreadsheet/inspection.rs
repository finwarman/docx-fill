//! Cell-addressed spreadsheet tag inspection.

use super::WorkbookTemplate;

/// The supported template tag categories in worksheet cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadsheetTagKind {
    Value,
    Loop,
    InvertedLoop,
    HorizontalLoop,
    Image,
    Chart,
    RichText,
}

/// A worksheet tag and its nested children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadsheetTag {
    pub name: String,
    pub written: String,
    pub kind: SpreadsheetTagKind,
    /// The cell that holds the opening or value tag, such as `B3`.
    pub cell: String,
    pub children: Vec<SpreadsheetTag>,
}

/// Tags found on one worksheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadsheetSheetTags {
    pub name: String,
    pub part: String,
    pub tags: Vec<SpreadsheetTag>,
}

impl WorkbookTemplate {
    /// Lists each parsed worksheet's tags in document order. Loop tags contain their child tags.
    pub fn tags(&self) -> Vec<SpreadsheetSheetTags> {
        self.sheets
            .iter()
            .map(|(info, sheet)| SpreadsheetSheetTags {
                name: info.name.clone(),
                part: info.part.clone(),
                tags: sheet
                    .inspect()
                    .into_iter()
                    .fold(vec![Vec::<SpreadsheetTag>::new()], |mut levels, item| {
                        match item.kind {
                            super::text::Kind::Value => {
                                levels.last_mut().expect("root level").push(SpreadsheetTag {
                                    name: item.name,
                                    written: item.written,
                                    kind: SpreadsheetTagKind::Value,
                                    cell: item.cell,
                                    children: Vec::new(),
                                })
                            }
                            super::text::Kind::Open { inverted, .. } => {
                                let kind = if item.written.starts_with('>') {
                                    SpreadsheetTagKind::HorizontalLoop
                                } else if inverted {
                                    SpreadsheetTagKind::InvertedLoop
                                } else {
                                    SpreadsheetTagKind::Loop
                                };
                                levels.last_mut().expect("root level").push(SpreadsheetTag {
                                    name: item.name,
                                    written: item.written,
                                    kind,
                                    cell: item.cell,
                                    children: Vec::new(),
                                });
                                levels.push(Vec::new());
                            }
                            super::text::Kind::Close { .. } => {
                                if levels.len() > 1 {
                                    let children = levels.pop().expect("nested level");
                                    if let Some(open) =
                                        levels.last_mut().and_then(|tags| tags.last_mut())
                                    {
                                        open.children = children;
                                    }
                                }
                            }
                            super::text::Kind::Image => {
                                levels.last_mut().expect("root level").push(SpreadsheetTag {
                                    name: item.name,
                                    written: item.written,
                                    kind: SpreadsheetTagKind::Image,
                                    cell: item.cell,
                                    children: Vec::new(),
                                })
                            }
                            super::text::Kind::Chart => {
                                levels.last_mut().expect("root level").push(SpreadsheetTag {
                                    name: item.name,
                                    written: item.written,
                                    kind: SpreadsheetTagKind::Chart,
                                    cell: item.cell,
                                    children: Vec::new(),
                                })
                            }
                            super::text::Kind::RichText { .. } => {
                                levels.last_mut().expect("root level").push(SpreadsheetTag {
                                    name: item.name,
                                    written: item.written,
                                    kind: SpreadsheetTagKind::RichText,
                                    cell: item.cell,
                                    children: Vec::new(),
                                })
                            }
                            super::text::Kind::Change => {}
                        }
                        levels
                    })
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
            })
            .collect()
    }
}
