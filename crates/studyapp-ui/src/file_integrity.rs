//! Shared, read-only inspection details; the caller owns the dialog and scrolling.
use crate::components::{column, muted, row};
use gpui_kit::*;
#[derive(Clone, Debug)]
pub struct FileFinding {
    pub path: SharedString,
    pub state: SharedString,
}
pub fn details(summary: SharedString, findings: &[FileFinding]) -> Div {
    div().min_w_0().child(column().id("integrity-result").test_support().min_w_0().gap_3()
        .child(div().child(summary))
        .children(findings.iter().enumerate().map(|(index, finding)| {
            row().id(SharedString::from(format!("integrity-finding-{index}"))).test_support()
                .min_w_0().items_start()
                .child(div().flex_1().min_w_0().child(finding.path.clone()))
                .child(muted(finding.state.clone()))
        }))
        .child(muted("仅检查当前版本列出的发行文件。不下载程序文件、不修复或删除文件，也不补安装记录；笔记、资料库及未知邻接文件不在检查范围内。")))
}
