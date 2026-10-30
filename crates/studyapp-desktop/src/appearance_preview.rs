//! An isolated Markdown surface for appearance drafts. It never owns a note.
use crate::{markdown_editor::MarkdownEditor, theme::ResolvedTheme};
use gpui_kit::*;

pub const PREVIEW_SAMPLE: &str = r#"# 让阅读更清晰

这是一份独立的主题样例。调整字体、字号、行高和阅读宽度，可以比较一整段文字的换行与留白。正文需要舒适的对比度，**重要结论**、*补充说明*和~~已撤回的内容~~也应清晰可辨。

[链接文字](#preview)与 `行内代码` 使用各自的样式。

## 引用与嵌套列表

> 从记录到理解，留出思考的空间。
> 引用背景和引用文字应始终容易区分。

1. 阅读并提出问题
   - 比较不同的解释
     - 保留证据与例子
2. 用自己的语言记录
   - [x] 完成第一轮整理
   - [ ] 安排下一次复习

## 表格

| 内容 | 状态 | 数量 |
| :--- | :---: | ---: |
| 概念与定义 | 已整理 | 12 |
| 例子与练习 | 复习中 | 8 |
| 下一步计划 | 待开始 | 3 |

## 代码与语法颜色

```rust
// 注释、关键字、类型、函数、字符串、数字与标点
struct StudyNote {
    title: String,
    minutes: u32,
}

fn summarize(note: &StudyNote) -> String {
    let target = 25;
    format!("{}: {} / {}", note.title, note.minutes, target)
}
```

```html
<article class="study-note" data-minutes="25">
  <h2>复习计划</h2>
  <p>理解，然后练习。</p>
</article>
```

---

### 阅读宽度

一段较长的文字可以帮助判断阅读宽度是否合适：过长的行会增加视线往返的距离，过短的行会让段落过于零碎。这里的排版、代码边框、表格网格和交替行背景都由正在编辑的主题决定。The same sample also includes English words and numbers 1234567890, so line wrapping and font proportions can be compared without opening or changing a personal note.
"#;

#[derive(Default)]
pub struct AppearancePreview {
    editor: Option<Entity<MarkdownEditor>>,
    applied_theme: Option<ResolvedTheme>,
    visible: bool,
    occlusions: crate::overlay_regions::OverlayRegions<studyapp_ui::bubble::OverlayShape>,
}

impl AppearancePreview {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn configure(
        &mut self,
        theme: ResolvedTheme,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_none() && !visible {
            return;
        }
        let editor = self.editor.get_or_insert_with(|| {
            cx.new(|cx| {
                let mut editor = MarkdownEditor::new(window, cx).without_outline();
                editor.set_value_with_readonly(PREVIEW_SAMPLE.into(), true, window, cx);
                editor.set_can_request_edit(false, cx);
                editor.set_overlay_shapes("preview-overlays", self.occlusions.all(), cx);
                editor.set_shortcuts(
                    crate::shortcuts::SHORTCUT_ACTIONS
                        .iter()
                        .map(|action| (action.id.to_owned(), String::new()))
                        .collect(),
                    cx,
                );
                editor
            })
        });
        if self.applied_theme != Some(theme) {
            editor.update(cx, |editor, cx| editor.set_theme(theme.editor_tokens(), cx));
            self.applied_theme = Some(theme);
        }
        if self.visible != visible {
            self.visible = visible;
            editor.update(cx, |editor, cx| editor.set_visible(visible, cx));
            cx.notify();
        }
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            self.visible = false;
            if let Some(editor) = &self.editor {
                editor.update(cx, |editor, cx| editor.set_visible(false, cx));
            }
            cx.notify();
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn close_native_host(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, _| editor.close_native_host());
        }
    }

    pub fn set_occlusions(&mut self, bounds: Vec<Bounds<Pixels>>, cx: &mut Context<Self>) {
        self.set_overlay_regions("notices", bounds, cx);
    }

    pub fn set_overlay_regions(
        &mut self,
        source: &'static str,
        bounds: Vec<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        self.set_overlay_shapes(
            source,
            bounds
                .into_iter()
                .map(studyapp_ui::bubble::OverlayShape::card)
                .collect(),
            cx,
        );
    }

    pub fn set_overlay_shapes(
        &mut self,
        source: &'static str,
        shapes: Vec<studyapp_ui::bubble::OverlayShape>,
        cx: &mut Context<Self>,
    ) {
        if self.occlusions.set(source, shapes)
            && let Some(editor) = &self.editor
        {
            let regions = self.occlusions.all();
            editor.update(cx, |editor, cx| {
                editor.set_overlay_shapes("preview-overlays", regions, cx)
            });
        }
    }

    #[cfg(feature = "ui-test")]
    pub fn editor(&self) -> Option<Entity<MarkdownEditor>> {
        self.editor.clone()
    }

    #[cfg(feature = "ui-test")]
    pub fn applied_theme(&self) -> Option<ResolvedTheme> {
        self.applied_theme
    }
}

