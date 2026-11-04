//! The desktop window observes the existing StudyApp; it owns no worker or clock.
use crate::{
    app::{EditorMode, Page, StudyApp},
    components::*,
    controls::Button,
    timer_widget_window::WidgetWindowState,
};
use gpui_kit::{
    component::{Sizable, button::ButtonVariants},
    *,
};
use std::{path::PathBuf, time::Duration};
use studyapp_core::{Command, TimerKind, TimerWidgetSettings};
use studyapp_ui::timer_card::{
    TimerCardAction, TimerCardModel, TimerCardStyle, TimerCardVariant, timer_card,
};

#[derive(Clone)]
struct WidgetPresentation {
    model: TimerCardModel,
    settings: TimerWidgetSettings,
    bindings: std::collections::BTreeMap<String, String>,
}
impl WidgetPresentation {
    fn from_app(app: &StudyApp) -> Self {
        let fallback = studyapp_core::TimerState::default();
        let timer = app
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.timer)
            .unwrap_or(&fallback);
        let segment = app
            .read_models
            .record_positions(&chrono::Local::now().format("%Y-%m-%d").to_string(), 0)
            .len()
            + 1;
        Self {
            model: TimerCardModel::from_timer(timer, segment),
            settings: app.effective_widget_settings(),
            bindings: app
                .preferences_ui
                .saved
                .as_ref()
                .map(|p| p.shortcuts.clone())
                .unwrap_or_else(crate::shortcuts::default_bindings),
        }
    }
}

pub struct TimerWidget {
    app: Entity<StudyApp>,
    state: WidgetWindowState,
    path: PathBuf,
    save_task: Option<Task<()>>,
    pinned: Option<bool>,
    presentation: WidgetPresentation,
}

fn forward(app: WeakEntity<StudyApp>, action: TimerCardAction, cx: &mut App) {
    let Some(owner) = app.upgrade() else {
        return;
    };
    let main = owner.read(cx).main_window;
    // The widget's window and view are currently borrowed. Enter the main view
    // only on the next turn, keeping every command on its existing UI boundary.
    cx.defer(move |cx| {
        let _ = main.update(cx, |_, window, cx| {
            let _ = app.update(cx, |app, cx| app.timer_action(action, window, cx));
        });
    });
}

impl TimerWidget {
    fn new(
        app: Entity<StudyApp>,
        state: WidgetWindowState,
        path: PathBuf,
        presentation: WidgetPresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&app, |this, app, cx| {
            this.presentation = WidgetPresentation::from_app(app.read(cx));
            cx.notify();
        })
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| {
            this.state.capture(window.window_bounds().get_bounds());
            this.state.open = true;
            this.save_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let _ = this.update(cx, |this, cx| this.persist(cx));
            }));
        })
        .detach();
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.state.capture(window.window_bounds().get_bounds());
                this.state.open = false;
                this.persist(cx);
                this.app.update(cx, |app, cx| {
                    app.timer_widget = None;
                    cx.notify();
                });
            });
            true
        });
        Self {
            app,
            state,
            path,
            save_task: None,
            pinned: None,
            presentation,
        }
    }
    fn persist(&self, cx: &mut Context<Self>) {
        let result = self.state.save(&self.path);
        self.app.update(cx, |app, cx| {
            app.widget_state = self.state.clone();
            if let Err(error) = result {
                app.push_notice(Err(format!("计时窗口位置未保存：{error:#}")), cx);
            }
        });
    }
}
impl Render for TimerWidget {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // open_window renders synchronously while the StudyApp update lease is
        // still held. Render only the supplied/observed value snapshot; never
        // read the owner from this new window's render path.
        let model = self.presentation.model.clone();
        let settings = self.presentation.settings.clone();
        let bindings = self.presentation.bindings.clone();
        if self.pinned != Some(settings.pinned)
            && crate::timer_widget_window::decorate(window, settings.pinned).is_ok()
        {
            self.pinned = Some(settings.pinned);
        }
        let app = self.app.downgrade();
        let button = |id: &str, label: &str, action: TimerCardAction| {
            let app = app.clone();
            Button::new(SharedString::from(id.to_owned()))
                .small()
                .ghost()
                .label(label.to_owned())
                .on_click(move |_, _, cx| forward(app.clone(), action, cx))
        };
        let card_owner = app.clone();
        let keys_owner = app.clone();
        let mut drag = div()
            .id("timer-widget-drag")
            .test_support()
            .flex_1()
            .h(px(28.))
            .min_w_0()
            .window_control_area(WindowControlArea::Drag)
            .cursor_move()
            .child(muted("⋮⋮ 计时"));
        if !cfg!(windows) {
            drag = drag.on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move());
        }
        column()
            .id("timer-widget")
            .test_support()
            .size_full()
            .gap_1()
            .p_2()
            .bg(rgba(0))
            .font_family(crate::design::UI_FONT)
            .text_color(rgb(crate::theme::palette().text))
            .on_key_down(move |event, _, cx| {
                let Some(owner) = keys_owner.upgrade() else {
                    return;
                };
                let _ = owner;
                let key = &event.keystroke;
                let combination = crate::shortcuts::key_combination(
                    &key.key,
                    key.modifiers.control,
                    key.modifiers.alt,
                    key.modifiers.shift,
                );
                if combination.is_ok_and(|key| {
                    bindings.get("app.timer-widget").is_some_and(|b| {
                        b.split(';').any(|alias| {
                            crate::shortcuts::normalize_binding(alias)
                                .is_ok_and(|alias| alias == key)
                        })
                    })
                }) {
                    forward(keys_owner.clone(), TimerCardAction::ShowOnDesktop, cx);
                    cx.stop_propagation();
                }
            })
            .child(
                row()
                    .gap_1()
                    .flex_shrink_0()
                    .child(drag)
                    .child(button(
                        "widget-open-main",
                        "主界面",
                        TimerCardAction::OpenMain,
                    ))
                    .child(button(
                        "widget-settings",
                        "外观",
                        TimerCardAction::OpenSettings,
                    ))
                    .child(button("widget-close", "关闭", TimerCardAction::CloseWidget)),
            )
            .child(
                div()
                    .id("timer-widget-scroll")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(timer_card(
                        model,
                        TimerCardStyle::from_widget_settings(&settings, crate::theme::palette()),
                        TimerCardVariant::Widget,
                        move |action, _, cx| forward(card_owner.clone(), action, cx),
                    )),
            )
            .child(row().flex_shrink_0().justify_end().child(button(
                "widget-exit",
                "退出应用",
                TimerCardAction::ExitApp,
            )))
    }
}

