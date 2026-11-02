//! Progress for a background file operation. Fractions apply to one phase.
use gpui_kit::*;

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub label: String,
    fraction: Option<f32>,
}

impl TransferProgress {
    pub fn bytes(phase: &str, done: u64, total: u64) -> Self {
        Self {
            label: format!("{phase} · {done} / {total} 字节"),
            fraction: (total != 0).then(|| (done as f64 / total as f64).clamp(0., 1.) as f32),
        }
    }
    pub fn panel(&self) -> Div {
        let mut panel = crate::settings_components::section("正在迁移")
            .child(crate::components::muted("迁移期间暂时不能编辑。"))
            .child(crate::components::muted(self.label.clone()));
        if let Some(fraction) = self.fraction {
            panel = panel.child(
                div()
                    .h(px(5.))
                    .w_full()
                    .bg(crate::theme::color(crate::components::BG))
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .bg(crate::theme::color(crate::components::GREEN)),
                    ),
            );
        }
        panel
    }
}
impl From<&str> for TransferProgress {
    fn from(label: &str) -> Self {
        Self {
            label: label.into(),
            fraction: None,
        }
    }
}
