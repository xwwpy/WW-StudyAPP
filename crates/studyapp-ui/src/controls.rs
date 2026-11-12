use gpui_kit::component::{
    Disableable, FocusableExt, Icon, Selectable, Sizable, Size,
    button::{Button as ComponentButton, ButtonCustomVariant, ButtonVariants},
    checkbox::Checkbox as ComponentCheckbox,
    text::Text,
};
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, Entity, InteractiveElement, Interactivity, IntoElement,
    ParentElement, Pixels, Point, Render, RenderOnce, SharedString, StyleRefinement, Styled,
    Window,
};

/// Application buttons share one cursor policy while the component retains
/// ownership of activation, focus, accessibility and visual states.
#[derive(IntoElement)]
pub struct AppButton {
    inner: ComponentButton,
    disabled: bool,
    loading: bool,
    tooltip_text: Option<SharedString>,
    size: Size,
    has_icon: bool,
    has_label: bool,
    has_children: bool,
    target_scale: f32,
    selection_state: Option<bool>,
    framed_selection: bool,
    selection_category: Option<studyapp_core::SelectionCategory>,
    surface_colors: Option<(gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla)>,
}

pub use AppButton as Button;

impl crate::menu::DropdownMenu for AppButton {}

impl AppButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            inner: ComponentButton::new(id),
            disabled: false,
            loading: false,
            tooltip_text: None,
            size: Size::Medium,
            has_icon: false,
            has_label: false,
            has_children: false,
            target_scale: 1.,
            selection_state: None,
            framed_selection: true,
            selection_category: Some(studyapp_core::SelectionCategory::Options),
            surface_colors: None,
        }
    }

    pub fn selection_category(mut self, category: studyapp_core::SelectionCategory) -> Self {
        self.selection_category = Some(category);
        self
    }

    /// Flat navigation keeps semantic selection and inherited colors without
    /// adding the ordinary option-button outline or trailing selection dot.
    pub fn flat_selection(mut self) -> Self {
        self.framed_selection = false;
        self
    }

    pub fn global_selection(mut self) -> Self {
        self.selection_category = None;
        self
    }

    /// Scale minimum pointer targets alongside custom-sized child content.
    pub fn target_scale(mut self, factor: f32) -> Self {
        self.target_scale = if factor.is_finite() {
            factor.clamp(0.5, 2.)
        } else {
            1.
        };
        self
    }

    /// Transparent button states with explicit foreground and alpha-aware feedback.
    pub fn transparent_surface(
        mut self,
        foreground: gpui_kit::Hsla,
        hover: gpui_kit::Hsla,
        active: gpui_kit::Hsla,
    ) -> Self {
        self.surface_colors = Some((foreground, hover, active));
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.has_label = true;
        self.inner = self.inner.label(label);
        self
    }

    /// Form navigation keeps the complete name at narrow widths and large
    /// type; the component still owns focus, activation and the accessible name.
    pub fn wrapped_label(mut self, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        self.has_label = true;
        self.has_children = true;
        self.inner = self
            .inner
            .accessibility_label(label.clone())
            .h_auto()
            .child(
                gpui_kit::div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(gpui_kit::px(crate::theme::palette().font_size as f32))
                    .child(label),
            );
        self
    }

    /// Icon-first operation with an explicit name. Unknown operations remain
    /// readable text; parameter values and destructive confirmations use label().
    pub fn action(self, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        match crate::action_icons::action_icon(&label) {
            Some(icon) => self.icon_action(
                icon,
                match label.as_ref() {
                    "+" | "＋" => "增加".into(),
                    "−" => "减少".into(),
                    _ => label,
                },
            ),
            None => self.label(label),
        }
    }

    pub fn icon_action(self, icon: impl Into<Icon>, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        self.icon(icon)
            .accessibility_label(label.clone())
            .tooltip(label)
    }

    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.accessibility_label(label);
        self
    }

    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        let icon: Icon = icon.into();
        self.has_icon = true;
        self.inner = self.inner.icon(icon);
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip_text = Some(tooltip.into());
        self
    }

    /// Expose a real toggle operation to assistive technology. Selected styling
    /// alone does not imply a pressed state for an ordinary operation button.
    pub fn toggled(mut self, toggled: bool) -> Self {
        self.inner = self.inner.toggled(toggled);
        self
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self.inner = self.inner.loading(loading);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.inner = self.inner.on_click(handler);
        self
    }

    /// Register drag on the same base surface as click/focus, so crossing the
    /// drag threshold uses GPUI's existing suppression of the pending click.
    pub fn on_drag<T: 'static, V: Render + 'static>(
        mut self,
        value: T,
        constructor: impl Fn(&T, Point<Pixels>, &mut Window, &mut App) -> Entity<V> + 'static,
    ) -> Self {
        self.inner.interactivity().on_drag(value, constructor);
        self
    }

    // Apply after every caller style. The component replays this instance
    // style over its own default and disabled layers during rendering.
    fn into_button(mut self) -> ComponentButton {
        let nominal: f32 = match self.size {
            Size::XSmall => 28.,
            Size::Small => 32.,
            Size::Medium | Size::Large => 36.,
            Size::Size(value) => value.into(),
        };
        let mut minimum =
            nominal.max(crate::theme::palette().font_size as f32 * 1.25 + 8.) * self.target_scale;
        // Do not replace a caller's larger minimum (e.g. navigation rows)
        // with the small-button baseline. Explicit relative minima stay relative.
        match self.inner.style().min_size.height {
            Some(gpui_kit::Length::Definite(gpui_kit::DefiniteLength::Absolute(
                gpui_kit::AbsoluteLength::Pixels(requested),
            ))) => {
                minimum = minimum.max(requested.into());
                self.inner = self.inner.min_h(gpui_kit::px(minimum));
            }
            None | Some(gpui_kit::Length::Auto) => {
                self.inner = self.inner.min_h(gpui_kit::px(minimum));
            }
            _ => {}
        }
        // The component's small icon default is 24px wide. Raising only its
        // height produces off-center rectangular targets throughout toolbars.
        // Keep explicit caller dimensions and text/navigation buttons intact.
        if self.has_icon && !self.has_label && !self.has_children {
            // A minimum alone still lets the component's horizontal padding
            // or its parent's stretch turn an icon target into a rectangle.
            // Preserve explicit caller sizes (titlebars and scaled widgets).
            let implicit_width = self.inner.style().size.width.is_none();
            let implicit_height = self.inner.style().size.height.is_none();
            if implicit_width {
                self.inner = self.inner.w(gpui_kit::px(minimum)).px_0();
            }
            if implicit_height {
                self.inner = self.inner.h(gpui_kit::px(minimum));
            }
            self.inner = self.inner.min_w(gpui_kit::px(minimum));
        }
        if self.disabled || self.loading {
            self.inner.cursor_default()
        } else {
            self.inner.cursor_pointer()
        }
    }
}

