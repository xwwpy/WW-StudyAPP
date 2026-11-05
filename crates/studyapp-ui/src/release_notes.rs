//! Version-separated notes inside one bounded scroll surface.
use crate::components::{column, heading, muted};
use gpui_kit::{prelude::FluentBuilder, *};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotesSection {
    pub version: SharedString,
    pub notes: SharedString,
}
impl ReleaseNotesSection {
    pub fn new(version: impl Into<SharedString>, notes: impl Into<SharedString>) -> Self {
        Self {
            version: version.into(),
            notes: notes.into(),
        }
    }
}
pub fn sections(id: &'static str, entries: &[ReleaseNotesSection]) -> Div {
    let palette = crate::theme::palette();
    column()
        .w_full()
        .min_w_0()
        .gap_3()
        .when(entries.is_empty(), |column| {
            column.child(muted("暂无新的版本说明。"))
        })
        .children(entries.iter().enumerate().map(|(index, entry)| {
            let key = format!("{id}-{index}");
            div()
                .id(SharedString::from(key.clone()))
                .test_support()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .min_w_0()
                .gap_2()
                .p_3()
                .rounded(px(crate::design::CONTROL_RADIUS))
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface))
                .child(heading(format!("v{}", entry.version)))
                .child(
                    div()
                        .id(SharedString::from(format!("{key}-body")))
                        .test_support()
                        .min_w_0()
                        .child(crate::markdown_view::copyable(
                            SharedString::from(format!("{key}-copy")),
                            crate::markdown_view::content(
                                SharedString::from(format!("{key}-markdown")),
                                &crate::markdown_content::release_notes(
                                    if entry.notes.trim().is_empty() {
                                        "本版本未提供更新说明。"
                                    } else {
                                        &entry.notes
                                    },
                                ),
                            ),
                        )),
                )
        }))
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{ReleaseNotesSection, sections};
    use gpui_kit::{
        AppContext, Context, IntoElement, Render, TestAppContext, Window, base::Root, px,
        test::TestWindowExt,
    };
    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            sections(
                "versions",
                &[
                    ReleaseNotesSection::new("0.3.0", "**Current target**"),
                    ReleaseNotesSection::new("0.2.6", "Intermediate changes"),
                ],
            )
        }
    }
    #[gpui_kit::test]
    fn versions_have_independent_regions_without_independent_scrollbars(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, visual) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| Fixture);
            Root::new(content, window, cx)
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            let target = window.find("versions-0").bounds();
            let intermediate = window.find("versions-1").bounds();
            assert!(target.size.height > px(0.));
            assert!(intermediate.top() >= target.bottom());
        });
    }
}
