//! The desktop window observes the existing StudyApp; it owns no worker or clock.
use crate::{
    app::{EditorMode, Page, StudyApp},
    timer_widget_window::{WidgetContentFit, WidgetWindowState},
};
use gpui_kit::*;
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};
use studyapp_core::{Command, TimerKind, TimerWidgetSettings};
use studyapp_ui::timer_card::{TimerCardAction, TimerCardModel, TimerCardStyle, timer_widget_card};

#[derive(Clone)]
struct WidgetPresentation {
    model: TimerCardModel,
    settings: TimerWidgetSettings,
    bindings: std::collections::BTreeMap<String, String>,
    pin_retry: u64,
}
impl WidgetPresentation {
    fn from_app(app: &StudyApp, cx: &App) -> Self {
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
            pin_retry: app.timer_pin_retry,
            settings: app.preview_widget_settings(cx),
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
    pin: crate::timer_widget_pin::PinController,
    pin_poll: Option<Task<()>>,
    presentation: WidgetPresentation,
    content_fit: Rc<RefCell<WidgetContentFit>>,
}

fn forward(app: WeakEntity<StudyApp>, action: TimerCardAction, cx: &mut App) {
    #[cfg(feature = "ui-test")]
    eprintln!("native widget action forwarded: {action:?}");
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

fn resize_widget(window: &mut Window, desired: Size<Pixels>, cx: &mut App) -> anyhow::Result<()> {
    // Reconcile the native client size before another content-fit callback can
    // reuse its previous width. Also avoid activating an unfocused widget.
    let current = window.window_bounds().get_bounds();
    crate::card_window::fit_passive(window, Bounds::new(current.origin, desired), cx)
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
        let handle = window.window_handle();
        cx.observe(&app, move |this, app, cx| {
            this.presentation = WidgetPresentation::from_app(app.read(cx), cx);
            if this.state.rescale(this.presentation.settings.scale) {
                let desired = this.state.bounds().size;
                let owner = this.app.downgrade();
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, cx| {
                        let current = window.window_bounds().get_bounds();
                        let available = window
                            .display(cx)
                            .map(|display| {
                                let area = display.visible_bounds();
                                size(
                                    (area.right() - current.left()).max(px(140.)),
                                    (area.bottom() - current.top()).max(px(100.)),
                                )
                            })
                            .unwrap_or(desired);
                        if let Err(error) = resize_widget(
                            window,
                            size(
                                desired.width.min(available.width),
                                desired.height.min(available.height),
                            ),
                            cx,
                        ) {
                            let _ = owner.update(cx, |app, cx| {
                                app.push_notice(Err(format!("计时小组件缩放失败：{error:#}")), cx)
                            });
                        }
                    });
                });
            }
            this.poll_pin(handle, cx);
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
        let mut this = Self {
            app,
            state,
            path,
            save_task: None,
            pin: Default::default(),
            pin_poll: None,
            presentation,
            content_fit: Rc::default(),
        };
        this.poll_pin(handle, cx);
        this
    }

