//! Exercises the real chart viewport and its scrollbar on the isolated CI window.
use crate::app::{Page, StudyApp};
use gpui_kit::{test::TestWindowExt, *};
use std::path::Path;

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Stats;
                app.page_scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();

    for days in [7_usize, 30, 90, 7] {
        window
            .update(cx, |_, window, cx| {
                window.click(("stats-days", days), cx);
                window.render_frame(cx);
                let (page, scroll) = {
                    let app = view.read(cx);
                    (app.page_scroll.clone(), app.statistics_scroll.clone())
                };
                // Bring the complete chart track inside its parent clip before
                // pointer input; compact CI desktops may clip the initial chart.
                let overflow =
                    (scroll.bounds().bottom() - page.bounds().bottom() + px(8.)).max(px(0.));
                page.set_offset(page.offset() - point(px(0.), overflow));
                window.render_frame(cx);
                let bounds = window.find("study-bars").bounds();
                assert!(
                    bounds.bottom() <= page.bounds().bottom(),
                    "chart track is outside its parent viewport"
                );
                assert_eq!(
                    scroll.max_offset().y,
                    px(0.),
                    "daily chart manufactured vertical overflow"
                );
                if days == 7 {
                    assert_eq!(
                        scroll.max_offset().x,
                        px(0.),
                        "seven days fit and must not need a scrollbar"
                    );
                    assert_eq!(
                        scroll.offset().x,
                        px(0.),
                        "shorter chart retained stale horizontal offset"
                    );
                } else {
                    assert!(scroll.max_offset().x > px(0.));
                    scroll.set_offset(point(px(0.), px(0.)));
                    window.render_frame(cx);
                    window.drag(
                        point(bounds.left() + px(8.), bounds.bottom() - px(8.)),
                        point(bounds.right() - px(4.), bounds.bottom() - px(8.)),
                        cx,
                    );
                    assert_eq!(
                        scroll.offset().x,
                        -scroll.max_offset().x,
                        "daily chart thumb must reach its final day and stay at the viewport"
                    );
                }
                let last = window.find(("bar", days - 1)).bounds();
                assert!(
                    last.right() <= bounds.right() + px(1.) && last.left() >= bounds.left(),
                    "final chart day is clipped or unreachable: last={last:?}, viewport={bounds:?}"
                );
                window
                    .render_to_image()
                    .unwrap()
                    .save(database.with_file_name(format!("statistics-{days}-days.png")))
                    .unwrap();
            })
            .unwrap();
    }
    std::fs::write(database.with_file_name("native-statistics-scroll.json"),
        "{\"passed\":true,\"ranges\":[7,30,90,7],\"checks\":[\"fit-without-scroll\",\"thumb-drag-to-final-day\",\"shorter-range-resets-offset\"]}\n").unwrap();
}
