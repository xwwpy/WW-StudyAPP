//! Runner-only compositor evidence; layout/readback alone is not a presented frame.
use crate::app::StudyApp;
use gpui_kit::{AnyWindowHandle, AsyncApp, Entity, test::TestWindowExt};
use image::RgbaImage;
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(super) const WITNESS_BACKGROUND: u32 = 0x25354a;
const CHANNEL_TOLERANCE: u8 = 24;

fn compare_cards(
    expected: &RgbaImage,
    actual: &RgbaImage,
    origin: [u32; 2],
    cards: &[[u32; 4]],
) -> serde_json::Value {
    let reports: Vec<_> = cards.iter().map(|[left, top, right, bottom]| {
        let mut ink = 0_u64;
        let mut mismatches = 0_u64;
        let contained = left < right && top < bottom
            && *right <= expected.width() && *bottom <= expected.height()
            && origin[0].checked_add(*right).is_some_and(|x| x <= actual.width())
            && origin[1].checked_add(*bottom).is_some_and(|y| y <= actual.height());
        if contained {
            for y in *top..*bottom {
                for x in *left..*right {
                    let expected = expected.get_pixel(x, y).0;
                    // Only fully opaque, visible ink: blank white/gray backgrounds
                    // must never make an empty or stale native capture pass.
                    if expected[3] != 255 || expected[..3].iter().all(|c| *c >= 200) { continue; }
                    ink += 1;
                    let actual = actual.get_pixel(origin[0]+x, origin[1]+y).0;
                    if (0..3).any(|i| expected[i].abs_diff(actual[i]) > CHANNEL_TOLERANCE) {
                        mismatches += 1;
                    }
                }
            }
        }
        let matched = contained && ink >= 12 && mismatches * 100 <= ink * 5;
        serde_json::json!({"bounds":[left,top,right,bottom],"inkPixels":ink,"mismatches":mismatches,"matched":matched})
    }).collect();
    let matched = !reports.is_empty() && reports.iter().all(|r| r["matched"] == true);
    serde_json::json!({"matched":matched,"channelTolerance":CHANNEL_TOLERANCE,"maximumMismatchPercent":5,"cards":reports})
}

fn all_cards_opaque(ids: &[u64], entries: &[(u64, f32)]) -> bool {
    !ids.is_empty()
        && entries.len() == ids.len()
        && ids.iter().all(|id| {
            entries
                .iter()
                .any(|(entry, opacity)| entry == id && *opacity == 1.)
        })
}