    fn poll_pin(&mut self, handle: AnyWindowHandle, cx: &mut Context<Self>) {
        if self.pin_poll.is_some()
            || !self.pin.needs_step(
                self.presentation.settings.pinned,
                self.presentation.pin_retry,
            )
        {
            return;
        }
        // Native acknowledgement must run even when the compositor does not
        // request frames (covered windows, inactive windows, or cached views).
        // Rendering only displays the last acknowledged state.
        #[cfg(feature = "ui-test")]
        eprintln!(
            "native pin poll scheduled: target={}, retry={}",
            self.presentation.settings.pinned, self.presentation.pin_retry
        );
        self.pin_poll = Some(cx.spawn(async move |this, cx| {
            #[cfg(feature = "ui-test")]
            eprintln!("native pin poll task started");
            loop {
                // A nested native event may temporarily hold the App lease.
                // Keep this task and its pending marker alive for that tick;
                // only a closed owner/window or a completed request stops it.
                let mut pending = true;
                let alive =
                    crate::owned_poll::update_in_app(&this, handle, cx, |this, window, cx| {
                        let settings = &this.presentation.settings;
                        #[cfg(feature = "ui-test")]
                        eprintln!(
                            "native pin poll tick: target={}, pending={}",
                            settings.pinned,
                            this.pin.pending()
                        );
                        let update = this.pin.step(
                            settings.pinned,
                            this.presentation.pin_retry,
                            std::time::Instant::now(),
                            |request| {
                                #[cfg(feature = "ui-test")]
                                eprintln!(
                                    "native pin platform begin: target={}, request={request}",
                                    settings.pinned
                                );
                                let result = crate::timer_widget_window::decorate(
                                    window,
                                    settings.pinned,
                                    request,
                                );
                                #[cfg(feature = "ui-test")]
                                eprintln!("native pin platform complete: {result:?}");
                                result
                            },
                        );
                        #[cfg(feature = "ui-test")]
                        match &update {
                            crate::timer_widget_pin::PinUpdate::Applied => {
                                eprintln!("native widget pin acknowledged: {}", settings.pinned)
                            }
                            crate::timer_widget_pin::PinUpdate::Failed(error) => {
                                eprintln!("native widget pin failed: {error}")
                            }
                            _ => {}
                        }
                        if let crate::timer_widget_pin::PinUpdate::Failed(error) = update {
                            let owner = this.app.downgrade();
                            cx.defer(move |cx| {
                                let _ = owner.update(cx, |app, cx| app.push_notice(Err(error), cx));
                            });
                        }
                        cx.notify();
                        pending = this.pin.pending();
                        if !pending {
                            this.pin_poll = None;
                        }
                    });
                #[cfg(feature = "ui-test")]
                eprintln!("native pin poll tick complete: alive={alive}, pending={pending}");
                if !alive || !pending {
                    break;
                }
                #[cfg(feature = "ui-test")]
                eprintln!("native pin poll timer begin");
                cx.background_executor()
                    .timer(crate::timer_widget_pin::POLL_INTERVAL)
                    .await;
                #[cfg(feature = "ui-test")]
                eprintln!("native pin poll timer complete");
            }
        }));
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
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // open_window renders synchronously while the StudyApp update lease is
        // still held. Render only the supplied/observed value snapshot; never
        // read the owner from this new window's render path.
        let model = self.presentation.model.clone();
        let settings = self.presentation.settings.clone();
        let bindings = self.presentation.bindings.clone();
        let mut style = TimerCardStyle::from_widget_settings(&settings, crate::theme::palette());
        style.pin_applied = Some(self.pin.applied());
        style.pin_pending = self.pin.pending();
        style.pin_failed = self.pin.failed();
        let app = self.app.downgrade();
        let card_owner = app.clone();
        let keys_owner = app.clone();
        let content_fit = self.content_fit.clone();
        div()
            .id("timer-widget")
            .test_support()
            .size_full()
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
            .child(timer_widget_card(
                model,
                style,
                move |action, _, cx| forward(card_owner.clone(), action, cx),
                move |natural, window, cx| {
                    let current = window.viewport_size();
                    let available = window
                        .display(cx)
                        .map(|display| {
                            let area = display.visible_bounds();
                            (area.bottom() - window.window_bounds().get_bounds().top())
                                .max(px(100.))
                                .min(area.size.height)
                        })
                        .unwrap_or(px(1000.));
                    if let Some(height) =
                        content_fit
                            .borrow_mut()
                            .height(current.width, natural, available)
                        && (height - current.height).abs() >= px(1.)
                    {
                        // Never resize in layout or reenter the owner lease.
                        let owner = app.clone();
                        window.defer(cx, move |window, cx| {
                            if window.viewport_size().width == current.width
                                && let Err(error) =
                                    resize_widget(window, size(current.width, height), cx)
                            {
                                let _ = owner.update(cx, |app, cx| {
                                    app.push_notice(
                                        Err(format!("计时小组件尺寸调整失败：{error:#}")),
                                        cx,
                                    )
                                });
                            }
                        });
                    }
                },
            ))
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
    pub fn preview_widget_settings(&self, cx: &App) -> TimerWidgetSettings {
        let mut settings = self.effective_widget_settings();
        if self.page == Page::Settings
            && let Some(color) = self.preferences_ui.appearance_colors.get("widget-accent")
        {
            let value = color.read(cx).preview_value(cx);
            if value.trim().is_empty() || studyapp_core::parse_rgba_color(value.trim()).is_some() {
                settings.accent = (!value.trim().is_empty()).then(|| value.trim().to_owned());
            }
        }
        settings
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
        state.rescale(self.preview_widget_settings(cx).scale);
        state.clamp(
            &cx.displays()
                .iter()
                .map(|display| display.visible_bounds())
                .collect::<Vec<_>>(),
        );
        let options = WindowOptions {
            kind: WindowKind::Normal,
            titlebar: None,
            app_owns_titlebar_drag: true,
            window_background: WindowBackgroundAppearance::Transparent,
            window_bounds: Some(WindowBounds::Windowed(state.bounds())),
            window_min_size: Some(size(px(140.), px(100.))),
            window_decorations: Some(WindowDecorations::Client),
            is_resizable: true,
            is_minimizable: false,
            focus: false,
            ..Default::default()
        };
        let app = cx.entity();
        let initial = state.clone();
        let presentation = WidgetPresentation::from_app(self, cx);
        // Component roots supply an opaque theme surface by default. Override
        // only this window's root; a transparent child cannot erase its parent.
        match cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| TimerWidget::new(app, initial, path, presentation, window, cx));
            cx.new(|cx| base::Root::new(view, window, cx).bg(rgba(0)))
        }) {
            Ok(handle) => {
                self.timer_widget = Some(handle.into());
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
        #[cfg(feature = "ui-test")]
        eprintln!(
            "native widget action received: {action:?}, page={:?}, modal={}, storage={}, preferences={}",
            self.page,
            self.has_modal(cx),
            self.storage_busy,
            self.preferences_ui.busy
        );
        match action {
            TimerCardAction::OpenMain => window.activate_window(),
            TimerCardAction::RetryPinned => {
                if self.has_modal(cx) || self.storage_busy || self.preferences_ui.busy {
                    return;
                }
                self.timer_pin_retry = self.timer_pin_retry.wrapping_add(1);
                cx.notify();
            }
            TimerCardAction::TogglePinned => {
                if self.has_modal(cx) || self.storage_busy || self.preferences_ui.busy {
                    return;
                }
                if self.page == Page::Settings {
                    if let Some(draft) = &mut self.preferences_ui.draft {
                        draft.timer_widget.pinned = !draft.timer_widget.pinned;
                        cx.notify();
                    }
                } else if let Some(snapshot) = &self.snapshot {
                    self.send(
                        Command::SetTimerWidgetPinned(!snapshot.settings.timer_widget.pinned),
                        cx,
                    );
                }
            }
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
                    self.select_settings_target("appearance.widget", cx);
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
                    TimerCardAction::Countdown(minutes) => {
                        match studyapp_core::countdown_seconds(minutes) {
                            Ok(seconds) => self.request_timer_change(
                                Command::StartTimer {
                                    kind: TimerKind::CountDown,
                                    target_seconds: Some(seconds),
                                    label: timer.label,
                                },
                                cx,
                            ),
                            Err(error) => self.push_notice(Err(error.to_string()), cx),
                        }
                    }
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
                            let started = minutes
                                .checked_mul(60)
                                .and_then(|seconds| i64::try_from(seconds).ok())
                                .and_then(|seconds| ended.checked_sub(seconds))
                                .and_then(|started| chrono::DateTime::from_timestamp(started, 0))
                                .map(|t| {
                                    t.with_timezone(&chrono::Local)
                                        .format("%Y-%m-%d %H:%M")
                                        .to_string()
                                });
                            if started.is_none() {
                                self.push_notice(
                                    Err("计时起点超出日历可表示范围，请手动填写学习起点".into()),
                                    cx,
                                );
                            }
                            let started = started.unwrap_or_default();
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

#[cfg(all(test, feature = "ui-test"))]
mod resize_tests {
    use super::resize_widget;
    use gpui_kit::{Context, IntoElement, Render, TestAppContext, Window, div, px, size};

    struct Blank;
    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui_kit::test]
    fn widget_zoom_updates_viewport_before_stale_content_fit_can_restore_old_width(
        cx: &mut TestAppContext,
    ) {
        let (_, visual) = cx.add_window_view(|_, _| Blank);
        visual.update(|window, cx| {
            for scale in [50., 73., 173., 200., 100.] {
                let before = window.viewport_size();
                let desired = size(px(400. * scale / 100.), px(300. * scale / 100.));
                resize_widget(window, desired, cx).unwrap();
                assert_eq!(
                    window.viewport_size(),
                    desired,
                    "native resize must reconcile the viewport synchronously"
                );
                // A height-only callback from the previous frame must not win
                // over a newer overall zoom or user width change.
                if window.viewport_size().width == before.width {
                    resize_widget(window, size(before.width, px(120.)), cx).unwrap();
                }
                assert_eq!(window.viewport_size().width, desired.width);
                let fitted = size(desired.width, desired.height + px(10.));
                resize_widget(window, fitted, cx).unwrap();
                assert_eq!(window.viewport_size(), fitted);
            }
        });
    }
}
