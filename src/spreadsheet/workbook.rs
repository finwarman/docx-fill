//! Workbook level sheet selection, naming and duplication plans.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{FillError, error, validate_sheet_name};

/// One new worksheet copied from a template worksheet.
#[derive(Debug, Clone)]
pub struct SheetInstance {
    /// The new worksheet name. It may contain value tags such as `{department}`.
    pub name: String,
    /// Data used to fill the copied worksheet. Object properties from the root render data remain
    /// visible as outer scope values.
    pub data: Value,
}

#[derive(Debug, Clone, Default)]
pub(super) struct WorkbookEdits {
    pub retained: Option<BTreeSet<String>>,
    pub copies: Vec<(String, Vec<SheetInstance>)>,
}

impl WorkbookEdits {
    pub fn retain(&mut self, names: impl IntoIterator<Item = String>) -> Result<(), FillError> {
        let mut retained = BTreeSet::new();
        for name in names {
            validate_sheet_name(&name)?;
            let key = name.to_lowercase();
            if !retained.insert(key) {
                return Err(error(format!("Duplicate worksheet selection: {name}")));
            }
        }
        if retained.is_empty() {
            return Err(error("A workbook must keep at least one worksheet"));
        }
        self.retained = Some(retained);
        Ok(())
    }

    pub fn repeat(&mut self, source: &str, instances: Vec<SheetInstance>) -> Result<(), FillError> {
        if self
            .copies
            .iter()
            .any(|(each, _)| each.eq_ignore_ascii_case(source))
        {
            return Err(error(format!(
                "Worksheet {source:?} already has a repeat plan"
            )));
        }
        let mut names = BTreeSet::new();
        for item in &instances {
            validate_sheet_name(&item.name)?;
            if !names.insert(item.name.to_lowercase()) {
                return Err(error(format!(
                    "Duplicate repeated worksheet name: {}",
                    item.name
                )));
            }
        }
        self.copies.push((source.to_string(), instances));
        Ok(())
    }

    pub fn selection_allows(&self, name: &str) -> bool {
        self.retained
            .as_ref()
            .is_none_or(|names| names.contains(&name.to_lowercase()))
    }
}
