//! Persistent color control. Configuration text stays in the existing draft
//! input; the picker is a lazily allocated presentation of that same value.
use crate::{
    components::*,
    controls::Button,
    settings_color_value::{ColorChoice, ColorDraft},
    settings_components as form,
};
use gpui_kit::{
    component::{
        Disableable, Selectable, Sizable,
        button::ButtonVariants,
        color_picker::{ColorPickerEvent, ColorPickerState},
        popover::Popover,
        slider::Slider,
    },
    *,
};
use std::rc::Rc;

pub struct ColorSetting {
    key: &'static str,
    source: Entity<gpui_kit::component::input::InputState>,
    draft: ColorDraft,
    automatic: bool,
    inherited: u32,
    disabled: bool,
    open: bool,
    popup_bounds: Option<Bounds<Pixels>>,
    popup_geometry: Option<gpui_kit::component::popover::PopoverSurfaceGeometry>,
    popup_surface: Option<Rc<()>>,
    layout_revision: u64,
    picker: Option<Entity<ColorPickerState>>,
    subscriptions: Vec<Subscription>,
}

pub struct ColorSettingChanged;
impl EventEmitter<ColorSettingChanged> for ColorSetting {}
pub struct ColorSettingPopupChanged;
impl EventEmitter<ColorSettingPopupChanged> for ColorSetting {}

impl ColorSetting {
    pub fn new(
        key: &'static str,
        source: Entity<gpui_kit::component::input::InputState>,
        automatic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = ColorDraft::new(source.read(cx).value().to_string(), automatic);
        let subscription = cx.observe_in(&source, window, |this, source, window, cx| {
            if this.draft.set_text(source.read(cx).value().to_string()) {
                this.sync_picker(window, cx);
                cx.notify();
            }
        });
        Self {
            key,
            source,
            draft,
            automatic,
            inherited: 0x233631,
            disabled: false,
            open: false,
            popup_bounds: None,
            popup_geometry: None,
            popup_surface: None,
            layout_revision: 0,
            picker: None,
            subscriptions: vec![subscription],
        }
    }

    pub fn configure(&mut self, inherited: u32, disabled: bool, cx: &mut Context<Self>) {
        if self.inherited != inherited || self.disabled != disabled {
            self.inherited = inherited;
            self.disabled = disabled;
            if disabled {
                self.dismiss(cx);
            }
            cx.notify();
        }
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.popup_bounds = None;
        self.popup_geometry = None;
        self.popup_surface = None;
        cx.emit(ColorSettingPopupChanged);
        if let Some(picker) = &self.picker {
            picker.update(cx, |picker, cx| picker.set_open(false, cx));
        }
        cx.notify();
    }

    fn rgb(&self) -> u32 {
        match self.draft.choice() {
            ColorChoice::Custom(rgb) => rgb,
            _ => self.inherited,
        }
    }

    fn sync_picker(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = &self.picker {
            picker.update(cx, |picker, cx| {
                picker.set_value(gpui_kit::rgb(self.rgb()), window, cx)
            });
        }
    }

    fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.open == open {
            return;
        }
        if open && self.picker.is_none() {
            let picker = cx.new(|cx| ColorPickerState::new(window, cx));
            self.subscriptions.push(cx.subscribe_in(
                &picker,
                window,
                |this, picker, event: &ColorPickerEvent, window, cx| {
                    let was_open = this.open;
                    this.open = picker.read(cx).is_open();
                    if was_open != this.open {
                        this.layout_revision = this.layout_revision.wrapping_add(1);
                        this.popup_bounds = None;
                        this.popup_geometry = None;
                        this.popup_surface = None;
                        cx.emit(ColorSettingPopupChanged);
                    }
                    let ColorPickerEvent::Change(color) = event;
                    if let Some(color) = color {
                        this.select_color(*color, window, cx);
                    }
                },
            ));
            self.subscriptions
                .push(cx.observe(&picker, |_, _, cx| cx.notify()));
            self.picker = Some(picker);
        }
        self.open = open;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.popup_bounds = None;
        self.popup_geometry = None;
        self.popup_surface = None;
        cx.emit(ColorSettingPopupChanged);
        if open {
            // Restore from the authoritative text when reopening, including a
            // source preset changed while this field was out of the viewport.
            self.draft
                .set_text(self.source.read(cx).value().to_string());
            self.sync_picker(window, cx);
        }
        if let Some(picker) = &self.picker {
            picker.update(cx, |picker, cx| {
                picker.set_open(open, cx);
                if !open {
                    picker.clear_preview(window, cx);
                }
            });
        }
        cx.notify();
    }

    fn select_color(&mut self, mut color: Hsla, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            self.sync_picker(window, cx);
            return;
        }
        let translucent = color.a != 1.;
        color.a = 1.;
        let rgba = Rgba::from(color);
        let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u32;
        let rgb = (channel(rgba.r) << 16) | (channel(rgba.g) << 8) | channel(rgba.b);
        let before = self.draft.raw().to_owned();
        self.draft.choose_rgb(rgb);
        if translucent {
            self.sync_picker(window, cx);
        }
        if self.draft.raw() != before {
            self.write_source(window, cx);
        }
        cx.notify();
    }

    fn use_automatic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.draft.choice() == ColorChoice::Automatic {
            return;
        }
        if self.draft.use_automatic() {
            self.write_source(window, cx);
            self.sync_picker(window, cx);
            self.dismiss(cx);
        }
    }

    fn write_source(&self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.draft.raw().to_owned();
        self.source
            .update(cx, |input, cx| input.set_value(value, window, cx));
        // InputState::set_value intentionally suppresses InputEvent::Change.
        // Only this semantic event marks application settings as edited.
        cx.emit(ColorSettingChanged);
        cx.notify();
    }

    pub fn popup_bounds(&self) -> Option<Bounds<Pixels>> {
        self.popup_bounds
    }

    pub fn popup_shapes(&self) -> Vec<crate::bubble::OverlayShape> {
        self.popup_geometry
            .map(|geometry| {
                let mut shapes = vec![crate::bubble::OverlayShape::RoundedRect {
                    bounds: geometry.body,
                    radius: geometry.radius,
                }];
                shapes.extend(geometry.arrow.map(crate::bubble::OverlayShape::Triangle));
                shapes
            })
            .unwrap_or_default()
    }

    fn surface_layout(
        &mut self,
        revision: u64,
        surface: Rc<()>,
        bounds: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        if revision != self.layout_revision || !self.open || self.disabled {
            return;
        }
        if bounds.is_none() {
            if self
                .popup_surface
                .as_ref()
                .is_some_and(|active| Rc::ptr_eq(active, &surface))
            {
                self.dismiss(cx);
            }
        } else {
            self.popup_surface = Some(surface);
            if self.popup_bounds != bounds {
                self.popup_bounds = bounds;
                cx.emit(ColorSettingPopupChanged);
            }
        }
    }

    fn surface_geometry(
        &mut self,
        revision: u64,
        surface: Rc<()>,
        geometry: Option<gpui_kit::component::popover::PopoverSurfaceGeometry>,
        cx: &mut Context<Self>,
    ) {
        if revision != self.layout_revision || !self.open || self.disabled {
            return;
        }
        if geometry.is_none() {
            self.surface_layout(revision, surface, None, cx);
        } else {
            self.popup_surface = Some(surface);
            if self.popup_geometry != geometry {
                self.popup_geometry = geometry;
                self.popup_bounds = geometry.map(|geometry| geometry.body);
                cx.emit(ColorSettingPopupChanged);
            }
        }
    }

    pub fn popup_open(&self) -> bool {
        self.open
    }

    #[cfg(any(test, feature = "ui-test"))]
    pub fn picker_id(&self) -> Option<EntityId> {
        self.picker.as_ref().map(Entity::entity_id)
    }

    fn trigger(&self) -> Button {
        Button::new(SharedString::from(format!("color-open-{}", self.key)))
            .small()
            .disabled(self.disabled)
            .label(match self.draft.choice() {
                ColorChoice::Automatic => "自动适配",
                ColorChoice::Custom(_) => "自定义",
                ColorChoice::Invalid => "修正颜色",
            })
            .accessibility_label("打开颜色选择器")
            .child(
                div()
                    .size(px(24.))
                    .rounded_sm()
                    .border_1()
                    .border_color(crate::theme::color(BORDER))
                    .bg(gpui_kit::rgb(self.rgb())),
            )
    }

    fn palette(&self, cx: &mut Context<Self>) -> Div {
        let picker = self.picker.as_ref().expect("open picker exists");
        let colors = [
            0x233631, 0xFFFFFF, 0x64748B, 0xEF4444, 0xF97316, 0xEAB308, 0x22C55E, 0x06B6D4,
            0x3B82F6, 0x8B5CF6, 0xEC4899, 0xA16207,
        ];
        let mut panel = column().gap_2().child(muted("常用颜色"));
        for (group, colors) in colors.chunks(6).enumerate() {
            panel = panel.child(row().gap_2().children(colors.iter().enumerate().map(
                |(index, &rgb)| {
                    let select = picker.clone();
                    let preview = picker.clone();
                    let color: Hsla = gpui_kit::rgb(rgb).into();
                    div()
                        .id(SharedString::from(format!(
                            "color-swatch-{}-{}",
                            self.key,
                            group * 6 + index
                        )))
                        .test_support()
                        .size(px(28.))
                        .child(
                            gpui_kit::base::ColorSwatch::new(("swatch", group * 6 + index), color)
                                .cursor_pointer()
                                .size_full()
                                .rounded_md()
                                .border_1()
                                .border_color(crate::theme::color(BORDER))
                                .bg(color)
                                .selected(self.draft.choice() == ColorChoice::Custom(rgb))
                                .on_hover(move |color, entered, window, cx| {
                                    preview_swatch(&preview, color, entered, window, cx)
                                })
                                .on_click(move |color, _, window, cx| {
                                    select.update(cx, |picker, cx| {
                                        picker.select_color(color, window, cx)
                                    })
                                }),
                        )
                },
            )));
        }
        let color = self.slider_color(cx);
        // Hover is a visual preview only. Slider labels and gradients must use
        // the same committed HSL values as the actual slider entities.
        let preview = picker.read(cx).displayed_color().unwrap_or(color);
        let sliders = picker.read(cx).sliders().clone();
        for (index, label, state, number) in [
            (0, "色相", sliders.hue().clone(), color.h * 360.),
            (1, "饱和度", sliders.saturation().clone(), color.s * 100.),
            (2, "明度", sliders.lightness().clone(), color.l * 100.),
        ] {
            let track = if index == 0 {
                row().gap_0().children((0..6).map(|segment| {
                    gradient(
                        hsla(segment as f32 / 6., 1., 0.5, 1.),
                        hsla((segment + 1) as f32 / 6., 1., 0.5, 1.),
                    )
                }))
            } else if index == 1 {
                gradient(
                    hsla(color.h, 0., color.l, 1.),
                    hsla(color.h, 1., color.l, 1.),
                )
            } else {
                gradient(
                    hsla(color.h, color.s, 0., 1.),
                    hsla(color.h, color.s, 1., 1.),
                )
            };
            panel = panel.child(
                row()
                    .gap_2()
                    .child(div().w(px(48.)).text_sm().child(label))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "color-slider-{}-{index}",
                                self.key
                            )))
                            .test_support()
                            .relative()
                            .flex_1()
                            .h(px(28.))
                            .items_center()
                            .flex()
                            .child(
                                track
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .h(px(8.))
                                    .rounded_sm()
                                    .overflow_hidden(),
                            )
                            .child(Slider::new(&state).flex_1().bg(transparent_black())),
                    )
                    .child(div().w(px(32.)).text_xs().child(format!("{number:.0}"))),
            );
        }
        panel = panel.child(form::field(
            "精确色值 · #RRGGBB",
            div()
                .id(SharedString::from(format!("color-value-{}", self.key)))
                .test_support()
                .child(form::input(&self.source, self.disabled)),
        ));
        panel.child(
            row()
                .child(
                    div()
                        .size(px(28.))
                        .rounded_md()
                        .border_1()
                        .border_color(crate::theme::color(BORDER))
                        .bg(preview),
                )
                .child(div().flex_1())
                .child(
                    Button::new(SharedString::from(format!("color-close-{}", self.key)))
                        .small()
                        .label("完成")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.set_open(false, window, cx)),
                        ),
                ),
        )
    }

    fn slider_color(&self, cx: &App) -> Hsla {
        self.picker
            .as_ref()
            .and_then(|picker| picker.read(cx).value())
            .unwrap_or_else(|| gpui_kit::rgb(self.rgb()).into())
    }
}

