//! Virtual drag-selection checks; no system clipboard or native input.
use crate::{
    feedback::{ModalFocus, modal_frame_shell, modal_shell},
    update_dialog::*,
};
use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, Modifiers, MouseButton,
    ParentElement, Render, StyleRefinement, Styled, TestAppContext, TestSupportExt, Window,
    base::{Root, TextSelection},
    component::text::{TextView, TextViewState},
    div, point, px,
    test::TestWindowExt,
};
struct CachedBackground {
    html: bool,
    state: Entity<TextViewState>,
}
impl Render for CachedBackground {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.html {
            return div()
                .size_full()
                .child(crate::markdown_view::summary(
                    "native-cached-background",
                    "<span>BackgroundOnly secret underneath</span>"
                        .repeat(60)
                        .as_str(),
                ))
                .into_any_element();
        }
        div()
            .size_full()
            .child(
                TextView::new(&self.state)
                    .selectable(true)
                    .scrollable(false),
            )
            .into_any_element()
    }
}
struct Fixture {
    focus: ModalFocus,
    open: bool,
    legacy: bool,
    html: bool,
    background: Entity<TextViewState>,
    cached: Entity<CachedBackground>,
    scroll: gpui_kit::ScrollHandle,
}
impl Render for Fixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.focus.sync(self.open, window, cx);
        div()
            .size_full()
            .child(
                self.focus.background(
                    div()
                        .id("background-notes")
                        .test_support()
                        .size_full()
                        .child(
                            self.cached
                                .clone()
                                .cached(StyleRefinement::default().size_full()),
                        ),
                ),
            )
            .children(self.open.then(|| {
                let card = UpdateDialog::new(
                    UpdateDialogContent::new(
                        "0.2.2",
                        "0.2.3",
                        "",
                        if self.html { "<div><b>ReleaseOnly</b><br><span style='font-size:24px'>ReleaseOnly updates</span><br>More release notes</div>" }
                        else { "## ReleaseOnly\n\nReleaseOnly updates\n\nMore release notes" },
                        UpdateDialogPhase::Ready,
                    ),
                    &self.scroll,
                );
                if self.legacy {
                    modal_shell(card, &self.focus, window, cx)
                } else {
                    modal_frame_shell(card, &self.focus, window, cx)
                }
            }))
    }
}
fn check_selection(cx: &mut TestAppContext, legacy: bool, html: bool) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(&Default::default(), false, cx);
    });
    let mut fixture = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let background = cx.new(|cx| {
            TextViewState::markdown(
                &"BackgroundOnly secret underneath

"
                .repeat(60),
                cx,
            )
        });
        let cached = cx.new(|_| CachedBackground {
            html,
            state: background.clone(),
        });
        let view = cx.new(|cx| Fixture {
            focus: ModalFocus::new(cx),
            open: false,
            legacy,
            html,
            background,
            cached,
            scroll: gpui_kit::ScrollHandle::new(),
        });
        fixture = Some(view.clone());
        Root::new(view, window, cx)
    });
    let fixture = fixture.unwrap();
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    fixture.update(visual, |view, cx| {
        view.open = true;
        cx.notify();
    });
    visual.run_until_parked();
    let (start, end) = visual.update(|window, cx| {
        window.render_frame(cx);
        let bounds = window.find("update-release-0-body").bounds();
        (
            bounds.origin + point(px(4.), px(12.)),
            point(
                bounds.right() + px(40.),
                window.viewport_size().height - px(10.),
            ),
        )
    });
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| window.render_frame(cx));
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        window.render_frame(cx);
        let selected = TextSelection::selected_text(window, cx);
        assert!(!selected.is_empty(), "release notes must remain selectable");
        assert!(selected.contains("ReleaseOnly"), "selected: {selected}");
        assert!(
            !selected.contains("BackgroundOnly"),
            "modal selected the underlying page: {selected}"
        );
        assert!(
            fixture
                .read(cx)
                .background
                .read(cx)
                .selected_text()
                .is_empty()
        );
    });
    fixture.update(visual, |view, cx| {
        view.open = false;
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            !TextSelection::has_selection(window, cx),
            "closing the modal must clear its selection"
        );
    });
    let start = point(px(1.), px(10.));
    let end = point(px(300.), px(10.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| window.render_frame(cx));
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            TextSelection::selected_text(window, cx).contains("BackgroundOnly"),
            "page selection must work after dismissal"
        );
    });
}
#[gpui_kit::test]
fn update_card_selection_never_includes_the_underlying_page(cx: &mut TestAppContext) {
    check_selection(cx, false, false);
}
#[gpui_kit::test]
fn legacy_modal_selection_is_also_isolated_and_restored(cx: &mut TestAppContext) {
    check_selection(cx, true, false);
}

#[gpui_kit::test]
fn native_html_modal_selection_excludes_cached_html_background(cx: &mut TestAppContext) {
    check_selection(cx, false, true);
}
#[gpui_kit::test]
fn native_html_legacy_modal_selection_excludes_cached_html_background(cx: &mut TestAppContext) {
    check_selection(cx, true, true);
}