impl StudyApp {
    pub fn effective_widget_settings(&self) -> TimerWidgetSettings {
        if self.page == Page::Settings
            && let Some(draft) = &self.preferences_ui.draft
        {
            return (*draft.timer_widget).clone();
        }
        self.snapshot
            .as_ref()
            .map(|s| (*s.settings.timer_widget).clone())
            .unwrap_or_default()
    }
    pub fn widget_state_path() -> Option<PathBuf> {
        studyapp_desktop::app_paths::AppPaths::runtime()
            .ok()
            .map(|paths| paths.effective.config.join("timer-widget-window.json"))
    }
    pub fn restore_widget(&mut self, cx: &mut Context<Self>) {
        if self.widget_restore_pending && self.snapshot.is_some() {
            self.widget_restore_pending = false;
            if self.widget_state.open {
                self.open_widget(cx);
            }
        }
    }
    pub fn open_widget(&mut self, cx: &mut Context<Self>) {
        if self.timer_widget.is_some() {
            return;
        }
        let Some(path) = Self::widget_state_path() else {
            self.push_notice(Err("无法定位计时窗口配置目录".into()), cx);
            return;
        };
        let mut state = self.widget_state.clone();
        state.open = true;
        state.clamp(
            &cx.displays()
                .iter()
                .map(|display| display.visible_bounds())
                .collect::<Vec<_>>(),
        );
        let options = WindowOptions {
            kind: WindowKind::Normal,
            titlebar: Some(TitlebarOptions {
                title: Some("WW-StudyApp 计时".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            app_owns_titlebar_drag: true,
            window_background: WindowBackgroundAppearance::Transparent,
            window_bounds: Some(WindowBounds::Windowed(state.bounds())),
            window_min_size: Some(size(px(280.), px(200.))),
            window_decorations: Some(WindowDecorations::Client),
            is_resizable: true,
            is_minimizable: false,
            focus: false,
            ..Default::default()
        };
        let app = cx.entity();
        let initial = state.clone();
        let presentation = WidgetPresentation::from_app(self);
        match open_window(options, cx, move |window, cx| {
            cx.new(|cx| TimerWidget::new(app, initial, path, presentation, window, cx))
        }) {
            Ok((handle, _)) => {
                self.timer_widget = Some(handle);
                self.widget_state = state;
                if let Some(path) = Self::widget_state_path()
                    && let Err(error) = self.widget_state.save(&path)
                {
                    self.push_notice(Err(error.to_string()), cx);
                }
            }
            Err(error) => self.push_notice(Err(format!("无法打开计时小组件：{error:#}")), cx),
        }
        cx.notify();
    }
    pub fn close_widget(&mut self, preserve_open: bool, cx: &mut Context<Self>) {
        if let Some(handle) = self.timer_widget.take() {
            let bounds = handle.update(cx, |_, window, _| {
                let bounds = window.window_bounds().get_bounds();
                window.remove_window();
                bounds
            });
            if let Ok(bounds) = bounds {
                self.widget_state.capture(bounds);
            }
        }
        self.widget_state.open = preserve_open;
        if let Some(path) = Self::widget_state_path()
            && let Err(error) = self.widget_state.save(&path)
        {
            self.push_notice(Err(error.to_string()), cx);
        }
        cx.notify();
    }
    pub fn request_timer_change(&mut self, command: Command, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.timer.active_seconds >= 60)
        {
            self.timer_pending = Some(command);
            cx.notify();
        } else {
            self.send(command, cx);
        }
    }
    pub fn timer_action(
        &mut self,
        action: TimerCardAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TimerCardAction::OpenMain => window.activate_window(),
            TimerCardAction::CloseWidget => self.close_widget(false, cx),
            TimerCardAction::ShowOnDesktop => {
                if self.timer_widget.is_some() {
                    self.close_widget(false, cx);
                } else {
                    self.open_widget(cx);
                }
            }
            TimerCardAction::ExitApp => {
                self.timer_exit_requested = true;
                window.activate_window();
                self.handle_close(window, cx);
            }
            TimerCardAction::OpenSettings => {
                window.activate_window();
                self.navigate(Page::Settings, window, cx);
                if self.page == Page::Settings {
                    self.select_settings_section(
                        crate::settings_ui::SettingsSection::Appearance,
                        cx,
                    );
                    self.select_appearance_page(crate::settings_ui::AppearancePage::Widget, cx);
                }
            }
            other => {
                if self.has_modal(cx) || self.storage_busy || self.preferences_ui.busy {
                    return;
                }
                let Some(timer) = self.snapshot.as_ref().map(|s| s.timer.clone()) else {
                    return;
                };
                match other {
                    TimerCardAction::Toggle => self.send(Command::SetPaused(!timer.paused), cx),
                    TimerCardAction::Finish => self.send(Command::FinishTimer, cx),
                    TimerCardAction::Reset => self.request_timer_change(Command::ResetTimer, cx),
                    TimerCardAction::ChooseKind(kind) => {
                        if kind != timer.kind {
                            self.request_timer_change(
                                Command::StartTimer {
                                    kind,
                                    target_seconds: if kind == TimerKind::CountDown {
                                        Some(1500)
                                    } else {
                                        None
                                    },
                                    label: String::new(),
                                },
                                cx,
                            );
                        }
                    }
                    TimerCardAction::Countdown(minutes) => self.request_timer_change(
                        Command::StartTimer {
                            kind: TimerKind::CountDown,
                            target_seconds: Some(u64::from(minutes) * 60),
                            label: timer.label,
                        },
                        cx,
                    ),
                    TimerCardAction::Configure => {
                        self.timer_name
                            .update(cx, |input, cx| input.set_value(timer.label, window, cx));
                        self.timer_duration.update(cx, |input, cx| {
                            input.set_value(
                                if timer.kind == TimerKind::CountDown {
                                    (timer.target_seconds / 60).to_string()
                                } else {
                                    "25".into()
                                },
                                window,
                                cx,
                            )
                        });
                        self.timer_setup = Some(timer.kind);
                        window.activate_window();
                        cx.notify();
                    }
                    TimerCardAction::Record => {
                        window.activate_window();
                        self.open_editor(EditorMode::Manual, window, cx);
                        if matches!(self.editor, Some(EditorMode::Manual))
                            && timer.kind != TimerKind::Study
                            && timer.active_seconds > 0
                        {
                            let ended = timer
                                .finished_at
                                .unwrap_or_else(|| chrono::Utc::now().timestamp());
                            let minutes = timer.active_seconds.div_ceil(60).max(1);
                            let started =
                                chrono::DateTime::from_timestamp(ended - minutes as i64 * 60, 0)
                                    .map(|t| {
                                        t.with_timezone(&chrono::Local)
                                            .format("%Y-%m-%d %H:%M")
                                            .to_string()
                                    })
                                    .unwrap_or_default();
                            self.start
                                .update(cx, |input, cx| input.set_value(started, window, cx));
                            self.minutes.update(cx, |input, cx| {
                                input.set_value(minutes.to_string(), window, cx)
                            });
                            self.summary
                                .update(cx, |input, cx| input.set_value(timer.label, window, cx));
                        }
                    }
                    _ => {}
                }
            }
        }
        if self.timer_pending.is_some() || self.timer_setup.is_some() {
            window.activate_window();
        }
    }
}
