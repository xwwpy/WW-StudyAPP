use gpui_kit::component::{
    Disableable, FocusableExt, Icon, Selectable, Sizable, Size,
    button::{Button as ComponentButton, ButtonVariants},
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
    size: Size,
}

pub use AppButton as Button;

impl gpui_kit::component::menu::DropdownMenu for AppButton {}

impl AppButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            inner: ComponentButton::new(id),
            disabled: false,
            loading: false,
            size: Size::Medium,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.accessibility_label(label);
        self
    }

    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        let icon: Icon = icon.into();
        self.inner = self.inner.icon(icon);
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.inner = self.inner.tooltip(tooltip);
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
        let minimum = nominal.max(crate::theme::palette().font_size as f32 * 1.25 + 8.);
        self.inner = self.inner.min_h(gpui_kit::px(minimum));
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
        self.inner.extend(elements);
    }
}

impl RenderOnce for AppButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
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
                let button = window.find("compact-control").bounds();
                let card = window.find("control-card").bounds();
                assert!(button.size.height >= px(32.));
                assert!(button.size.height >= px(size as f32 * 1.25 + 8.));
                assert!(button.bottom() <= card.bottom());
            });
        }
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
        use gpui_kit::{
            AppContext, MouseButton,
            component::menu::{ContextMenuExt, PopupMenuItem},
            point, px,
        };
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
