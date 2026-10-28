//! Compact Markdown authoring with a native input and an offline preview.
use crate::{
    components::{BORDER, column, muted, row},
    controls::Button,
    markdown_content::markdown_preview,
    markdown_format::{MarkdownFormat, format_markdown},
};
use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::ButtonVariants,
    input::{Textarea, TextareaState},
    text::{TextView, TextViewStyle},
};
use gpui_kit::*;

pub fn summary_markdown_style() -> TextViewStyle {
    let mut table = StyleRefinement::default();
    table.overflow.x = Some(Overflow::Scroll);
    TextViewStyle::default().table(table)
}

pub struct CompactMarkdown {
    input: Entity<TextareaState>,
    preview: bool,
    preview_source: String,
    _input_observer: Subscription,
}

impl CompactMarkdown {
    pub fn new(input: Entity<TextareaState>, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(&input, |this, _, cx| {
            if this.preview {
                this.preview_source = markdown_preview(&this.input.read(cx).value());
                cx.notify();
            }
        });
        Self {
            input,
            preview: false,
            preview_source: String::new(),
            _input_observer: observer,
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.preview = false;
        self.preview_source.clear();
        cx.notify();
    }

    pub fn is_preview(&self) -> bool {
        self.preview
    }

    fn format(&mut self, kind: MarkdownFormat, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            if let Some(edit) = format_markdown(&input.value(), input.selected_range(), kind) {
                input.set_selected_range(edit.range, cx);
                input.replace(edit.replacement, window, cx);
                input.set_selected_range(edit.selection, cx);
                input.focus(window, cx);
            }
        });
    }
}

impl Render for CompactMarkdown {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = row().flex_wrap().gap_1().children(
            [
                ("summary-heading", "标题", MarkdownFormat::Heading),
                ("summary-bold", "粗体", MarkdownFormat::Bold),
                ("summary-italic", "斜体", MarkdownFormat::Italic),
                ("summary-list", "列表", MarkdownFormat::List),
                ("summary-quote", "引用", MarkdownFormat::Quote),
                ("summary-code", "代码", MarkdownFormat::Code),
                ("summary-table", "表格", MarkdownFormat::Table),
            ]
            .into_iter()
            .map(|(id, label, kind)| {
                Button::new(id)
                    .ghost()
                    .small()
                    .label(label)
                    .disabled(self.preview)
                    .on_click(cx.listener(move |this, _, window, cx| this.format(kind, window, cx)))
            }),
        );
        column()
            .id("summary-markdown")
            .test_support()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(
                row()
                    .flex_wrap()
                    .gap_2()
                    .child(muted("学习摘要 · Markdown"))
                    .child(div().flex_1())
                    .child(
                        Button::new("summary-edit")
                            .ghost()
                            .small()
                            .label("编辑")
                            .selected(!self.preview)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.preview = false;
                                this.input.update(cx, |input, cx| input.focus(window, cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("summary-preview")
                            .ghost()
                            .small()
                            .label("预览")
                            .selected(self.preview)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let source = this.input.read(cx).value();
                                this.preview_source = if source.trim().is_empty() {
                                    "写下学习内容后，在这里查看排版。".into()
                                } else {
                                    markdown_preview(&source)
                                };
                                this.preview = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(toolbar)
            .child(if self.preview {
                div()
                    .id("summary-markdown-preview")
                    .test_support()
                    .w_full()
                    .h(px(150.))
                    .min_w_0()
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(crate::theme::color(BORDER))
                    .rounded_md()
                    .p_3()
                    .child(
                        TextView::markdown("summary-markdown-content", self.preview_source.clone())
                            .style(summary_markdown_style())
                            .on_link_click(|_, _, _, _| {})
                            .scrollable(false),
                    )
                    .into_any_element()
            } else {
                Textarea::new(&self.input)
                    .h(px(150.))
                    .aria_label("学习摘要 Markdown 源码")
                    .into_any_element()
            })
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::CompactMarkdown;
    use gpui_kit::component::input::TextareaState;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, TestAppContext};

    #[gpui_kit::test]
    fn formatting_preserves_unicode_selection_undo_and_preview_source(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::apply(&Default::default(), false, cx);
        });
        let (editor, test_cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| TextareaState::new(window, cx));
            CompactMarkdown::new(input, cx)
        });
        test_cx.update(|window, cx| {
            let input = editor.read(cx).input.clone();
            input.update(cx, |input, cx| {
                input.set_value("前 中😀文 后", window, cx);
                input.set_selected_range(4..14, cx);
            });
            window.render_frame(cx);
            window.click("summary-bold", cx);
            assert_eq!(input.read(cx).value().as_str(), "前 **中😀文** 后");
            assert_eq!(input.read(cx).selected_value().as_str(), "中😀文");
            window.press("ctrl-z", cx);
            assert_eq!(input.read(cx).value().as_str(), "前 中😀文 后");
            window.press("ctrl-y", cx);
            assert_eq!(input.read(cx).value().as_str(), "前 **中😀文** 后");
            window.click("summary-preview", cx);
            assert!(editor.read(cx).is_preview());
            assert!(editor.read(cx).preview_source.contains("**中😀文**"));
            window.click("summary-edit", cx);
            assert_eq!(input.read(cx).value().as_str(), "前 **中😀文** 后");
            window.click("summary-preview", cx);
            editor.update(cx, |editor, cx| editor.reset(cx));
            assert!(!editor.read(cx).is_preview());
        });
    }
}
