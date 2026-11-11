//! Shared neutral form surfaces, labels and inputs for the settings workspace.
use crate::components::*;
use crate::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    component::{
        Sizable,
        input::{Input, InputState},
        tooltip::Tooltip,
    },
    *,
};
use std::cell::RefCell;

/// Keep two independently scrollable surfaces visible inside their allocated
/// workspace. Flex proportions adapt to height without minimum panel guesses.
pub fn adaptive_split(
    available: Size<Pixels>,
    primary: impl IntoElement,
    secondary: impl IntoElement,
) -> Div {
    let layout = if available.width >= px(820.) {
        row().gap_3()
    } else {
        column().gap_2()
    };
    layout
        .flex_1()
        .min_w_0()
        .min_h_0()
        .items_stretch()
        .child(
            div()
                .flex()
                .flex_basis(px(0.))
                .flex_grow(0.44)
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(primary),
        )
        .child(
            div()
                .flex()
                .flex_basis(px(0.))
                .flex_grow(0.56)
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(secondary),
        )
}

/// Keeps variable-height form rows virtualized without resetting the user's
/// scroll position when a binding, conflict, or inline recorder changes height.
pub struct VirtualRows<K> {
    pub scroll: ListState,
    source: RefCell<Option<(Vec<usize>, K)>>,
    focused: RefCell<Vec<(usize, FocusHandle)>>,
}

impl<K> Default for VirtualRows<K> {
    fn default() -> Self {
        Self {
            scroll: ListState::new(0, ListAlignment::Top, px(48.)),
            source: RefCell::new(None),
            focused: RefCell::new(Vec::new()),
        }
    }
}

impl<K: PartialEq> VirtualRows<K> {
    pub fn sync(&self, rows: &[usize], layout: K) {
        self.sync_with_height(rows, layout, px(58.));
    }

    pub fn sync_with_height(&self, rows: &[usize], layout: K, estimate: Pixels) {
        let mut source = self.source.borrow_mut();
        match source.as_ref() {
            Some((previous_rows, previous_layout)) if previous_rows == rows => {
                if previous_layout == &layout {
                    return;
                }
                self.scroll.remeasure();
            }
            _ => {
                self.scroll.reset_with_uniform_height(rows.len(), estimate);
                self.focused.borrow_mut().clear();
            }
        }
        *source = Some((rows.to_vec(), layout));
    }

    /// Keep the one active inline recorder in the keyboard dispatch tree when
    /// the user scrolls it off screen. Other offscreen rows remain unrendered.
    pub fn track_focus(&self, next: Option<(usize, FocusHandle)>) {
        self.track_focuses(next.into_iter().collect());
    }

    pub fn track_focuses(&self, next: Vec<(usize, FocusHandle)>) {
        let mut focused = self.focused.borrow_mut();
        if *focused == next {
            return;
        }
        for (row, _) in focused
            .iter()
            .filter(|(row, _)| !next.iter().any(|(next, _)| next == row))
        {
            self.scroll.set_item_focus_handle(*row, None);
        }
        for (row, handle) in &next {
            if !focused.iter().any(|entry| entry == &(*row, handle.clone())) {
                self.scroll
                    .set_item_focus_handle(*row, Some(handle.clone()));
            }
        }
        *focused = next;
    }

    pub fn scroll_to_top(&self) {
        self.scroll.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
    }
}

pub fn virtual_rows(
    state: &ListState,
    mut render: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
) -> impl IntoElement {
    div()
        .id("settings-section-scroll")
        .test_support()
        .relative()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .rounded(px(crate::design::CARD_RADIUS))
        .border_1()
        .border_color(crate::theme::color(BORDER))
        .bg(crate::theme::color(0xFFFFFF))
        // The list owns this complete viewport, including its left padding,
        // so wheels over the gutter do not escape to an outer scroll surface.
        .child(
            list(state.clone(), move |row, window, cx| {
                div()
                    .w_full()
                    .min_w_0()
                    .px_4()
                    .child(render(row, window, cx))
                    .into_any_element()
            })
            .size_full()
            .pb_4(),
        )
        .child(crate::virtual_list_scrollbar::virtual_list_scrollbar(
            "settings-section-scrollbar",
            state,
            px(0.),
        ))
}

pub fn card_grid(width: Pixels) -> Div {
    div()
        .grid()
        .grid_cols(if width >= px(800.) { 2 } else { 1 })
        .gap_4()
        .w_full()
        .min_w_0()
        .items_start()
}

