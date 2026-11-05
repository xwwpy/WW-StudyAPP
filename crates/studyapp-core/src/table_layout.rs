//! Presentation-only table metadata. No Markdown or filesystem paths are stored.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TableSizing {
    #[default]
    Equal,
    Adaptive,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TableAlignment {
    #[default]
    Source,
    Left,
    Center,
    Right,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct TableDefaults {
    pub sizing: TableSizing,
    pub alignment: TableAlignment,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableAnchor {
    pub header: String,
    pub content: String,
    pub context: String,
    pub occurrence: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableView {
    pub anchor: TableAnchor,
    pub sizing: Option<TableSizing>,
    pub widths: Option<Vec<u16>>,
    pub alignments: Vec<Option<TableAlignment>>,
}
pub type DocumentTableViews = BTreeMap<String, TableView>;
pub type LibraryTableViews = BTreeMap<String, DocumentTableViews>;

pub fn valid_table_key(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl TableView {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [
                &self.anchor.header,
                &self.anchor.content,
                &self.anchor.context
            ]
            .iter()
            .all(|v| valid_table_key(v))
                && self.anchor.occurrence <= 1_000_000,
            "表格标识无效"
        );
        ensure!(self.alignments.len() <= 64, "表格最多配置 64 列");
        if let Some(widths) = &self.widths {
            ensure!(
                !widths.is_empty()
                    && widths.len() <= 64
                    && widths.iter().all(|v| *v > 0)
                    && widths.iter().map(|v| u32::from(*v)).sum::<u32>() == 10_000,
                "表格列宽无效"
            );
        }
        Ok(())
    }
}
pub(crate) fn validate_library(views: &LibraryTableViews) -> Result<()> {
    ensure!(
        views.len() <= 4096 && views.values().map(BTreeMap::len).sum::<usize>() <= 4096,
        "表格显示配置数量已达上限，请清除不再使用的配置"
    );
    for (document, tables) in views {
        ensure!(
            uuid::Uuid::parse_str(document).is_ok() && tables.len() <= 128,
            "笔记表格配置无效"
        );
        for (key, value) in tables {
            ensure!(valid_table_key(key), "表格标识无效");
            value.validate()?;
        }
    }
    Ok(())
}