impl Disableable for AppButton {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.inner = self.inner.disabled(disabled);
        self
    }
}

impl Selectable for AppButton {
    fn selected(mut self, selected: bool) -> Self {
        self.selection_state = Some(selected);
        self.inner = self.inner.selected(selected);
        self
    }

    fn is_selected(&self) -> bool {
        self.inner.is_selected()
    }

    fn open(mut self, open: bool) -> Self {
        self.inner = self.inner.open(open);
        self
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }
}

impl Sizable for AppButton {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self.inner = self.inner.with_size(self.size);
        self
    }

    fn prepare_for_toolbar(mut self) -> Self {
        self.inner = self.inner.prepare_for_toolbar();
        self
    }
}

impl ButtonVariants for AppButton {
    fn with_variant(mut self, variant: gpui_kit::component::button::ButtonVariant) -> Self {
        self.inner = self.inner.with_variant(variant);
        self
    }
}

impl FocusableExt for AppButton {
    fn focus_ring(mut self, enabled: bool) -> Self {
        self.inner = self.inner.focus_ring(enabled);
        self
    }

    fn is_focus_ring_enabled(&self) -> bool {
        self.inner.is_focus_ring_enabled()
    }
}

impl Styled for AppButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}

impl InteractiveElement for AppButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }
}