pub(super) async fn desktop(
    handle: AnyWindowHandle,
    owner: &Entity<StudyApp>,
    ids: &[u64],
    database: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> serde_json::Value {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let began = Instant::now();
    let deadline = began + Duration::from_secs(5);
    let foreground = super::foreground();
    // A fitted DOM rectangle can still be the zero-opacity opening frame.
    // Observe the real queue's transition, not a guessed delay or a retry of
    // failed pixel samples; only then prepare one final native presentation.
    let mut opacity_frames = Vec::new();
    let opacities = loop {
        let opacities: Vec<_> = handle
            .update(cx, |_, _, cx| {
                owner
                    .read(cx)
                    .notifications
                    .read(cx)
                    .desktop_items()
                    .into_iter()
                    .map(|(notice, opacity)| (notice.id, opacity))
                    .collect()
            })
            .unwrap();
        assert_eq!(
            super::foreground(),
            foreground,
            "waiting for G opacity must not activate it"
        );
        opacity_frames.push(
            serde_json::json!({"elapsedMs":began.elapsed().as_millis(),"opacities":opacities}),
        );
        std::fs::write(
            database.with_file_name(format!("native-{name}-presentation.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({"stage":"waitingForOpacity","frames":opacity_frames}),
            )
            .unwrap(),
        )
        .unwrap();
        if all_cards_opaque(ids, &opacities) {
            break opacities;
        }
        assert!(
            Instant::now() < deadline,
            "G did not finish its opening transition within 5 seconds: {opacities:?}"
        );
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    };
    handle
        .update(cx, |_, window, cx| {
            assert!(!window.is_window_active());
            window.reset_render_diagnostics();
            window.render_frame(cx);
            // Wake a real present-only frame even if this passive window is idle.
            window.on_next_frame(|_, _| {});
        })
        .unwrap();
    // Yield to the real platform frame loop. Do not simulate presentation or
    // repeatedly draw/read images until a bad capture happens to pass.
    let diagnostics = loop {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let diagnostics = handle
            .update(cx, |_, window, _| window.render_diagnostics())
            .unwrap();
        assert_eq!(
            super::foreground(),
            foreground,
            "waiting for G presentation must not activate it"
        );
        std::fs::write(
            database.with_file_name(format!("native-{name}-presentation.json")),
            serde_json::to_vec_pretty(&serde_json::json!({"stage":"waitingForPresentation","diagnostics":diagnostics,"opacities":opacities,"opacityFrames":opacity_frames})).unwrap(),
        )
        .unwrap();
        if diagnostics["presentations"].as_u64().unwrap_or(0) > 0 {
            break diagnostics;
        }
        assert!(
            Instant::now() < deadline,
            "G did not present a native frame within 5 seconds: {diagnostics}"
        );
    };
    let comparison = handle
        .update(cx, |_, window, cx| {
            let scene = crate::notification_desktop::test_paint_observation(window, cx);
            std::fs::write(
                database.with_file_name(format!("native-{name}-scene.json")),
                serde_json::to_vec_pretty(&scene).unwrap(),
            )
            .unwrap();
            use windows::Win32::{
                Foundation::{HWND, POINT, RECT},
                Graphics::{Dwm::DwmFlush, Gdi::ClientToScreen},
                UI::WindowsAndMessaging::GetWindowRect,
            };
            use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let RawWindowHandle::Win32(raw) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                panic!("Win32 notification required")
            };
            let hwnd = HWND(raw.hwnd.get() as *mut _);
            let mut outer = RECT::default();
            let mut client = POINT::default();
            // SAFETY: own live HWND only. This flush waits for the compositor; it
            // does not activate/raise any window, inject input, or change the desktop.
            unsafe {
                GetWindowRect(hwnd, &mut outer).unwrap();
                ClientToScreen(hwnd, &mut client).ok().unwrap();
                DwmFlush().unwrap();
            }
            assert!(client.x >= outer.left && client.y >= outer.top);
            let origin = [
                (client.x - outer.left) as u32,
                (client.y - outer.top) as u32,
            ];
            let expected = window.render_to_image().unwrap();
            expected
                .save(database.with_file_name(format!("{name}.png")))
                .unwrap();
            let native = database.with_file_name(format!("{name}-native.png"));
            crate::native_test_pointer::capture_diagnostic(window, &native);
            let actual = image::open(native).unwrap().into_rgba8();
            let scale = window.scale_factor();
            let cards: Vec<_> = ids
                .iter()
                .map(|id| {
                    let bounds = window.find(format!("desktop-notice-{id}")).bounds();
                    [
                        f32::from(bounds.left()) * scale,
                        f32::from(bounds.top()) * scale,
                        f32::from(bounds.right()) * scale,
                        f32::from(bounds.bottom()) * scale,
                    ]
                    .map(|v| v.floor().max(0.) as u32)
                })
                .collect();
            let mut comparison = compare_cards(&expected, &actual, origin, &cards);
            let witness = [(WITNESS_BACKGROUND >> 16) as u8, (WITNESS_BACKGROUND >> 8) as u8, WITNESS_BACKGROUND as u8];
            let (width, height) = actual.dimensions();
            assert!(width > 8 && height > 8);
            // The independent witness fills the work area. Transparent native
            // margins must show that witness, not an opaque caption/frame slab.
            let samples: Vec<_> = [[2,2],[width/2,2],[width-3,2],[2,height/2],[width-3,height/2],[width/2,height-3]].into_iter().map(|[x,y]| {
                let pixel = actual.get_pixel(x,y).0;
                let matched = (0..3).all(|i| witness[i].abs_diff(pixel[i]) <= CHANNEL_TOLERANCE);
                serde_json::json!({"point":[x,y],"rgb":pixel[..3],"matched":matched})
            }).collect();
            let transparent = samples.iter().all(|sample| sample["matched"] == true);
            comparison["nativeMargins"] = serde_json::json!({"matched":transparent,"witnessColor":WITNESS_BACKGROUND,"samples":samples});
            comparison["matched"] = (comparison["matched"] == true && transparent).into();
            comparison
        })
        .unwrap();
    let receipt = serde_json::json!({"diagnostics":diagnostics,"comparison":comparison,"opacities":opacities,"opacityFrames":opacity_frames,"capturedOnce":true,"foregroundUnchanged":super::foreground()==foreground});
    std::fs::write(
        database.with_file_name(format!("native-{name}-presentation.json")),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["foregroundUnchanged"], true);
    assert_eq!(
        receipt["comparison"]["matched"], true,
        "native G capture differs from its final scene: {receipt}"
    );
    receipt
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn opacity_gate_requires_every_expected_card_and_a_complete_transition() {
        assert!(all_cards_opaque(&[1, 2], &[(1, 1.), (2, 1.)]));
        for entries in [
            vec![],
            vec![(1, 1.)],
            vec![(1, 1.), (2, 0.)],
            vec![(1, 1.), (2, 0.999)],
            vec![(1, 1.), (3, 1.)],
            vec![(1, 1.), (2, f32::NAN)],
        ] {
            assert!(!all_cards_opaque(&[1, 2], &entries));
        }
        assert!(!all_cards_opaque(&[], &[]));
    }

    #[test]
    fn real_screen_comparison_rejects_blank_stale_missing_and_faded_cards() {
        let cards = [[4, 4, 44, 28], [4, 36, 44, 60]];
        let mut expected = RgbaImage::new(52, 68);
        for [left, top, right, bottom] in cards {
            for y in top..bottom {
                for x in left..right {
                    expected.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
            for y in top + 4..top + 8 {
                for x in left + 4..left + 10 {
                    expected.put_pixel(x, y, Rgba([30, 30, 30, 255]));
                }
            }
        }
        let blank = RgbaImage::from_pixel(68, 107, Rgba([243, 244, 246, 255]));
        let mut actual = blank.clone();
        for (x, y, pixel) in expected.enumerate_pixels() {
            if pixel[3] == 255 {
                actual.put_pixel(x + 8, y + 31, *pixel);
            }
        }
        assert_eq!(
            compare_cards(&expected, &actual, [8, 31], &cards)["matched"],
            true
        );
        assert_eq!(
            compare_cards(&expected, &blank, [8, 31], &cards)["matched"],
            false
        );
        for y in 36..60 {
            for x in 4..44 {
                actual.put_pixel(x + 8, y + 31, Rgba([243, 244, 246, 255]));
            }
        }
        let missing = compare_cards(&expected, &actual, [8, 31], &cards);
        assert_eq!(missing["cards"][0]["matched"], true);
        assert_eq!(missing["cards"][1]["matched"], false);
        for y in 40..44 {
            for x in 8..14 {
                actual.put_pixel(x + 8, y + 31, Rgba([210, 210, 210, 255]));
            }
        }
        assert_eq!(
            compare_cards(&expected, &actual, [8, 31], &cards)["matched"],
            false
        );
        assert_eq!(
            compare_cards(&expected, &actual, [9, 31], &cards)["matched"],
            false
        );
        assert_eq!(
            compare_cards(&expected, &actual, [80, 31], &cards)["matched"],
            false
        );
        assert_eq!(
            compare_cards(&blank, &blank, [0, 0], &cards)["matched"],
            false
        );
    }
}