pub fn section(title: impl Into<SharedString>) -> Div {
    let title = title.into();
    column()
        .relative()
        .child(crate::settings_anchor::probe(title.clone()))
        .min_w_0()
        .flex_shrink_0()
        .gap_3()
        .p_4()
        .rounded(px(crate::design::CARD_RADIUS))
        .border_1()
        .border_color(crate::theme::color(BORDER))
        .bg(crate::theme::color(0xFFFFFF))
        .child(heading(title))
}

pub fn field(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    let label = label.into();
    column()
        .relative()
        .child(crate::settings_anchor::probe(label.clone()))
        .min_w_0()
        .gap_1()
        .child(
            typography(crate::design::TypeRole::Body)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(crate::theme::color(INK))
                .child(label),
        )
        .child(control)
}

/// One aligned label/value row, shared by ordinary values and location fields.
pub fn settings_row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    let label = label.into();
    row()
        .relative()
        .child(crate::settings_anchor::probe(label.clone()))
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_start()
        .gap_3()
        .py_2()
        .border_b_1()
        .border_color(crate::theme::color(BORDER))
        .child(
            div()
                .flex_basis(px(180.))
                .flex_grow(0.7)
                .min_w_0()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(label),
        )
        .child(
            div()
                .flex_basis(px(280.))
                .flex_grow(1.)
                .max_w_full()
                .min_w_0()
                .child(control),
        )
}

/// Lightweight named outline for one wrapping multi-select set, not a card.
pub fn checkbox_group(label: impl Into<SharedString>, controls: impl IntoElement) -> Div {
    let label = label.into();
    column()
        .relative()
        .min_w_0()
        .mt_2()
        .p_3()
        .pt_4()
        .border_1()
        .border_color(crate::theme::color(BORDER))
        .rounded_sm()
        .child(
            div()
                .absolute()
                .top(px(-9.))
                .left(px(10.))
                .px_2()
                .bg(crate::theme::color(0xFFFFFF))
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(label.clone()),
        )
        .child(crate::settings_anchor::probe(label))
        .child(controls)
}

/// Location display and optional precision editor share one presentation.
/// Callers own the draft, expansion and picker; this component performs no IO.
#[derive(IntoElement)]
pub struct DirectoryField {
    id: SharedString,
    label: SharedString,
    path: SharedString,
    fallback: SharedString,
    expanded: bool,
    editor: Option<AnyElement>,
    actions: Option<AnyElement>,
}

impl DirectoryField {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        path: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            path: path.into(),
            fallback: "默认位置".into(),
            expanded: false,
            editor: None,
            actions: None,
        }
    }
    pub fn fallback(mut self, value: impl Into<SharedString>) -> Self {
        self.fallback = value.into();
        self
    }
    pub fn expanded(mut self, value: bool) -> Self {
        self.expanded = value;
        self
    }
    pub fn editor(mut self, value: impl IntoElement) -> Self {
        self.editor = Some(value.into_any_element());
        self
    }
    pub fn actions(mut self, value: impl IntoElement) -> Self {
        self.actions = Some(value.into_any_element());
        self
    }
}

impl RenderOnce for DirectoryField {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let summary: SharedString = self
            .path
            .rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .filter(|_| !self.path.trim().is_empty())
            .map(str::to_owned)
            .map(Into::into)
            .unwrap_or_else(|| self.fallback.clone());
        let absolute = if self.path.trim().is_empty() {
            self.fallback.clone()
        } else {
            self.path.clone()
        };
        column()
            .min_w_0()
            .gap_1()
            .child(settings_row(
                self.label,
                row()
                    .child(div().flex_1().truncate().child(summary))
                    .children(self.actions),
            ))
            .child(
                div()
                    .id(SharedString::from(format!("{}-path", self.id)))
                    .test_support()
                    .min_w_0()
                    .text_xs()
                    .text_color(crate::theme::color(MUTED))
                    .child(absolute.clone())
                    .tooltip(move |window, cx| Tooltip::new(absolute.clone()).build(window, cx)),
            )
            .children(Some({
                div().id(self.id).test_support().min_w_0().child(
                    self.editor
                        .unwrap_or_else(|| muted(self.path).into_any_element()),
                )
            }))
    }
}

pub fn input(state: &Entity<InputState>, disabled: bool) -> Input {
    Input::new(state)
        .small()
        // Input::h only affects multiline inputs; use the shared style minimum
        // so single-line fields keep a usable hit target at every theme size.
        .min_h(px(34.).max(
            crate::design::type_size(
                crate::theme::palette().font_size,
                crate::design::TypeRole::Body,
            ) * 1.55
                + px(10.),
        ))
        .disabled(disabled)
        .rounded(px(crate::design::CONTROL_RADIUS))
        .border_color(rgb(crate::theme::palette().border_strong))
        .bg(rgb(crate::theme::palette().inset))
}