impl ParentElement for AppButton {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        let mut elements = elements.into_iter().peekable();
        self.has_children |= elements.peek().is_some();
        self.inner.extend(elements);
    }
}

impl RenderOnce for AppButton {
    fn render(mut self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        if let Some(text) = self.tooltip_text.take() {
            self.inner = crate::components::control_tooltip(self.inner, text.to_string());
        }
        if self.surface_colors.is_none()
            && let Some(selected) = self.selection_state
        {
            let p = self
                .selection_category
                .map_or_else(crate::theme::global_selection, crate::theme::selection);
            let bg = if selected {
                p.selected_background
            } else {
                p.idle_background
            };
            let fg = if selected {
                p.selected_text
            } else {
                p.idle_text
            };
            self.inner = self.inner.custom(
                ButtonCustomVariant::new(cx)
                    .color(gpui_kit::rgb(bg).into())
                    .foreground(gpui_kit::rgb(fg).into())
                    .hover(crate::theme::hover_color())
                    // ComponentButton uses the active variant for its selected
                    // surface. Keep persistent selection distinct from pressing.
                    .active(if selected {
                        gpui_kit::rgb(bg).into()
                    } else {
                        crate::theme::pressed_color()
                    }),
            );
            self.inner = self
                .inner
                .rounded(gpui_kit::px(f32::from(p.selection_radius)));
            if self.framed_selection {
                self.inner = self
                    .inner
                    .border_1()
                    .border_color(gpui_kit::rgb(if selected { p.selected_border } else { bg }));
            } else {
                self.inner = self.inner.border_0();
            }
            if self.framed_selection && selected && p.selection_indicator {
                self.inner = self.inner.child(
                    gpui_kit::div()
                        .size(gpui_kit::px(4.))
                        .rounded_full()
                        .bg(gpui_kit::rgb(p.selected_border)),
                );
            }
            if selected {
                self.inner = self.inner.font_weight(gpui_kit::FontWeight::SEMIBOLD);
            }
        }
        if let Some((foreground, hover, active)) = self.surface_colors {
            self.inner = self.inner.custom(
                ButtonCustomVariant::new(cx)
                    .foreground(foreground)
                    .hover(hover)
                    .active(active),
            );
        }
        self.into_button()
    }
}

/// Controlled checkbox values and activation remain owned by the component.
#[derive(IntoElement)]
pub struct AppCheckbox {
    inner: ComponentCheckbox,
    disabled: bool,
}

pub use AppCheckbox as Checkbox;

impl AppCheckbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            inner: ComponentCheckbox::new(id),
            disabled: false,
        }
    }

    pub fn label(mut self, label: impl Into<Text>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.accessibility_label(label);
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.inner = self.inner.checked(checked);
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.inner = self.inner.on_click(handler);
        self
    }

    fn into_checkbox(self) -> ComponentCheckbox {
        if self.disabled {
            self.inner.cursor_default()
        } else {
            self.inner.cursor_pointer()
        }
    }
}

impl Disableable for AppCheckbox {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.inner = self.inner.disabled(disabled);
        self
    }
}

impl Styled for AppCheckbox {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}

impl InteractiveElement for AppCheckbox {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }
}

impl gpui_kit::StatefulInteractiveElement for AppCheckbox {}

impl ParentElement for AppCheckbox {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.extend(elements);
    }
}

impl Selectable for AppCheckbox {
    fn selected(mut self, selected: bool) -> Self {
        self.inner = self.inner.selected(selected);
        self
    }

    fn is_selected(&self) -> bool {
        self.inner.is_selected()
    }
}

impl Sizable for AppCheckbox {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.inner = self.inner.with_size(size);
        self
    }
}

impl FocusableExt for AppCheckbox {
    fn focus_ring(mut self, enabled: bool) -> Self {
        self.inner = self.inner.focus_ring(enabled);
        self
    }

    fn is_focus_ring_enabled(&self) -> bool {
        self.inner.is_focus_ring_enabled()
    }
}

