//! Selection-preserving, explicit copy for offline Markdown surfaces.
use crate::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::{base::TextSelection, component::text::TextView, *};

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

/// A mouse release outside the native child may never reach GPUI's selection
/// layer. End that gesture before its bubble-phase hover handler extends text.
pub fn selection_gesture_guard() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |_, _, window, _| {
            window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
                if phase.capture() && event.pressed_button != Some(MouseButton::Left) {
                    TextSelection::end_edge_drag(window, cx);
                    TextSelection::end(window, cx);
                }
            });
        },
    )
    .absolute()
    .size_full()
}

/// Shared offline renderer with per-surface, display-only code folding.
pub fn content(id: impl Into<ElementId>, source: &str) -> AnyElement {
    MarkdownContent {
        id: id.into(),
        source: source.to_owned(),
    }
    .into_any_element()
}

pub fn summary(id: impl Into<ElementId>, source: &str) -> impl IntoElement {
    let id = id.into();
    copyable(id.clone(), content(id, source))
}

#[derive(Clone, Debug, PartialEq)]
enum Part {
    Prose(String),
    Code {
        markdown: String,
        code: String,
        language: String,
    },
}
fn parts(source: &str) -> Vec<Part> {
    use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};
    let mut parts = Vec::new();
    let mut end = 0;
    let mut block = None;
    let mut code = String::new();
    for (event, range) in crate::markdown_parser::parser(source).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                if range.start > end {
                    parts.push(Part::Prose(source[end..range.start].into()));
                }
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").into()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                block = Some((range.start, language));
                code.clear();
            }
            Event::Text(text) if block.is_some() => code.push_str(&text),
            Event::End(TagEnd::CodeBlock) => {
                if let Some((start, language)) = block.take() {
                    end = range.end;
                    parts.push(Part::Code {
                        markdown: source[start..end].into(),
                        code: std::mem::take(&mut code),
                        language,
                    });
                }
            }
            _ => {}
        }
    }
    if end < source.len() {
        parts.push(Part::Prose(source[end..].into()));
    }
    parts
}
#[derive(Default)]
struct DisplayState {
    source: String,
    parts: Vec<Part>,
    collapsed: std::collections::HashSet<usize>,
}
#[derive(IntoElement)]
struct MarkdownContent {
    id: ElementId,
    source: String,
}
impl RenderOnce for MarkdownContent {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        use crate::components::{column, muted, row};
        use crate::controls::Button;
        use gpui_kit::component::{Sizable, button::ButtonVariants};
        use gpui_kit::prelude::FluentBuilder;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| DisplayState::default());
        if state.read(cx).source != self.source {
            state.update(cx, |state, _| {
                state.parts = parts(&self.source);
                state.source = self.source;
                state.collapsed.clear();
            });
        }
        let palette = crate::theme::palette();
        let layout = palette.markdown_layout;
        let mut view = column()
            .id(self.id)
            .w_full()
            .min_w_0()
            .text_color(rgba(palette.markdown_rgba[0].packed_rgba()))
            .when(layout.reading_width > 0, |view| {
                view.max_w(px(layout.reading_width as f32))
            });
        for (index, part) in state.read(cx).parts.clone().into_iter().enumerate() {
            match part {
                Part::Prose(source) => {
                    if let Some(content) =
                        crate::native_html::content(("summary-prose", index), &source)
                    {
                        view = view.child(content);
                    } else {
                        let safe = crate::markdown_content::markdown_preview_with_html(
                            &source,
                            palette.render_html,
                        );
                        view = view.child(
                            TextView::markdown(
                                ("summary-prose", index),
                                crate::markdown_parser::display_source(&safe).into_owned(),
                            )
                            .style(crate::compact_markdown::summary_markdown_style())
                            .on_link_click(|_, _, _, _| {})
                            .w_full()
                            .min_w_0()
                            .scrollable(false),
                        );
                    }
                }
                Part::Code {
                    markdown,
                    code,
                    language,
                } => {
                    let collapsed = state.read(cx).collapsed.contains(&index);
                    let toggle = state.clone();
                    let mut card = column()
                        .id(("summary-code", index))
                        .test_support()
                        .w_full()
                        .min_w_0()
                        .my_1()
                        .bg(rgba(palette.markdown_rgba[7].packed_rgba()))
                        .rounded(px(layout.code_radius as f32))
                        .when(layout.code_border, |view| {
                            view.border_1()
                                .border_color(rgba(palette.markdown_rgba[14].packed_rgba()))
                        })
                        .child(
                            row()
                                .min_w_0()
                                .px_2()
                                .py_1()
                                .child(muted(if language.is_empty() {
                                    "代码".into()
                                } else {
                                    language
                                }))
                                .child(div().flex_1())
                                .child(
                                    Button::new(("summary-code-copy", index))
                                        .ghost()
                                        .small()
                                        .label("复制")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                code.clone(),
                                            ))
                                        }),
                                )
                                .child(
                                    Button::new(("summary-code-toggle", index))
                                        .ghost()
                                        .small()
                                        .label(if collapsed { "展开" } else { "折叠" })
                                        .on_click(move |_, _, cx| {
                                            toggle.update(cx, |state, cx| {
                                                if !state.collapsed.remove(&index) {
                                                    state.collapsed.insert(index);
                                                }
                                                cx.notify();
                                            })
                                        }),
                                ),
                        );
                    if !collapsed {
                        card = card.child(
                            TextView::markdown(("summary-code-content", index), markdown)
                                .style(crate::compact_markdown::summary_markdown_style())
                                .w_full()
                                .min_w_0()
                                .scrollable(false),
                        );
                    }
                    view = view.child(card);
                }
            }
        }
        view
    }
}

