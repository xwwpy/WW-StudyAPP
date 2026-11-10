use serde::{Deserialize, Serialize};
/// Controls metadata hints without hiding ordinary control help or error details.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HoverContent {
    pub source: bool,
    pub status: bool,
    pub learned: bool,
    pub reviewed: bool,
    pub next_review: bool,
}
impl Default for HoverContent {
    fn default() -> Self {
        Self {
            source: true,
            status: true,
            learned: true,
            reviewed: true,
            next_review: true,
        }
    }
}
impl HoverContent {
    pub fn brief() -> Self {
        Self {
            source: false,
            status: false,
            learned: false,
            reviewed: false,
            next_review: true,
        }
    }
    pub fn filter(self, text: &str) -> String {
        text.lines()
            .filter(|line| {
                if matches!(*line, "已取消管理" | "应用管理副本" | "外部跟踪 · 只读")
                {
                    self.source
                } else if matches!(*line, "A · 尚未记录" | "M · 修改未记录" | "已记录")
                {
                    self.status
                } else if line.starts_with("学习时间：") || line.starts_with("最近学习：")
                {
                    self.learned
                } else if line.starts_with("上次复习：") {
                    self.reviewed
                } else if line.starts_with("下次复习：") {
                    self.next_review
                } else {
                    true
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
#[cfg(test)]
mod tests {
    use super::HoverContent;
    #[test]
    fn brief_hints_hide_only_optional_metadata_never_errors_or_control_help() {
        let text = "托管笔记\n应用管理副本\nM · 修改未记录\n学习时间：昨天\n上次复习：昨天\n下次复习：明天\n文件无法读取";
        assert_eq!(
            HoverContent::brief().filter(text),
            "托管笔记\n下次复习：明天\n文件无法读取"
        );
        assert_eq!(HoverContent::default().filter(text), text);
        let old: HoverContent = serde_json::from_str(r#"{"learned":false}"#).unwrap();
        assert!(old.source && old.status && old.next_review);
        let source_only = HoverContent {
            source: true,
            ..HoverContent::brief()
        };
        assert!(source_only.filter(text).contains("应用管理副本"));
        assert!(!source_only.filter(text).contains("M · 修改未记录"));
    }
}