impl RenderOnce for AppCheckbox {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.into_checkbox()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::CursorStyle;
    #[cfg(feature = "ui-test")]
    use gpui_kit::{Context, Render, TestAppContext, div};

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn compact_buttons_have_readable_minimum_targets_at_large_type(cx: &mut TestAppContext) {
        use gpui_kit::{TestSupportExt, px, test::TestWindowExt};
        struct Fixture;
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                crate::components::card()
                    .id("control-card")
                    .test_support()
                    .w(px(400.))
                    .child(Button::new("compact-control").small().label("操作"))
                    .child(
                        Button::new("explicit-minimum")
                            .small()
                            .min_h(px(48.))
                            .label("宽松操作"),
                    )
                    .child(
                        Button::new("undersized-minimum")
                            .small()
                            .min_h(px(1.))
                            .label("仍保持可点击尺寸"),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| Fixture);
        for size in [12, 14, 18, 22] {
            cx.update(|window, cx| {
                crate::theme::apply(
                    &studyapp_core::AppearanceSettings {
                        font_size: size,
                        ..Default::default()
                    },
                    false,
                    cx,
                );
                window.render_frame(cx);
                let card = window.find("control-card").bounds();
                for (id, minimum) in [
                    ("compact-control", 32.),
                    ("explicit-minimum", 48.),
                    ("undersized-minimum", 32.),
                ] {
                    let button = window.find(id).bounds();
                    assert!(button.size.height >= px(minimum), "{id}: {button:?}");
                    assert!(button.size.height >= px(size as f32 * 1.25 + 8.));
                    assert!(button.bottom() <= card.bottom());
                }
            });
        }
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn icon_targets_are_square_without_widening_navigation_or_losing_explicit_titlebar_size(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{component::IconName, px, test::TestWindowExt};
        struct Fixture;
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                crate::components::column()
                    .w(px(500.))
                    .child(
                        crate::components::row()
                            .child(Button::new("xs-square").xsmall().action("设置"))
                            .child(Button::new("small-square").small().action("暂停"))
                            .child(Button::new("medium-square").action("保存"))
                            .child(
                                Button::new("large-minimum-square")
                                    .small()
                                    .min_h(px(48.))
                                    .action("设置"),
                            )
                            .child(
                                Button::new("titlebar-icon")
                                    .icon_action(IconName::Close, "关闭")
                                    .w(px(44.))
                                    .h(px(44.)),
                            ),
                    )
                    .child(
                        Button::new("navigation-text-icon")
                            .icon(IconName::Settings)
                            .label("通知设置")
                            .w_full(),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let (_, visual) = cx.add_window_view(|_, _| Fixture);
        for font_size in [12, 14, 18, 22] {
            visual.update(|window, cx| {
                crate::theme::apply(
                    &studyapp_core::AppearanceSettings {
                        font_size,
                        ..Default::default()
                    },
                    false,
                    cx,
                );
                window.render_frame(cx);
                for (id, nominal) in [
                    ("xs-square", 28_f32),
                    ("small-square", 32.),
                    ("medium-square", 36.),
                    ("large-minimum-square", 48.),
                ] {
                    let bounds = window.find(id).bounds();
                    assert!(
                        (bounds.size.width - bounds.size.height).abs() < px(0.1),
                        "{id}: {bounds:?}"
                    );
                    assert!(bounds.size.width >= px(nominal.max(font_size as f32 * 1.25 + 8.)));
                }
                assert_eq!(
                    window.find("titlebar-icon").bounds().size,
                    gpui_kit::size(px(44.), px(44.))
                );
                assert_eq!(
                    window.find("navigation-text-icon").label(),
                    Some("通知设置")
                );
                assert_eq!(
                    window.find("navigation-text-icon").bounds().size.width,
                    px(500.)
                );
            });
        }
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn icon_actions_keep_names_keyboard_activation_and_disabled_inertia(cx: &mut TestAppContext) {
        use gpui_kit::{px, test::TestWindowExt};
        use std::{cell::Cell, rc::Rc};
        struct Fixture(Rc<Cell<usize>>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let enabled = self.0.clone();
                let disabled = self.0.clone();
                let loading = self.0.clone();
                crate::components::column()
                    .tab_group()
                    .child(
                        Button::new("pause-icon")
                            .small()
                            .action("暂停")
                            .on_click(move |_, _, _| enabled.set(enabled.get() + 1)),
                    )
                    .child(
                        Button::new("disabled-icon")
                            .small()
                            .action("保存")
                            .disabled(true)
                            .on_click(move |_, _, _| disabled.set(1000)),
                    )
                    .child(
                        Button::new("loading-icon")
                            .small()
                            .action("保存")
                            .loading(true)
                            .on_click(move |_, _, _| loading.set(1000)),
                    )
                    .child(Button::new("duration-value").action("25 分钟"))
                    .child(Button::new("confirm-text").label("确认重新开始"))
            }
        }
        cx.update(gpui_kit::init);
        let count = Rc::new(Cell::new(0));
        let (_, visual) = cx.add_window_view({
            let count = count.clone();
            move |_, _| Fixture(count)
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("pause-icon").label(), Some("暂停"));
            assert_eq!(window.find("duration-value").label(), Some("25 分钟"));
            assert_eq!(window.find("confirm-text").label(), Some("确认重新开始"));
            assert!(window.find("pause-icon").bounds().size.width < px(60.));
            window.click("pause-icon", cx);
            assert_eq!(count.get(), 1);
            window.focus_next(cx);
            window.render_frame(cx);
            assert!(window.focused(cx).is_some());
        });
        for key in ["enter", "space"] {
            let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
            visual.simulate_event(gpui_kit::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            visual.simulate_event(gpui_kit::KeyUpEvent { keystroke });
        }
        assert_eq!(count.get(), 3);
        visual.update(|window, cx| {
            window.click("disabled-icon", cx);
            window.click("loading-icon", cx);
            assert_eq!(count.get(), 3);
        });
    }

    #[::core::prelude::v1::test]
    fn cursor_follows_final_disabled_and_loading_state() {
        let mut enabled = Button::new("enabled").into_button();
        assert_eq!(
            enabled.style().mouse_cursor,
            Some(CursorStyle::PointingHand)
        );

        let mut disabled = Button::new("disabled")
            .disabled(true)
            .cursor_pointer()
            .into_button();
        assert_eq!(disabled.style().mouse_cursor, Some(CursorStyle::Arrow));

        let mut loading = Button::new("loading")
            .loading(true)
            .cursor_pointer()
            .into_button();
        assert_eq!(loading.style().mouse_cursor, Some(CursorStyle::Arrow));

        let mut enabled_again = Button::new("enabled-again")
            .disabled(true)
            .loading(true)
            .disabled(false)
            .loading(false)
            .cursor_default()
            .into_button();
        assert_eq!(
            enabled_again.style().mouse_cursor,
            Some(CursorStyle::PointingHand)
        );
    }

    #[::core::prelude::v1::test]
    fn checkbox_cursor_follows_final_disabled_state() {
        let mut enabled = Checkbox::new("enabled").into_checkbox();
        assert_eq!(
            enabled.style().mouse_cursor,
            Some(CursorStyle::PointingHand)
        );

        let mut disabled = Checkbox::new("disabled")
            .disabled(true)
            .cursor_pointer()
            .into_checkbox();
        assert_eq!(disabled.style().mouse_cursor, Some(CursorStyle::Arrow));

        let mut enabled_again = Checkbox::new("enabled-again")
            .disabled(true)
            .disabled(false)
            .cursor_default()
            .into_checkbox();
        assert_eq!(
            enabled_again.style().mouse_cursor,
            Some(CursorStyle::PointingHand)
        );
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn facade_preserves_enabled_activation_and_disabled_loading_inertia(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt;
        use std::{cell::Cell, rc::Rc};

        struct Fixture(Rc<Cell<usize>>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().flex().flex_col().children(
                    [
                        ("enabled", false, false),
                        ("disabled", true, false),
                        ("loading", false, true),
                    ]
                    .into_iter()
                    .map(|(id, disabled, loading)| {
                        let clicks = self.0.clone();
                        Button::new(id)
                            .label(id)
                            .disabled(disabled)
                            .loading(loading)
                            .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    }),
                )
            }
        }

        cx.update(gpui_kit::init);
        let clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let clicks = clicks.clone();
            move |_, _| Fixture(clicks)
        });
        cx.update(|window, cx| {
            window.click("enabled", cx);
            window.click("disabled", cx);
            window.click("loading", cx);
        });
        assert_eq!(clicks.get(), 1);
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn button_drag_preserves_click_and_context_menu_routing(cx: &mut TestAppContext) {
        use crate::menu::{ContextMenuExt, PopupMenuItem};
        use gpui_kit::{AppContext, MouseButton, point, px};
        use std::{cell::Cell, rc::Rc};

        struct Preview;
        impl Render for Preview {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().child("Folder")
            }
        }
        struct Fixture {
            clicks: Rc<Cell<usize>>,
            drags: Rc<Cell<usize>>,
            menus: Rc<Cell<usize>>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let clicks = self.clicks.clone();
                let drags = self.drags.clone();
                let menus = self.menus.clone();
                Button::new("folder-drag-facade")
                    .label("Folder")
                    .w(gpui_kit::px(160.))
                    .h(gpui_kit::px(40.))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    .on_drag("folder", move |_, _, _, cx| {
                        drags.set(drags.get() + 1);
                        cx.new(|_| Preview)
                    })
                    .context_menu(move |menu, _, _| {
                        menus.set(menus.get() + 1);
                        menu.item(PopupMenuItem::new("Rename"))
                    })
            }
        }
        cx.update(gpui_kit::init);
        let clicks = Rc::new(Cell::new(0));
        let drags = Rc::new(Cell::new(0));
        let menus = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let clicks = clicks.clone();
            let drags = drags.clone();
            let menus = menus.clone();
            move |_, _| Fixture {
                clicks,
                drags,
                menus,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let start = point(px(24.), px(20.));
        cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
        cx.simulate_mouse_up(start, MouseButton::Left, Default::default());
        assert_eq!(clicks.get(), 1);
        assert_eq!(drags.get(), 0);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
        let moved = start + point(px(28.), px(0.));
        cx.simulate_mouse_move(moved, MouseButton::Left, Default::default());
        assert_eq!(drags.get(), 1);
        cx.simulate_mouse_up(moved, MouseButton::Left, Default::default());
        assert_eq!(clicks.get(), 1, "Dropping cannot also select the source");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(start, MouseButton::Right, Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(menus.get(), 1);
        assert_eq!(drags.get(), 1, "Right-click cannot begin a directory drag");
    }

    #[cfg(feature = "ui-test")]
    #[gpui_kit::test]
    fn checkbox_facade_preserves_controlled_value_and_disabled_inertia(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt;
        use std::{cell::Cell, rc::Rc};

        struct Fixture(Rc<Cell<bool>>, Rc<Cell<usize>>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let checked = self.0.clone();
                let calls = self.1.clone();
                let disabled_calls = self.1.clone();
                div()
                    .flex()
                    .flex_col()
                    .child(
                        Checkbox::new("enabled-checkbox")
                            .label("Enabled")
                            .checked(self.0.get())
                            .on_click(move |value, _, cx| {
                                checked.set(*value);
                                calls.set(calls.get() + 1);
                                cx.refresh_windows();
                            }),
                    )
                    .child(
                        Checkbox::new("disabled-checkbox")
                            .label("Disabled")
                            .disabled(true)
                            .on_click(move |_, _, _| disabled_calls.set(disabled_calls.get() + 1)),
                    )
            }
        }

        cx.update(gpui_kit::init);
        let checked = Rc::new(Cell::new(false));
        let calls = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let checked = checked.clone();
            let calls = calls.clone();
            move |_, _| Fixture(checked, calls)
        });
        cx.update(|window, cx| {
            window.click("enabled-checkbox", cx);
            window.click("disabled-checkbox", cx);
            assert_eq!(window.find("enabled-checkbox").checked(), Some(true));
        });
        assert!(checked.get());
        assert_eq!(calls.get(), 1);
    }
}
