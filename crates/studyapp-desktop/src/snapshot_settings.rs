use crate::{app::StudyApp, components::*, controls::Button, settings_components as form};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    component::{
        Disableable,
        input::{InputEvent, InputState},
    },
    *,
};
use studyapp_core::{Command, SnapshotMode, SnapshotSettings};

impl StudyApp {
    pub(crate) fn fill_snapshot_fields(
        &mut self,
        settings: SnapshotSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (key, value) in [
            ("interval", settings.interval_seconds),
            ("stable", settings.stable_seconds),
        ] {
            if let Some(input) = self.preferences_ui.snapshot_fields.get(key) {
                input.update(cx, |input, cx| {
                    input.set_value(value.to_string(), window, cx)
                });
            } else {
                let input =
                    cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
                cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
                .detach();
                self.preferences_ui.snapshot_fields.insert(key, input);
            }
        }
    }
    pub(crate) fn snapshot_fields_dirty(&self, settings: SnapshotSettings, cx: &App) -> bool {
        [
            ("interval", settings.interval_seconds),
            ("stable", settings.stable_seconds),
        ]
        .into_iter()
        .any(|(key, value)| {
            self.preferences_ui
                .snapshot_fields
                .get(key)
                .is_some_and(|input| input.read(cx).value().trim() != value.to_string())
        })
    }
    pub(crate) fn snapshot_settings_value(
        &self,
        mut settings: SnapshotSettings,
        cx: &App,
    ) -> anyhow::Result<SnapshotSettings> {
        for (key, value, label) in [
            ("interval", &mut settings.interval_seconds, "扫描间隔"),
            ("stable", &mut settings.stable_seconds, "稳定等待时间"),
        ] {
            if let Some(input) = self.preferences_ui.snapshot_fields.get(key) {
                *value = input
                    .read(cx)
                    .value()
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("{label}请填写整数秒"))?;
            }
        }
        settings.validate()?;
        Ok(settings)
    }
    pub(crate) fn snapshot_settings(&self, cx: &Context<Self>) -> Div {
        let policy = self
            .preferences_ui
            .draft
            .as_ref()
            .map(|s| s.snapshots)
            .unwrap_or_default();
        let busy = self.preferences_ui.busy || self.preferences_ui.replacing || self.storage_busy;
        let dirty = self.settings_dirty(cx);
        let mut view = form::section("笔记快照").child(form::settings_row(
            "保存方式",
            form::choice(
                "snapshot-mode",
                policy.mode,
                SnapshotMode::ALL
                    .into_iter()
                    .map(|v| (v, v.label().into()))
                    .collect(),
                busy,
                cx.weak_entity(),
                |this, value, _, cx| {
                    if this.preferences_ui.busy
                        || this.preferences_ui.replacing
                        || this.storage_busy
                    {
                        return;
                    }
                    if let Some(draft) = &mut this.preferences_ui.draft {
                        draft.snapshots.mode = value;
                    }
                    cx.notify();
                },
            ),
        ));
        for (key, label, default, min, disabled) in [
            (
                "interval",
                "扫描间隔",
                60,
                3,
                policy.mode == SnapshotMode::Manual,
            ),
            (
                "stable",
                "内容稳定等待",
                3,
                1,
                policy.mode != SnapshotMode::Automatic,
            ),
        ] {
            if let Some(input) = self.preferences_ui.snapshot_fields.get(key) {
                view = view.child(form::settings_row(
                    label,
                    form::NumericSetting::new(
                        format!("snapshot-{key}"),
                        input,
                        "秒",
                        default,
                        min,
                        86400,
                        busy || disabled,
                    ),
                ));
            }
        }
        view.child(muted(match policy.mode {
            SnapshotMode::Automatic => "内容停止变化并达到稳定等待时间后才生成快照；继续修改会重新等待。扫描间隔与稳定等待分别设置。",
            SnapshotMode::ScanOnly => "定时检查文件状态，不自动新增快照；稳定等待仅用于自动保存模式。",
            SnapshotMode::Manual => "不在启动或后台扫描；扫描间隔和稳定等待暂不启用，当前草稿仍保留。",
        }))
        .child(row().flex_wrap()
            .child(Button::new("capture-snapshots").label("扫描并保存全部变化").disabled(busy || dirty)
                .on_click(cx.listener(|this, _, _, cx| this.send(Command::CaptureSnapshots, cx))))
            .child(Button::new("preview-snapshots").label("查看变化并勾选保存…").disabled(busy || dirty)
                .on_click(cx.listener(|this, _, _, cx| this.open_snapshot_preview(cx)))))
        .when(dirty, |view| view.child(muted("请先保存或放弃全部设置修改，再按已保存的策略执行快照操作。")))
        .child(muted("Ctrl+S 独立保存正文，不自动增加历史。导入基线与冲突保护仍保留；保存快照不改写原文件，也不覆盖或删除旧历史。"))
    }
}