fn preview_swatch(
    picker: &Entity<ColorPickerState>,
    color: Hsla,
    entered: bool,
    window: &mut Window,
    cx: &mut App,
) {
    picker.update(cx, |picker, cx| {
        if entered {
            picker.preview_color(color, window, cx);
        } else if picker.preview() == Some(color) {
            // Do not erase a newer swatch's preview if its enter callback ran
            // before this swatch's leave callback in the same mouse move.
            picker.clear_preview(window, cx);
        }
    });
}

fn gradient(start: Hsla, end: Hsla) -> Div {
    div().flex_1().h_full().bg(linear_gradient(
        90.,
        linear_color_stop(start, 0.),
        linear_color_stop(end, 1.),
    ))
}

impl Render for ColorSetting {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self
            .draft
            .set_text(self.source.read(cx).value().to_string())
        {
            self.sync_picker(window, cx);
        }
        let revision = self.layout_revision;
        let surface = Rc::new(());
        let weak = cx.weak_entity();
        let trigger = if self.disabled {
            self.trigger().into_any_element()
        } else {
            Popover::new("color-popover")
                .open(self.open)
                .arrow(true)
                .on_surface_geometry(move |geometry, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.surface_geometry(revision, surface.clone(), geometry, cx)
                    });
                })
                .w(px(296.))
                .on_open_change(
                    cx.listener(|this, open: &bool, window, cx| this.set_open(*open, window, cx)),
                )
                .trigger(self.trigger())
                .children(self.open.then(|| self.palette(cx)))
                .into_any_element()
        };
        let mut field = row().min_w_0().gap_2().child(trigger);
        if self.automatic && self.draft.choice() != ColorChoice::Automatic {
            field = field.child(
                Button::new(SharedString::from(format!("color-auto-{}", self.key)))
                    .small()
                    .ghost()
                    .label("自动适配")
                    .selected(self.draft.choice() == ColorChoice::Automatic)
                    .disabled(self.disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.use_automatic(window, cx))),
            );
        }
        column().min_w_0().gap_1().child(field).children(
            (self.draft.choice() == ColorChoice::Invalid).then(|| {
                muted("请输入六位 #RRGGBB 色值").text_color(crate::theme::color(0xBE3434))
            }),
        )
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{ColorSetting, preview_swatch};
    use gpui_kit::{
        AppContext, Axis, TestAppContext, VisualTestContext, component::input::InputState, hsla,
        test::TestWindowExt,
    };

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    struct ColorHarness {
        control: gpui_kit::Entity<ColorSetting>,
        mounted: bool,
        inset: f32,
    }
    impl gpui_kit::Render for ColorHarness {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{ParentElement, Styled};
            gpui_kit::div()
                .size_full()
                .pl(gpui_kit::px(self.inset))
                .children(self.mounted.then(|| self.control.clone()))
        }
    }

    #[gpui_kit::test]
    fn popup_geometry_tracks_move_unmount_and_rejects_old_callbacks(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|window, cx| {
            let source = cx.new(|cx| InputState::new(window, cx).default_value("#123456"));
            let control = cx.new(|cx| ColorSetting::new("geometry", source, false, window, cx));
            ColorHarness {
                control,
                mounted: true,
                inset: 10.,
            }
        });
        let control = host.read_with(cx, |host, _| host.control.clone());
        cx.update(|window, cx| {
            control.update(cx, |control, cx| control.set_open(true, window, cx))
        });
        draw(cx);
        let first = control.read_with(cx, |control, _| control.popup_bounds.unwrap());
        let first_shapes = control.read_with(cx, |control, _| control.popup_shapes());
        assert_eq!(first_shapes.len(), 2);
        let old = control.read_with(cx, |control, _| control.layout_revision);
        host.update(cx, |host, cx| {
            host.inset = 120.;
            cx.notify();
        });
        draw(cx);
        let moved = control.read_with(cx, |control, _| control.popup_bounds.unwrap());
        assert!(moved.left() > first.left());
        assert_ne!(
            control.read_with(cx, |control, _| control.popup_shapes()),
            first_shapes
        );
        cx.update(|window, cx| {
            control.update(cx, |control, cx| {
                control.set_open(false, window, cx);
                control.set_open(true, window, cx);
            })
        });
        draw(cx);
        control.update(cx, |control, cx| {
            let current = control.popup_bounds;
            let current_shapes = control.popup_shapes();
            control.surface_layout(old, std::rc::Rc::new(()), None, cx);
            control.surface_layout(old, std::rc::Rc::new(()), Some(first), cx);
            assert_eq!(
                control.popup_bounds, current,
                "old teardown replaced a newer popup"
            );
            control.surface_geometry(old, std::rc::Rc::new(()), None, cx);
            control.surface_geometry(control.layout_revision, std::rc::Rc::new(()), None, cx);
            assert_eq!(
                control.popup_shapes(),
                current_shapes,
                "stale shape teardown cleared current arrow"
            );
            assert!(control.open);
        });
        host.update(cx, |host, cx| {
            host.mounted = false;
            cx.notify();
        });
        draw(cx);
        draw(cx);
        assert!(
            control
                .read_with(cx, |control, _| control.popup_bounds())
                .is_none(),
            "unmounted control retained a native occlusion"
        );
        assert!(!control.read_with(cx, |control, _| control.popup_open()));
        assert!(
            control
                .read_with(cx, |control, _| control.popup_shapes())
                .is_empty()
        );
        host.update(cx, |host, cx| {
            host.mounted = true;
            cx.notify();
        });
        draw(cx);
        cx.update(|window, cx| {
            control.update(cx, |control, cx| control.set_open(true, window, cx))
        });
        draw(cx);
        assert_eq!(
            control.read_with(cx, |control, _| control.popup_bounds()),
            Some(moved)
        );
    }

    #[gpui_kit::test]
    fn popup_reports_outer_surface_and_clears_on_close_disable_and_unmount(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("#123456"));
            ColorSetting::new("layout", input, false, window, cx)
        });
        draw(cx);
        for _ in 0..2 {
            cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(true, window, cx)));
            draw(cx);
            let bounds = view
                .read_with(cx, |view, _| view.popup_bounds())
                .expect("open popup did not publish its surface");
            assert_eq!(bounds.size.width, gpui_kit::px(296.));
            assert_eq!(
                view.read_with(cx, |view, _| view.popup_shapes()).len(),
                2,
                "color popup must report its painted arrow separately"
            );
            cx.update(|window, _| {
                let input = window.find("color-value-layout").bounds();
                assert!(
                    bounds.left() < input.left() && bounds.right() > input.right(),
                    "reported content bounds omitted the popup container"
                );
                assert!(bounds.top() < input.top() && bounds.bottom() > input.bottom());
            });
            cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(false, window, cx)));
            draw(cx);
            assert!(view.read_with(cx, |view, _| view.popup_bounds()).is_none());
            assert!(view.read_with(cx, |view, _| view.popup_shapes()).is_empty());
        }
        cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(true, window, cx)));
        draw(cx);
        cx.update(|_, cx| view.update(cx, |view, cx| view.configure(0x123456, true, cx)));
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.popup_bounds()).is_none());
        assert!(view.read_with(cx, |view, _| view.popup_shapes()).is_empty());
    }

    #[gpui_kit::test]
    fn rendered_swatch_commit_clears_body_and_arrow_before_and_after_teardown(
        cx: &mut TestAppContext,
    ) {
        use crate::bubble::OverlayShape;
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("#123456"));
            ColorSetting::new("swatch-close", input, false, window, cx)
        });
        let assert_open = |view: &ColorSetting| {
            assert!(view.popup_open());
            assert!(view.popup_bounds().is_some());
            let shapes = view.popup_shapes();
            assert_eq!(shapes.len(), 2);
            assert!(matches!(shapes[0], OverlayShape::RoundedRect { .. }));
            assert!(matches!(shapes[1], OverlayShape::Triangle(_)));
        };
        let assert_closed = |view: &ColorSetting, cx: &gpui_kit::App| {
            assert_eq!(view.source.read(cx).value().as_str(), "#EF4444");
            assert!(!view.popup_open());
            assert!(view.popup_bounds().is_none());
            assert!(
                view.popup_shapes().is_empty(),
                "swatch event left the closed popup's body and arrow active"
            );
        };
        draw(cx);
        cx.update(|window, cx| window.click("color-open-swatch-close", cx));
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.source.read(cx).value().as_str(), "#123456");
            assert_open(view);
        });
        let picker_id = view.read_with(cx, |view, _| view.picker_id());
        // Exercise the rendered ColorSwatch -> select_color -> Change event,
        // rather than directly closing the control or invoking its setter.
        cx.update(|window, cx| window.click("color-swatch-swatch-close-3", cx));
        view.read_with(cx, |view, cx| assert_closed(view, cx));
        for _ in 0..3 {
            draw(cx);
            view.read_with(cx, |view, cx| assert_closed(view, cx));
        }
        cx.update(|window, cx| window.click("color-open-swatch-close", cx));
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_open(view);
            assert_eq!(view.picker_id(), picker_id);
        });
        cx.update(|window, cx| window.click("color-close-swatch-close", cx));
        view.read_with(cx, |view, cx| assert_closed(view, cx));
        for _ in 0..3 {
            draw(cx);
            view.read_with(cx, |view, cx| assert_closed(view, cx));
        }
    }

    #[gpui_kit::test]
    fn collapsed_color_setting_uses_swatch_and_keeps_hex_inside_popup(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("#123456"));
            ColorSetting::new("compact", input, true, window, cx)
        });
        draw(cx);
        cx.update(|window, _| {
            assert!(
                window.try_find("color-value-compact").is_none(),
                "collapsed settings still mount a full HEX input"
            );
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(true, window, cx)));
        draw(cx);
        cx.update(|window, _| {
            assert!(
                window.try_find("color-value-compact").is_some(),
                "precision input must be available in the color popup"
            );
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(false, window, cx)));
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.source.read(cx).value().as_str(), "#123456")
        });
    }

    #[gpui_kit::test]
    fn color_picker_is_lazy_reused_and_keeps_opaque_draft_after_close(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("#123456"));
            ColorSetting::new("test", input, true, window, cx)
        });
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.picker.is_none()));
        cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(true, window, cx)));
        draw(cx);
        let picker = view.read_with(cx, |view, _| {
            view.picker
                .clone()
                .expect("opening did not allocate the picker")
        });
        let id = picker.entity_id();
        cx.update(|window, cx| {
            picker.update(cx, |picker, cx| {
                picker.select_color(hsla(0., 1., 0.5, 0.2), window, cx)
            })
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.source.read(cx).value().as_str(), "#FF0000");
            assert!(!view.open);
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(true, window, cx)));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.picker.as_ref().unwrap().entity_id()),
            id
        );
        cx.update(|window, cx| {
            picker.update(cx, |picker, cx| {
                picker.preview_color(hsla(0.5, 1., 0.5, 1.), window, cx)
            });
            view.update(cx, |view, cx| view.set_open(false, window, cx));
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.source.read(cx).value().as_str(),
                "#FF0000",
                "closing committed a hovered preview"
            )
        });
        cx.update(|window, cx| {
            let source = view.read(cx).source.clone();
            source.update(cx, |input, cx| input.set_value("#AABBCC", window, cx));
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.draft.raw(),
                "#AABBCC",
                "programmatic assignment did not synchronize the control"
            );
            assert_eq!(view.source.read(cx).value().as_str(), "#AABBCC");
        });
    }

    #[gpui_kit::test]
    fn automatic_and_disabled_controls_do_not_allocate_or_discard_invalid_text(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("#123456"));
            ColorSetting::new("automatic-test", input, true, window, cx)
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.use_automatic(window, cx)));
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(view.source.read(cx).value().is_empty());
            assert!(
                view.picker.is_none(),
                "automatic mode unnecessarily allocated a popup"
            );
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.configure(0xABCDEF, true, cx);
                view.set_open(true, window, cx);
            })
        });
        assert!(view.read_with(cx, |view, _| view.picker.is_none()));
        cx.update(|window, cx| {
            let source = view.read(cx).source.clone();
            source.update(cx, |input, cx| input.set_value("#abc", window, cx));
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.draft.choice(),
                crate::settings_color_value::ColorChoice::Invalid
            );
            assert_eq!(view.source.read(cx).value().as_str(), "#abc");
        });
    }

    #[gpui_kit::test]
    fn popup_children_accept_slider_and_hex_input_after_pointer_containment(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("#FF0000"));
            ColorSetting::new("pointer", input, false, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| {
            window.click("color-open-pointer", cx);
            window.render_frame(cx);
            window.click("color-slider-pointer-0", cx);
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.source.read(cx).value().as_str(), "#00FFFF");
            assert!(view.popup_open());
        });
        cx.update(|window, cx| {
            window.click("color-value-pointer", cx);
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-a"
                } else {
                    "ctrl-a"
                },
                cx,
            );
            window.input("#AABBCC", cx);
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.source.read(cx).value().as_str(), "#AABBCC");
            assert_eq!(view.draft.raw(), "#AABBCC");
        });
        cx.update(|window, cx| window.click("color-close-pointer", cx));
        draw(cx);
        assert!(!view.read_with(cx, |view, _| view.popup_open()));
    }

    #[gpui_kit::test]
    fn hover_preview_leaves_slider_values_consistent_and_clears_on_exit(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("#FF0000"));
            ColorSetting::new("hover", input, false, window, cx)
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.set_open(true, window, cx)));
        draw(cx);
        let picker = view.read_with(cx, |view, _| view.picker.clone().unwrap());
        let white = hsla(0., 0., 1., 1.);
        cx.update(|window, cx| preview_swatch(&picker, white, true, window, cx));
        draw(cx);
        view.read_with(cx, |view, cx| {
            let color = view.slider_color(cx);
            let sliders = picker.read(cx).sliders();
            assert_eq!(
                color.s,
                sliders.saturation().read(cx).value().start(),
                "hover changed the shown saturation without changing its slider"
            );
            assert_eq!(color.l, sliders.lightness().read(cx).value().start());
            assert_eq!(view.source.read(cx).value().as_str(), "#FF0000");
        });
        cx.update(|window, cx| preview_swatch(&picker, white, false, window, cx));
        draw(cx);
        cx.update(|window, cx| {
            assert_eq!(
                picker.read(cx).preview(),
                picker.read(cx).value(),
                "leaving a swatch retained a stale preview"
            );
            let bounds = window.find("color-slider-hover-0").bounds();
            let hue = picker.read(cx).sliders().hue().clone();
            hue.update(cx, |slider, cx| {
                slider.update_value_by_position(
                    Axis::Horizontal,
                    bounds.center(),
                    false,
                    window,
                    cx,
                )
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.source.read(cx).value().as_str(),
                "#00FFFF",
                "hue slider committed a color other than the displayed HSL state"
            )
        });
    }
}