#[cfg(test)]
mod code_parts_tests {
    use super::{Part, parts};
    #[test]
    fn code_copy_is_parser_text_and_never_rewrites_source() {
        let source = "before

~~~rust
let 中文 = 1;
~~~

after";
        let parsed = parts(source);
        assert!(
            matches!(&parsed[1], Part::Code{code,language,..} if code == "let 中文 = 1;
" && language == "rust")
        );
        let joined: String = parsed
            .iter()
            .map(|part| match part {
                Part::Prose(source) => source.as_str(),
                Part::Code { markdown, .. } => markdown.as_str(),
            })
            .collect();
        assert_eq!(joined, source);
    }
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

#[cfg(all(test, feature = "ui-test"))]
mod code_control_tests {
    use super::*;
    use gpui_kit::{base::Root, test::TestWindowExt};
    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(500.))
                .child(summary("code-summary", "```rust\nlet value = 1;\n```"))
                .child(selection_gesture_guard())
        }
    }
    #[gpui_kit::test]
    fn collapsed_summary_can_copy_full_code_without_changing_source(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let (_, visual) = cx.add_window_view(|window, cx| {
            let fixture = cx.new(|_| Fixture);
            Root::new(fixture, window, cx)
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            let expanded = window.find(("summary-code", 0)).bounds().size.height;
            window.click(("summary-code-toggle", 0), cx);
            window.render_frame(cx);
            assert!(window.find(("summary-code", 0)).bounds().size.height < expanded);
            window.click(("summary-code-copy", 0), cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("let value = 1;\n".into())
            );
            window.click(("summary-code-toggle", 0), cx);
            window.render_frame(cx);
            assert_eq!(
                window.find(("summary-code", 0)).bounds().size.height,
                expanded
            );
        });
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod selection_guard_tests {
    use super::*;
    use gpui_kit::{base::Root, test::TestWindowExt};
    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .p_4()
                .w(px(500.))
                .child(summary(
                    "guard-selection",
                    "Selectable words across a paragraph\n\nAnother paragraph below",
                ))
                .child(selection_gesture_guard())
        }
    }
    #[gpui_kit::test]
    fn hover_without_left_button_ends_a_lost_release_without_clearing_selection(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let (_, visual) = cx.add_window_view(|window, cx| {
            let fixture = cx.new(|_| Fixture);
            Root::new(fixture, window, cx)
        });
        visual.update(|window, cx| window.render_frame(cx));
        visual.simulate_mouse_down(
            point(px(18.), px(24.)),
            MouseButton::Left,
            Default::default(),
        );
        visual.simulate_mouse_move(
            point(px(120.), px(24.)),
            Some(MouseButton::Left),
            Default::default(),
        );
        visual.update(|window, cx| window.render_frame(cx));
        let before = visual.update(TextSelection::selected_text);
        assert!(!before.is_empty());
        // Intentionally omit mouseup: it occurred outside the native window.
        visual.simulate_mouse_move(point(px(300.), px(70.)), None, Default::default());
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(visual.update(TextSelection::selected_text), before);
    }
}
