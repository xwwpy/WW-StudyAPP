//! Progress for a background file operation. Fractions apply to one phase.
use gpui_kit::*;
use studyapp_core::NoticeLayout;

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub label: String,
    completed: Option<(u64, u64)>,
}

impl TransferProgress {
    pub fn bytes(phase: &str, done: u64, total: u64) -> Self {
        Self {
            label: format!("{phase} · {done} / {total} 字节"),
            completed: (total != 0 && done <= total).then_some((done, total)),
        }
    }
    pub fn completed(&self) -> Option<(u64, u64)> {
        self.completed
    }
    pub fn panel(&self) -> Div {
        self.panel_with_layout(NoticeLayout::Primary)
    }
    pub fn panel_with_layout(&self, layout: NoticeLayout) -> Div {
        crate::settings_components::section("正在迁移")
            .child(crate::components::muted("迁移期间暂时不能编辑。"))
            .child(crate::notification_surfaces::hud(
                self.label.clone(),
                self.completed,
                layout,
            ))
    }
}
impl From<&str> for TransferProgress {
    fn from(label: &str) -> Self {
        Self {
            label: label.into(),
            completed: None,
        }
    }
}