pub fn binding_cell() -> Div {
    row()
        .min_w_0()
        .min_h(px(34.))
        .px_2()
        .py_1()
        .gap_1()
        .rounded(px(crate::design::CONTROL_RADIUS))
        .border_1()
        .border_color(crate::theme::color(BORDER))
        .bg(crate::theme::color(BG))
}

/// Display familiar keycap glyphs without changing stored/recorded key tokens.
pub fn keycap_label(key: &str) -> &str {
    match key {
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "ArrowUp" => "↑",
        "ArrowDown" => "↓",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Backquote" => "`",
        "Minus" => "−",
        "Equal" => "=",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Escape" => "Esc",
        "Enter" => "↵",
        "Backspace" => "⌫",
        "PageUp" => "PgUp",
        "PageDown" => "PgDn",
        "Delete" => "Del",
        "Insert" => "Ins",
        other => other,
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::input;
    use super::{DirectoryField, VirtualRows, virtual_rows};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, ListOffset,
        ParentElement, Render, ScrollDelta, ScrollWheelEvent, StyleRefinement, Styled,
        TestAppContext, VisualTestContext, WeakEntity, Window, div, point, px,
    };
    use gpui_kit::{TestSupportExt, component::input::InputState};
    use std::cell::RefCell;
    use std::{cell::Cell, collections::BTreeSet, rc::Rc};

    struct NumberEventFixture {
        source: Entity<InputState>,
        clock: Entity<InputState>,
        events: usize,
        clock_events: usize,
        inherited: Option<u64>,
    }
    impl Render for NumberEventFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut control = super::NumericSetting::new(
                "semantic-number",
                &self.source,
                "分钟",
                if self.inherited.is_some() { 0 } else { 5 },
                1,
                10,
                false,
            );
            if let Some(value) = self.inherited {
                control =
                    control.inherit_zero(value, format!("继承上级 · 当前有效值 {value} 分钟"));
            }
            super::column()
                .w(px(600.))
                .child(control)
                .child(crate::settings_time::time_input(
                    "semantic-clock",
                    &self.clock,
                    0,
                    false,
                    cx,
                ))
        }
    }
    #[gpui_kit::test]
    fn step_reset_and_time_buttons_emit_real_changes_without_coercing_invalid_text(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|window, cx| {
            let source = cx.new(|cx| InputState::new(window, cx).default_value("3"));
            let clock = cx.new(|cx| InputState::new(window, cx).default_value("23:59"));
            cx.subscribe(
                &source,
                |this: &mut NumberEventFixture,
                 _,
                 event: &gpui_kit::component::input::InputEvent,
                 cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        this.events += 1;
                        cx.notify();
                    }
                },
            )
            .detach();
            cx.subscribe(
                &clock,
                |this: &mut NumberEventFixture,
                 _,
                 event: &gpui_kit::component::input::InputEvent,
                 cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        this.clock_events += 1;
                        cx.notify();
                    }
                },
            )
            .detach();
            NumberEventFixture {
                source,
                clock,
                events: 0,
                clock_events: 0,
                inherited: None,
            }
        });
        draw(visual);
        visual.update(|window, cx| window.click("semantic-number-increase", cx));
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).source.read(cx).value(), "4");
            assert_eq!(view.read(cx).events, 1);
            window.click("semantic-number-reset", cx);
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).source.read(cx).value(), "5");
            assert_eq!(view.read(cx).events, 2);
            let source = view.read(cx).source.clone();
            source.update(cx, |input, cx| input.set_value("-", window, cx));
        });
        draw(visual);
        visual.update(|window, cx| {
            window.click("semantic-number-increase", cx);
            assert_eq!(view.read(cx).source.read(cx).value(), "-");
            assert_eq!(view.read(cx).events, 2);
            window.click("semantic-clock-minute-up", cx);
        });
        draw(visual);
        visual.update(|_, cx| {
            assert_eq!(view.read(cx).clock.read(cx).value(), "00:00");
            assert_eq!(view.read(cx).clock_events, 1);
        });
    }

    #[gpui_kit::test]
    fn inherited_numbers_only_materialize_on_customization_and_restore_the_sentinel(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|window, cx| {
            let source = cx.new(|cx| InputState::new(window, cx).default_value("0"));
            let clock = cx.new(|cx| InputState::new(window, cx).default_value("00:00"));
            cx.subscribe(
                &source,
                |this: &mut NumberEventFixture,
                 _,
                 event: &gpui_kit::component::input::InputEvent,
                 cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        this.events += 1;
                        cx.notify();
                    }
                },
            )
            .detach();
            NumberEventFixture {
                source,
                clock,
                events: 0,
                clock_events: 0,
                inherited: Some(3),
            }
        });
        draw(visual);
        view.update(visual, |this, cx| {
            this.inherited = Some(7);
            cx.notify();
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).source.read(cx).value(), "0");
            assert_eq!(view.read(cx).events, 0);
            assert!(window.try_find("semantic-number-increase").is_none());
            window.click("semantic-number-customize", cx);
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).source.read(cx).value(), "7");
            assert_eq!(view.read(cx).events, 1);
            window.click("semantic-number-reset", cx);
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).source.read(cx).value(), "0");
            assert_eq!(view.read(cx).events, 2);
            assert!(window.try_find("semantic-number-customize").is_some());
        });
    }

    struct ChoiceFixture {
        value: usize,
        changes: usize,
        width: f32,
    }
    impl Render for ChoiceFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            super::column()
                .w(px(self.width))
                .child(super::choice_with_default(
                    "responsive-choice",
                    (self.value, 0usize),
                    vec![(0, "默认".into()), (1, "浅色".into()), (2, "深色".into())],
                    false,
                    cx.weak_entity(),
                    |this, value, _, cx| {
                        this.value = value;
                        this.changes += 1;
                        cx.notify();
                    },
                ))
        }
    }
    #[gpui_kit::test]
    fn responsive_choices_keep_value_across_layouts_and_reset_through_real_input(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, _| ChoiceFixture {
            value: 0,
            changes: 0,
            width: 600.,
        });
        draw(visual);
        visual.update(|window, cx| {
            assert!(
                window
                    .try_find((gpui_kit::ElementId::from("responsive-choice"), "option-2"))
                    .is_some()
            );
            super::select_choice(window, "responsive-choice", 2, cx);
            assert_eq!(view.read(cx).value, 2);
            assert_eq!(view.read(cx).changes, 1);
        });
        view.update(visual, |this, cx| {
            this.width = 150.;
            cx.notify();
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(view.read(cx).value, 2);
            assert_eq!(view.read(cx).changes, 1);
            assert!(
                window
                    .try_find((gpui_kit::ElementId::from("responsive-choice"), "option-2"))
                    .is_none()
            );
            super::select_choice(window, "responsive-choice", 1, cx);
            assert_eq!(view.read(cx).value, 1);
            assert_eq!(view.read(cx).changes, 2);
            window.click(
                (gpui_kit::ElementId::from("responsive-choice"), "reset"),
                cx,
            );
        });
        draw(visual);
        visual.update(|_, cx| {
            assert_eq!(view.read(cx).value, 0);
            assert_eq!(view.read(cx).changes, 3);
        });
    }

    struct DirectoryFixture {
        expanded: bool,
    }

    #[gpui_kit::test]
    fn single_line_setting_input_uses_the_shared_control_height(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        struct InputFixture(Entity<InputState>);
        impl Render for InputFixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                super::column().w(px(200.)).h(px(200.)).child(
                    div()
                        .id("setting-input-bounds")
                        .test_support()
                        .flex_shrink_0()
                        .child(input(&self.0, false)),
                )
            }
        }
        let (_, cx) = cx.add_window_view(|window, cx| {
            InputFixture(cx.new(|cx| InputState::new(window, cx).default_value("60")))
        });
        draw(cx);
        cx.update(|window, _| {
            assert_eq!(
                window.find("setting-input-bounds").bounds().size.height,
                px(34.)
            );
        });
    }
    impl Render for DirectoryFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(600.)).child(
                DirectoryField::new("directory-detail", "图片位置", "C:/private/library/images")
                    .expanded(self.expanded),
            )
        }
    }

    #[gpui_kit::test]
    fn directory_field_keeps_the_precision_editor_visible_regardless_of_legacy_expansion(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, _| DirectoryFixture { expanded: false });
        draw(cx);
        cx.update(|window, _| {
            assert!(
                window.try_find("directory-detail").is_some(),
                "precision editor must remain visible"
            )
        });
        cx.update(|window, _| assert!(window.try_find("directory-detail-path").is_some()));
        view.update(cx, |view, cx| {
            view.expanded = true;
            cx.notify();
        });
        draw(cx);
        cx.update(|window, _| assert!(window.try_find("directory-detail").is_some()));
    }

    struct FormFixture {
        rows: VirtualRows<u64>,
        ids: Vec<usize>,
        revision: u64,
        tall: Rc<Cell<bool>>,
        rendered: Rc<RefCell<BTreeSet<usize>>>,
        width: f32,
        focus: FocusHandle,
        recording: bool,
    }

    impl Render for FormFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.rows.sync(&self.ids, self.revision);
            self.rows.track_focus(
                self.recording
                    .then(|| {
                        self.ids
                            .iter()
                            .position(|&id| id == 30)
                            .map(|row| (row, self.focus.clone()))
                    })
                    .flatten(),
            );
            let ids = self.ids.clone();
            let rendered = self.rendered.clone();
            let tall = self.tall.clone();
            let focus = self.focus.clone();
            div()
                .flex()
                .flex_col()
                .w(px(self.width))
                .h(px(300.))
                .child(virtual_rows(&self.rows.scroll, move |ix, _, _| {
                    let id = ids[ix];
                    rendered.borrow_mut().insert(id);
                    let mut row = div()
                        .h(px(if id == 30 && tall.get() { 140. } else { 58. }))
                        .child(format!("Shortcut {id}"));
                    if id == 30 {
                        row = row.track_focus(&focus);
                    }
                    row.into_any_element()
                }))
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            _ = window.draw(cx);
        });
    }

    #[gpui_kit::test]
    fn settings_rows_bound_render_work_and_preserve_variable_height_scroll(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let rendered = Rc::new(RefCell::new(BTreeSet::new()));
        let tall = Rc::new(Cell::new(false));
        let (view, cx) = cx.add_window_view(|_, cx| FormFixture {
            rows: VirtualRows::default(),
            ids: (0..50).collect(),
            revision: 0,
            tall: tall.clone(),
            rendered: rendered.clone(),
            width: 500.,
            focus: cx.focus_handle(),
            recording: false,
        });
        draw(cx);
        assert!(
            rendered.borrow().len() < 12,
            "offscreen rows were constructed"
        );
        assert!(!rendered.borrow().contains(&49));
        view.read_with(cx, |view, _| {
            assert!(
                view.rows.scroll.max_offset_for_scrollbar().y > px(2000.),
                "initial layout discarded offscreen row height estimates"
            )
        });

        rendered.borrow_mut().clear();
        view.update(cx, |view, cx| {
            view.width = 420.;
            cx.notify();
        });
        draw(cx);
        assert!(rendered.borrow().len() < 12);
        view.read_with(cx, |view, _| {
            assert!(
                view.rows.scroll.max_offset_for_scrollbar().y > px(2000.),
                "resize discarded offscreen row height estimates"
            )
        });

        view.update(cx, |view, cx| {
            view.rows.scroll.scroll_to(ListOffset {
                item_ix: 30,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        rendered.borrow_mut().clear();
        draw(cx);
        assert!(rendered.borrow().contains(&30));
        assert!(!rendered.borrow().contains(&0));

        // Inline recording/conflict feedback changes a row's height. Invalidate
        // measurements while retaining the same logical row under the viewport.
        tall.set(true);
        view.update(cx, |view, cx| {
            view.revision += 1;
            cx.notify();
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.rows.scroll.logical_scroll_top().item_ix, 30);
            let row = view.rows.scroll.bounds_for_item(30).unwrap();
            let next = view.rows.scroll.bounds_for_item(31).unwrap();
            assert_eq!(row.size.height, px(140.));
            assert!(
                next.top() >= row.bottom(),
                "expanded row overlaps its successor"
            );
            assert!(view.rows.scroll.max_offset_for_scrollbar().y > px(2000.));
        });

        view.update(cx, |view, cx| {
            view.recording = true;
            cx.notify();
        });
        cx.update(|window, cx| {
            let focus = view.read(cx).focus.clone();
            focus.focus(window, cx);
        });
        draw(cx);
        rendered.borrow_mut().clear();
        // Dispatch a real GPUI wheel over the list padding, as native smoke
        // does, and retain only the active offscreen recorder in addition to
        // the newly visible rows.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(5.), px(150.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-700.))),
            ..Default::default()
        });
        draw(cx);
        assert!(
            rendered.borrow().contains(&30),
            "offscreen recorder lost keyboard dispatch"
        );
        assert!(rendered.borrow().iter().any(|&id| id > 35));
        assert!(rendered.borrow().len() < 14);

        let extent = view.read_with(cx, |view, _| view.rows.scroll.max_offset_for_scrollbar());
        view.update(cx, |view, cx| {
            view.recording = false;
            view.revision += 1;
            cx.notify();
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.rows.scroll.max_offset_for_scrollbar(),
                extent,
                "canceling an offscreen recorder discarded its unchanged height"
            )
        });

        // Filtering replaces row identities and starts at the first result.
        rendered.borrow_mut().clear();
        view.update(cx, |view, cx| {
            view.ids = vec![49];
            cx.notify();
        });
        draw(cx);
        assert_eq!(*rendered.borrow(), BTreeSet::from([49]));
        view.read_with(cx, |view, _| {
            assert_eq!(view.rows.scroll.logical_scroll_top().item_ix, 0)
        });
    }

    struct CachedShell {
        content: Entity<CachedContent>,
        source: usize,
        width: f32,
    }

    struct CachedContent {
        owner: WeakEntity<CachedShell>,
        source: Option<usize>,
        renders: Rc<Cell<u64>>,
    }

    impl Render for CachedShell {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.content.update(cx, |content, cx| {
                if content.source != Some(self.source) {
                    content.source = Some(self.source);
                    cx.notify();
                }
            });
            div().w(px(self.width)).h(px(300.)).child(
                self.content
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }

    impl Render for CachedContent {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            // Match AppearanceView: constructing the content through its owner
            // must not make every future owner notification dirty this cache.
            self.owner
                .update(cx, |owner, _| {
                    div().size_full().child(format!("{}", owner.source))
                })
                .unwrap()
        }
    }

    fn draw_cached(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn settings_cached_view_reuses_parent_notifications_but_updates_source_and_bounds(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let renders = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let owner = cx.weak_entity();
            let content = cx.new(|_| CachedContent {
                owner,
                source: None,
                renders: renders.clone(),
            });
            CachedShell {
                content,
                source: 0,
                width: 500.,
            }
        });
        draw_cached(cx);
        let baseline = renders.get();
        assert!(baseline > 0);
        for _ in 0..20 {
            view.update(cx, |_, cx| cx.notify());
            draw_cached(cx);
        }
        assert_eq!(
            renders.get(),
            baseline,
            "unchanged shell notification rebuilt cached content"
        );
        view.update(cx, |view, cx| {
            view.source = 1;
            cx.notify();
        });
        draw_cached(cx);
        assert!(
            renders.get() > baseline,
            "changed source did not invalidate content"
        );
        let before_resize = renders.get();
        view.update(cx, |view, cx| {
            view.width = 420.;
            cx.notify();
        });
        draw_cached(cx);
        assert!(
            renders.get() > before_resize,
            "new viewport bounds reused obsolete layout"
        );
    }
}

/// Compact enumerated preference control; only the selected value stays on the page.
pub fn choice<T: 'static, V: Clone + PartialEq + 'static>(
    id: impl Into<ElementId>,
    current: V,
    options: Vec<(V, String)>,
    disabled: bool,
    owner: WeakEntity<T>,
    change: impl Fn(&mut T, V, &mut Window, &mut Context<T>) + Clone + 'static,
) -> impl IntoElement {
    use gpui_kit::component::{Disableable, Selectable};
    let id = id.into();
    let short = (2..=3).contains(&options.len())
        && options.iter().all(|(_, label)| label.chars().count() <= 8);
    column().w_full().min_w_0().child(
        container_query(move |available, _, _| {
            let needed = options
                .iter()
                .map(|(_, label)| label.chars().count() as f32 * 14. + 28.)
                .sum::<f32>();
            if short && f32::from(available.width) >= needed {
                return row()
                    .id(id.clone())
                    .test_support()
                    .min_w_0()
                    .gap_1()
                    .children(options.iter().enumerate().map(|(index, (value, label))| {
                        let owner = owner.clone();
                        let change = change.clone();
                        let value = value.clone();
                        crate::controls::Button::new((id.clone(), format!("option-{index}")))
                            .small()
                            .label(label.clone())
                            .selected(value == current)
                            .disabled(disabled)
                            .on_click(move |_, window, cx| {
                                let _ = owner
                                    .update(cx, |this, cx| change(this, value.clone(), window, cx));
                            })
                    }))
                    .into_any_element();
            }
            let label = options
                .iter()
                .find(|(value, _)| *value == current)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| "选择…".into());
            let options = options.clone();
            let current = current.clone();
            let owner = owner.clone();
            let change = change.clone();
            crate::controls::Button::new(id.clone())
                .label(label)
                .disabled(disabled)
                .dropdown_menu(move |mut menu, _, _| {
                    for (value, label) in &options {
                        let owner = owner.clone();
                        let change = change.clone();
                        let value = value.clone();
                        let checked = value == current;
                        menu =
                            menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                                move |_, window, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        change(this, value.clone(), window, cx)
                                    });
                                },
                            ));
                    }
                    menu
                })
                .into_any_element()
        })
        .h(px(40.)),
    )
}

/// Runner-only input driver shared by native smoke and portable GPUI fixtures.
#[cfg(feature = "ui-test")]
pub fn select_choice(window: &mut Window, id: &'static str, index: usize, cx: &mut App) {
    use gpui_kit::test::TestWindowExt;
    let option = (ElementId::from(id), format!("option-{index}"));
    window.render_frame(cx);
    if window.try_find(option.clone()).is_some() {
        window.click(option, cx);
    } else {
        window.click(id, cx);
        window.render_frame(cx);
        for _ in 0..=index {
            window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
        }
        window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
    }
    window.render_frame(cx);
}

/// Ordinary enums reset their own draft value; inherited enums use `choice`
/// with a None option instead of inventing a default override.
pub fn choice_with_default<T: 'static, V: Clone + PartialEq + 'static>(
    id: impl Into<ElementId>,
    values: (V, V),
    options: Vec<(V, String)>,
    disabled: bool,
    owner: WeakEntity<T>,
    change: impl Fn(&mut T, V, &mut Window, &mut Context<T>) + Clone + 'static,
) -> impl IntoElement {
    use gpui_kit::component::{Disableable, button::ButtonVariants};
    let id = id.into();
    let (current, default) = values;
    let reset_disabled = disabled || current == default;
    row()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .gap_2()
        .child(div().flex_1().min_w_0().child(choice(
            id.clone(),
            current,
            options,
            disabled,
            owner.clone(),
            change.clone(),
        )))
        .child(
            crate::controls::Button::new((id, "reset"))
                .small()
                .ghost()
                .label("恢复默认")
                .disabled(reset_disabled)
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| change(this, default.clone(), window, cx));
                }),
        )
}

/// An unset enum remains live inheritance until the user explicitly customizes.
pub fn inherited_choice<T: 'static, V: Clone + PartialEq + 'static>(
    id: impl Into<ElementId>,
    current: Option<V>,
    effective: (V, String),
    options: Vec<(V, String)>,
    disabled: bool,
    owner: WeakEntity<T>,
    change: impl Fn(&mut T, Option<V>, &mut Window, &mut Context<T>) + Clone + 'static,
) -> impl IntoElement {
    use gpui_kit::component::{Disableable, button::ButtonVariants};
    let id = id.into();
    if let Some(current) = current {
        let choose = change.clone();
        row()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap_2()
            .child(div().flex_1().min_w_0().child(choice(
                id.clone(),
                current,
                options,
                disabled,
                owner.clone(),
                move |this, value, window, cx| choose(this, Some(value), window, cx),
            )))
            .child(
                crate::controls::Button::new((id, "inherit"))
                    .small()
                    .ghost()
                    .label("恢复继承")
                    .disabled(disabled)
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| change(this, None, window, cx));
                    }),
            )
    } else {
        row()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap_2()
            .child(muted(effective.1))
            .child(
                crate::controls::Button::new((id, "customize"))
                    .small()
                    .ghost()
                    .label("自定义")
                    .disabled(disabled)
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            change(this, Some(effective.0.clone()), window, cx)
                        });
                    }),
            )
    }
}

/// Caller-owned numeric draft text is validated on global Save, including
/// invalid interim input; it never silently falls back to an older value.
#[derive(IntoElement)]
pub struct NumericSetting {
    id: SharedString,
    input: Entity<InputState>,
    unit: SharedString,
    default: u64,
    min: u64,
    max: u64,
    disabled: bool,
    zero_inheritance: Option<(u64, SharedString)>,
}
impl NumericSetting {
    pub fn new(
        id: impl Into<SharedString>,
        input: &Entity<InputState>,
        unit: impl Into<SharedString>,
        default: u64,
        min: u64,
        max: u64,
        disabled: bool,
    ) -> Self {
        Self {
            id: id.into(),
            input: input.clone(),
            unit: unit.into(),
            default,
            min,
            max,
            disabled,
            zero_inheritance: None,
        }
    }
    /// Zero remains an inheritance sentinel, not the lower bound of a wider
    /// numeric range. Customizing is an explicit edit; rendering never freezes
    /// the effective inherited value into the caller's source.
    pub fn inherit_zero(mut self, custom: u64, description: impl Into<SharedString>) -> Self {
        self.zero_inheritance = Some((custom.clamp(self.min, self.max), description.into()));
        self
    }
}
fn numeric_valid(raw: &str, min: u64, max: u64, inherit_zero: bool) -> bool {
    raw.trim()
        .parse::<u64>()
        .is_ok_and(|value| (min..=max).contains(&value) || (inherit_zero && value == 0))
}
fn step_integer(raw: &str, increase: bool, min: u64, max: u64) -> Option<u64> {
    let value = raw
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| (min..=max).contains(value))?;
    if increase {
        value.checked_add(1).filter(|next| *next <= max)
    } else {
        value.checked_sub(1).filter(|next| *next >= min)
    }
}
impl RenderOnce for NumericSetting {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        use gpui_kit::component::{Disableable, button::ButtonVariants};
        let raw = self.input.read(cx).value();
        let invalid = !numeric_valid(&raw, self.min, self.max, self.zero_inheritance.is_some());
        if raw.trim().parse::<u64>() == Ok(0)
            && let Some((custom, description)) = &self.zero_inheritance
        {
            let custom = *custom;
            let source = self.input.clone();
            return column().id(self.id.clone()).test_support().gap_1().child(
                row()
                    .flex_wrap()
                    .gap_2()
                    .child(muted(description.clone()))
                    .child(
                        crate::controls::Button::new(SharedString::from(format!(
                            "{}-customize",
                            self.id
                        )))
                        .small()
                        .ghost()
                        .label("自定义")
                        .disabled(self.disabled)
                        .on_click(move |_, window, cx| {
                            source.update(cx, |input, cx| {
                                input.set_value(custom.to_string(), window, cx);
                                cx.emit(gpui_kit::component::input::InputEvent::Change);
                            });
                        }),
                    ),
            );
        }
        let (min, max) = (self.min, self.max);
        let mut number = row().gap_1().min_w_0();
        for (suffix, label, increase) in [("decrease", "−", false), ("increase", "+", true)] {
            if increase {
                number = number.child(div().w(px(112.)).child(input(&self.input, self.disabled)));
            }
            let source = self.input.clone();
            number = number.child(
                crate::controls::Button::new(SharedString::from(format!("{}-{suffix}", self.id)))
                    .small()
                    .ghost()
                    .label(label)
                    .disabled(self.disabled || step_integer(&raw, increase, min, max).is_none())
                    .on_click(move |_, window, cx| {
                        let next = step_integer(&source.read(cx).value(), increase, min, max);
                        if let Some(next) = next {
                            source.update(cx, |input, cx| {
                                input.set_value(next.to_string(), window, cx);
                                cx.emit(gpui_kit::component::input::InputEvent::Change);
                            });
                        }
                    }),
            );
        }
        let source = self.input.clone();
        let default = self.default;
        column()
            .id(self.id.clone())
            .test_support()
            .gap_1()
            .child(
                row()
                    .flex_wrap()
                    .gap_2()
                    .child(number)
                    .child(muted(self.unit))
                    .child(
                        crate::controls::Button::new(SharedString::from(format!(
                            "{}-reset",
                            self.id
                        )))
                        .small()
                        .ghost()
                        .label(if self.zero_inheritance.is_some() {
                            "恢复继承"
                        } else {
                            "重置"
                        })
                        .disabled(self.disabled || raw.trim() == default.to_string())
                        .on_click(move |_, window, cx| {
                            source.update(cx, |input, cx| {
                                input.set_value(default.to_string(), window, cx);
                                // Explicit user edits must notify semantic subscribers.
                                cx.emit(gpui_kit::component::input::InputEvent::Change);
                            });
                        }),
                    ),
            )
            .when(invalid, |view| {
                view.child(muted(format!("请输入 {}–{} 的整数", self.min, self.max)))
            })
    }
}
#[cfg(test)]
mod integer_tests {
    #[test]
    fn inheritance_sentinel_does_not_expand_the_custom_range() {
        use super::numeric_valid;
        assert!(numeric_valid("0", 12, 28, true));
        assert!(!numeric_valid("0", 12, 28, false));
        for value in ["12", "28", " 14 "] {
            assert!(numeric_valid(value, 12, 28, true));
        }
        for value in ["1", "11", "29", "-", "", "1.5", "abc"] {
            assert!(!numeric_valid(value, 12, 28, true));
        }
    }
    #[test]
    fn numeric_steps_are_exact_and_never_coerce_invalid_text_or_overflow() {
        use super::step_integer;
        assert_eq!(
            step_integer("9007199254740993", true, 1, u64::MAX),
            Some(9007199254740994)
        );
        assert_eq!(step_integer(&u64::MAX.to_string(), true, 0, u64::MAX), None);
        assert_eq!(step_integer("0", false, 0, 10), None);
        assert_eq!(step_integer("10", true, 1, 10), None);
        assert_eq!(step_integer("1", false, 1, 10), None);
        for raw in ["-", "", "1.5", "11", "abc"] {
            assert_eq!(step_integer(raw, true, 1, 10), None);
        }
    }
}
