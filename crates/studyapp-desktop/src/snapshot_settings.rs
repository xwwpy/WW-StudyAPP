use crate::{app::StudyApp, components::*, controls::Button, settings_components as form};
use gpui_kit::{component::Disableable, *};
use studyapp_core::{Command, SnapshotMode};

impl StudyApp {
    pub(crate) fn snapshot_settings(&self, cx: &Context<Self>) -> Div {
        let policy = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|s| s.snapshots)
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        form::section("笔记快照")
            .child(form::settings_row("保存方式", form::choice(
                "snapshot-mode", policy.mode,
                SnapshotMode::ALL.into_iter().map(|v| (v, v.label().into())).collect(),
                busy, cx.weak_entity(), |this, value, _, cx| {
                    if this.preferences_ui.busy || this.preferences_ui.replacing || this.storage_busy { return; }
                    if let Some(draft) = &mut this.preferences_ui.draft { draft.snapshots.mode = value; }
                    cx.notify();
                },
            )))
            .child(form::settings_row("扫描间隔", form::choice(
                "snapshot-interval", policy.interval_seconds,
                [3, 15, 30, 60, 300, 900, 1800, 3600, 86400].into_iter()
                    .chain(std::iter::once(policy.interval_seconds))
                    .collect::<std::collections::BTreeSet<_>>().into_iter()
                    .map(|v| (v, format!("{v} 秒"))).collect(),
                busy || policy.mode == SnapshotMode::Manual, cx.weak_entity(), |this, value, _, cx| {
                    if this.preferences_ui.busy || this.preferences_ui.replacing || this.storage_busy { return; }
                    if let Some(draft) = &mut this.preferences_ui.draft { draft.snapshots.interval_seconds = value; }
                    cx.notify();
                },
            )))
            .child(muted(match policy.mode {
                SnapshotMode::Automatic => "定时检查文件内容，仅为变化后的稳定内容保存快照。",
                SnapshotMode::ScanOnly => "定时检查文件状态，不自动新增快照；点击“扫描并保存快照”才记录当前内容。",
                SnapshotMode::Manual => "不在启动或后台扫描；点击“扫描并保存快照”才检查文件并记录变化。",
            }))
            .child(Button::new("capture-snapshots").label("扫描并保存快照").disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.send(Command::CaptureSnapshots, cx))))
            .child(muted("旧快照和学习记录保持不变。相同内容不会重复保存；编辑器的主动保存、导入基线与冲突保护不受此开关影响。保存全部设置后生效。"))
    }
}