impl Render for AppearancePreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("appearance-markdown-preview")
            .test_support()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .children(self.visible.then(|| self.editor.clone()).flatten())
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{AppearancePreview, PREVIEW_SAMPLE};
    use crate::markdown_editor::MarkdownEditor;
    use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, TestAppContext, Window, div};

    /// This fixture deliberately never mounts or draws either MarkdownEditor.
    /// Its test-platform Window only supplies the context required by the model;
    /// therefore MarkdownEditor::render/ensure_host cannot create a WebView.
    struct StateFixture {
        preview: Entity<AppearancePreview>,
        note: Entity<MarkdownEditor>,
    }

    impl Render for StateFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn fixture(window: &mut Window, cx: &mut Context<StateFixture>) -> StateFixture {
        let note = cx.new(|cx| {
            let mut note = MarkdownEditor::new(window, cx);
            note.set_value("用户笔记：保持原样".into(), window, cx);
            note
        });
        StateFixture {
            preview: cx.new(|_| AppearancePreview::new()),
            note,
        }
    }

    #[gpui_kit::test]
    fn picker_regions_never_replace_notifications_or_allocate_native_editor(
        cx: &mut TestAppContext,
    ) {
        let (fixture, cx) = cx.add_window_view(fixture);
        let preview = fixture.read_with(cx, |fixture, _| fixture.preview.clone());
        let notice = gpui_kit::Bounds::new(
            gpui_kit::point(gpui_kit::px(20.), gpui_kit::px(30.)),
            gpui_kit::size(gpui_kit::px(120.), gpui_kit::px(80.)),
        );
        let picker = gpui_kit::Bounds::new(
            gpui_kit::point(gpui_kit::px(200.), gpui_kit::px(50.)),
            gpui_kit::size(gpui_kit::px(296.), gpui_kit::px(400.)),
        );
        preview.update(cx, |preview, cx| {
            preview.set_occlusions(vec![notice], cx);
            preview.set_overlay_regions("appearance-color-picker", vec![picker], cx);
            assert_eq!(preview.occlusions.all().len(), 2);
            preview.set_occlusions(vec![], cx);
            assert_eq!(
                preview.occlusions.all(),
                vec![studyapp_ui::bubble::OverlayShape::card(picker)]
            );
            preview.set_occlusions(vec![notice], cx);
            preview.set_overlay_regions("appearance-color-picker", vec![], cx);
            assert_eq!(
                preview.occlusions.all(),
                vec![studyapp_ui::bubble::OverlayShape::card(notice)]
            );
            assert!(preview.editor.is_none());
        });
    }

    #[gpui_kit::test]
    fn hidden_preview_does_not_allocate_an_editor(cx: &mut TestAppContext) {
        let (fixture, cx) = cx.add_window_view(fixture);
        cx.update(|window, cx| {
            let preview = fixture.read(cx).preview.clone();
            let theme = crate::theme::resolve(&Default::default(), false);
            preview.update(cx, |preview, cx| {
                preview.configure(theme, false, window, cx);
                preview.hide(cx);
                assert!(preview.editor.is_none());
                assert!(!preview.visible);
                assert!(preview.applied_theme.is_none());
            });
        });
    }

    #[gpui_kit::test]
    fn preview_reuses_one_readonly_sample_across_themes_and_visibility(cx: &mut TestAppContext) {
        let (fixture, cx) = cx.add_window_view(fixture);
        let light = crate::theme::resolve(&Default::default(), false);
        let dark = crate::theme::resolve(&Default::default(), true);
        let (preview, note) = fixture.read_with(cx, |fixture, _| {
            (fixture.preview.clone(), fixture.note.clone())
        });
        let note_generation = note.read_with(cx, |note, _| note.generation());
        let editor = cx.update(|window, cx| {
            preview.update(cx, |preview, cx| {
                preview.configure(light, true, window, cx);
                preview
                    .editor
                    .clone()
                    .expect("visible preview must create its independent editor")
            })
        });
        let id = editor.entity_id();
        let generation = editor.read_with(cx, |editor, _| {
            assert_eq!(editor.value(), PREVIEW_SAMPLE);
            assert!(!editor.outline_enabled());
            assert!(editor.readonly());
            assert!(!editor.creating());
            assert!(!editor.ready());
            editor.generation()
        });
        for (theme, visible) in [
            (light, true),
            (dark, true),
            (light, false),
            (dark, false),
            (dark, true),
        ] {
            cx.update(|window, cx| {
                preview.update(cx, |preview, cx| {
                    preview.configure(theme, visible, window, cx);
                    assert_eq!(preview.visible, visible);
                    assert_eq!(preview.editor.as_ref().unwrap().entity_id(), id);
                    assert_eq!(preview.applied_theme, Some(theme));
                })
            });
            editor.read_with(cx, |editor, _| {
                assert_eq!(editor.value(), PREVIEW_SAMPLE);
                assert_eq!(
                    editor.generation(),
                    generation,
                    "theme changes must not reload or reset the sample"
                );
                assert!(editor.readonly());
                assert!(
                    !editor.creating(),
                    "state tests must never request a native host"
                );
            });
        }
        preview.update(cx, |preview, cx| {
            preview.hide(cx);
            assert!(!preview.visible);
            assert_eq!(preview.editor.as_ref().unwrap().entity_id(), id);
        });
        note.read_with(cx, |note, _| {
            assert_eq!(note.value(), "用户笔记：保持原样");
            assert_eq!(note.generation(), note_generation);
            assert_ne!(note.generation(), 0);
        });
        assert_ne!(note.entity_id(), id);
    }
}
