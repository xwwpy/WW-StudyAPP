//! Selection-preserving, explicit copy for offline Markdown surfaces.
use gpui_kit::{
    base::TextSelection,
    component::{
        menu::{ContextMenuExt, PopupMenuItem},
        text::TextView,
    },
    *,
};

pub fn copyable(id: impl Into<ElementId>, content: impl IntoElement) -> impl IntoElement {
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .child(content)
        .context_menu(|menu, window, cx| {
            // Capture before the popup takes focus; never copy the whole document or
            // text from the inactive background of a blocking dialog.
            let selected = TextSelection::selected_text(window, cx);
            let empty = selected.is_empty();
            menu.item(
                PopupMenuItem::new("复制")
                    .disabled(empty)
                    .on_click(move |_, _, cx| {
                        if !selected.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(selected.clone()));
                        }
                    }),
            )
        })
}

/// Shared offline renderer. HTML has no resource or application callbacks.
pub fn content(id: impl Into<ElementId>, source: &str) -> AnyElement {
    let id = id.into();
    if let Some(content) = crate::native_html::content(id.clone(), source) {
        return content;
    }
    let safe = crate::markdown_content::markdown_preview_with_html(
        source,
        crate::theme::palette().render_html,
    );
    TextView::markdown(
        id,
        crate::markdown_parser::display_source(&safe).into_owned(),
    )
    .style(crate::compact_markdown::summary_markdown_style())
    .on_link_click(|_, _, _, _| {})
    .w_full()
    .min_w_0()
    .scrollable(false)
    .into_any_element()
}

pub fn summary(id: impl Into<ElementId>, source: &str) -> impl IntoElement {
    copyable(id, content("summary-content", source))
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::summary;
    use gpui_kit::{
        AppContext, Context, IntoElement, MouseButton, ParentElement, Render, Styled,
        TestAppContext, Window, base::TextSelection, div, point, px,
    };
    use gpui_kit::{base::Root, test::TestWindowExt};
    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().p_4().w(px(380.)).child(summary(
                "copy-summary",
                "**Selectable words**\n\n第二段摘要",
            ))
        }
    }
    #[gpui_kit::test]
    fn right_click_preserves_selected_markdown_and_copies_in_virtual_clipboard(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Fixture);
            Root::new(view, window, cx)
        });
        visual.update(|window, cx| window.render_frame(cx));
        let start = point(px(18.), px(24.));
        let end = point(px(148.), px(24.));
        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.update(|window, cx| window.render_frame(cx));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        let selected = visual.update(TextSelection::selected_text);
        assert!(!selected.trim().is_empty());
        visual.simulate_mouse_down(end, MouseButton::Right, Default::default());
        visual.simulate_mouse_up(end, MouseButton::Right, Default::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find("popup-menu").bounds().size.height > px(0.));
            window.press("down", cx);
            window.press("enter", cx);
        });
        visual.run_until_parked();
        visual.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(selected)
            );
        });
    }
}
