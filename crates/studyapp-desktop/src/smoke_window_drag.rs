//! Whole-window movement through the real Win32 caption modal loop.
//! All input targets the isolated smoke process; no user library is touched.
use crate::app::{Page, StudyApp};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT},
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
            SendInput,
        },
        WindowsAndMessaging::{
            GetClipCursor, GetCursorPos, GetForegroundWindow, GetWindowRect, HTCAPTION,
            HTMAXBUTTON, HTMINBUTTON, HWND_NOTOPMOST, IsIconic, IsZoomed, SPI_GETDRAGFULLWINDOWS,
            SPI_SETDRAGFULLWINDOWS, SW_RESTORE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetCursorPos, SetWindowPos,
            ShowWindow, SystemParametersInfoW, WM_NCHITTEST,
        },
    },
};

fn full_window_drag() -> windows::core::Result<bool> {
    let mut enabled = windows::core::BOOL::default();
    unsafe {
        SystemParametersInfoW(
            SPI_GETDRAGFULLWINDOWS,
            0,
            Some((&mut enabled as *mut windows::core::BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )?;
    }
    Ok(enabled.as_bool())
}

fn set_full_window_drag(enabled: bool) -> windows::core::Result<()> {
    // Do not persist a user-profile setting or broadcast a settings update.
    unsafe {
        SystemParametersInfoW(
            SPI_SETDRAGFULLWINDOWS,
            u32::from(enabled),
            None,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
}

/// GitHub Windows desktops may show only a drag outline, leaving GetWindowRect
/// unchanged until release. Keep this fixture prerequisite scoped to CI and
/// restore it on normal return or a background-thread assertion unwind.
struct FullWindowDrag {
    previous: bool,
    effective: Option<bool>,
    changed: bool,
    evidence: PathBuf,
}

impl FullWindowDrag {
    fn prepare(evidence: PathBuf) -> Self {
        let previous = full_window_drag().expect("query full-window dragging");
        let mut guard = Self {
            previous,
            effective: None,
            changed: false,
            evidence,
        };
        let ci = std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true");
        std::fs::write(
            &guard.evidence,
            serde_json::to_vec_pretty(&json!({
                "originalDragFullWindows": previous, "ci": ci,
                "temporarilyEnabled": false, "restored": null,
            }))
            .unwrap(),
        )
        .unwrap();
        if !previous {
            assert!(
                ci,
                "outline-only dragging cannot measure held-window movement; only the CI fixture may temporarily enable full-window dragging"
            );
            guard.changed = true;
            set_full_window_drag(true)
                .expect("enable full-window dragging for isolated CI fixture");
        }
        let effective = full_window_drag().expect("verify full-window dragging");
        guard.effective = Some(effective);
        std::fs::write(
            &guard.evidence,
            serde_json::to_vec_pretty(&json!({
                "originalDragFullWindows": previous, "ci": ci,
                "temporarilyEnabled": guard.changed, "effectiveDragFullWindows": effective,
                "restored": null,
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(
            effective,
            "native drag benchmark requires actual full-window movement"
        );
        guard
    }
}

impl Drop for FullWindowDrag {
    fn drop(&mut self) {
        let restore = if self.changed {
            set_full_window_drag(self.previous)
        } else {
            Ok(())
        };
        let restored = full_window_drag();
        let evidence = json!({
            "originalDragFullWindows": self.previous,
            "effectiveDragFullWindows": self.effective,
            "temporarilyEnabled": self.changed,
            "restoreSucceeded": restore.is_ok(),
            "restoredDragFullWindows": restored.as_ref().ok().copied(),
            "restored": restore.is_ok() && restored.as_ref().is_ok_and(|value| *value == self.previous),
            "restoreError": restore.as_ref().err().map(ToString::to_string),
        });
        if let Ok(encoded) = serde_json::to_vec_pretty(&evidence) {
            let _ = std::fs::write(&self.evidence, encoded);
        }
        if let Err(error) = restore {
            eprintln!("failed to restore CI full-window dragging: {error}");
        }
    }
}

async fn settle(cx: &AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(250))
        .await;
}

fn mouse_button(down: bool) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: if down {
                    MOUSEEVENTF_LEFTDOWN
                } else {
                    MOUSEEVENTF_LEFTUP
                },
                ..Default::default()
            },
        },
    };
    assert_eq!(
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) },
        1
    );
}

fn control_point(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    id: &str,
    cx: &mut AsyncApp,
) -> (usize, i32, i32) {
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let center = window
                .find(SharedString::from(id.to_owned()))
                .bounds()
                .center();
            let native = view.read(cx).note_content.read(cx).native_state().unwrap();
            let scale = native["logicalBounds"][4].as_f64().unwrap() / 96.;
            (
                native["parentHwnd"].as_u64().unwrap() as usize,
                native["parentBounds"]["x"].as_i64().unwrap() as i32
                    + (f64::from(f32::from(center.x)) * scale).round() as i32,
                native["parentBounds"]["y"].as_i64().unwrap() as i32
                    + (f64::from(f32::from(center.y)) * scale).round() as i32,
            )
        })
        .unwrap()
}

fn assert_hit(raw: usize, x: i32, y: i32, expected: u32) {
    let hwnd = HWND(raw as *mut _);
    assert_eq!(
        unsafe { GetForegroundWindow() },
        hwnd,
        "native input must target the isolated foreground window"
    );
    let hit = unsafe {
        SendMessageW(
            hwnd,
            WM_NCHITTEST,
            None,
            Some(LPARAM(((y as u32) << 16 | (x as u32 & 0xffff)) as isize)),
        )
    };
    assert_eq!(
        hit.0, expected as isize,
        "custom titlebar has the wrong native hit region"
    );
}

/// CI native input helper; full-smoke close uses the actual custom Close region.
/// Coordinates come from the current GPUI layout, including maximized windows.
pub async fn click_window_control(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    id: &str,
    expected_hit: u32,
    cx: &mut AsyncApp,
) {
    let (raw, x, y) = control_point(window, view, id, cx);
    assert_eq!(unsafe { GetForegroundWindow() }, HWND(raw as *mut _));
    unsafe {
        SetCursorPos(x, y).unwrap();
    }
    // GPUI's native hit-test callback consults the rendered mouse hitboxes.
    // Deliver the real hover before verifying the corresponding Win32 region.
    settle(cx).await;
    assert_hit(raw, x, y, expected_hit);
    mouse_button(true);
    mouse_button(false);
}

fn native_drag(raw: usize, x: i32, y: i32, evidence: PathBuf) -> Value {
    let _full_window_drag = FullWindowDrag::prepare(evidence.with_extension("environment.json"));
    let hwnd = HWND(raw as *mut _);
    let mut rect = RECT::default();
    let mut clip_before = RECT::default();
    unsafe {
        GetWindowRect(hwnd, &mut rect).unwrap();
        GetClipCursor(&mut clip_before).unwrap();
    }
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd);
    unsafe {
        SetCursorPos(x, y).unwrap();
    }
    std::thread::sleep(Duration::from_millis(80));
    assert_hit(raw, x, y, HTCAPTION);
    mouse_button(true);
    // Always release our input when assertions fail, before reporting the failure.
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            mouse_button(false);
        }
    }
    let release = Release;
    std::thread::sleep(Duration::from_millis(50));
    let mut clip_held = RECT::default();
    unsafe {
        GetClipCursor(&mut clip_held).unwrap();
    }
    let began = Instant::now();
    let mut positions = Vec::new();
    let mut cursor_positions = Vec::new();
    for step in 0..80_i32 {
        assert_eq!(
            unsafe { GetForegroundWindow() },
            hwnd,
            "drag lost isolated foreground"
        );
        let offset = if step < 40 { step * 3 } else { (79 - step) * 3 };
        unsafe {
            SetCursorPos(x + offset, y + (offset / 4)).unwrap();
        }
        std::thread::sleep(Duration::from_millis(16));
        let mut current = RECT::default();
        let mut cursor = POINT::default();
        unsafe {
            GetWindowRect(hwnd, &mut current).unwrap();
            GetCursorPos(&mut cursor).unwrap();
        }
        positions.push((began.elapsed().as_micros(), current.left, current.top));
        cursor_positions.push((x + offset, y + offset / 4, cursor.x, cursor.y));
    }
    drop(release);
    let changed = positions
        .windows(2)
        .filter(|pair| pair[0].1 != pair[1].1 || pair[0].2 != pair[1].2)
        .count();
    let mut change_gaps = Vec::new();
    let mut last_change = positions[0].0;
    for pair in positions.windows(2) {
        if pair[0].1 != pair[1].1 || pair[0].2 != pair[1].2 {
            change_gaps.push(pair[1].0 - last_change);
            last_change = pair[1].0;
        }
    }
    change_gaps.sort_unstable();
    let report = json!({"inputSamples": positions.len(), "changedPositions": changed,
        "motionP95GapMicros": change_gaps.get(change_gaps.len().saturating_sub(1) * 95 / 100),
        "motionMaxGapMicros": change_gaps.last(),
        "elapsedMicros": began.elapsed().as_micros(), "positions": positions,
        "original": [rect.left, rect.top, rect.right, rect.bottom],
        "captionPoint": [x, y],
        "cursorClipBefore": [clip_before.left, clip_before.top, clip_before.right, clip_before.bottom],
        "cursorClipHeld": [clip_held.left, clip_held.top, clip_held.right, clip_held.bottom],
        "cursorRequestedActual": cursor_positions});
    // Persist diagnostics before the strict movement assertion, including for
    // outline/clip/desktop-geometry failures. No file writes occur while held.
    std::fs::write(&evidence, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    report
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    crate::smoke_library::until(
        window,
        view,
        "window drag fixture loaded",
        |app, _| {
            app.snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.notes.is_empty())
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.selected_note = app
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.notes.first().map(|note| note.id.clone()));
                cx.notify();
            })
        })
        .unwrap();
    let editor = window
        .update(cx, |_, _, cx| view.read(cx).note_content.clone())
        .unwrap();
    let visible = crate::smoke_visibility::visible(window, &editor, cx).await;
    let raw = visible["parentHwnd"].as_u64().unwrap() as usize;
    unsafe {
        let _ = ShowWindow(HWND(raw as *mut _), SW_RESTORE);
    }
    settle(cx).await;
    unsafe {
        SetWindowPos(
            HWND(raw as *mut _),
            Some(HWND_NOTOPMOST),
            30,
            30,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        )
        .unwrap();
    }
    assert!(
        !unsafe { IsZoomed(HWND(raw as *mut _)) }.as_bool(),
        "drag fixture must start restored"
    );
    // Query the renderer once, outside the held-input interval.
    let renderer = window
        .update(cx, |_, window, _| window.gpu_specs())
        .unwrap();
    std::fs::write(
        database.with_file_name("window-drag-renderer.json"),
        serde_json::to_vec_pretty(&json!({
            "gpuSpecs": renderer,
            "debugAssertions": cfg!(debug_assertions),
        }))
        .unwrap(),
    )
    .unwrap();
    let mut results = Vec::new();
    for (page, name) in [
        (Page::Today, "today"),
        (Page::Notes, "notes"),
        (Page::Settings, "settings"),
    ] {
        window
            .update(cx, |_, _, cx| {
                view.update(cx, |app, cx| {
                    app.page = page;
                    cx.notify();
                })
            })
            .unwrap();
        settle(cx).await;
        let before = window
            .update(cx, |_, _, cx| editor.read(cx).native_state().unwrap())
            .unwrap();
        let (_, x, y) = control_point(window, view, "window-title-drag", cx);
        let evidence = database.with_file_name(format!("window-drag-{name}-samples.json"));
        let diagnostic_path = database.with_file_name(format!("window-drag-{name}-render.json"));
        let geometry = window
            .update(cx, |_, window, _| {
                window.reset_render_diagnostics();
                json!({
                    "viewport": [f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)],
                    "scaleFactor": window.scale_factor(),
                })
            })
            .unwrap();
        let mut result = cx
            .background_executor()
            .spawn(async move { native_drag(raw, x, y, evidence) })
            .await;
        let diagnostics = window
            .update(cx, |_, window, _| window.render_diagnostics())
            .unwrap();
        // Keep both files on a failed movement gate. The interval also includes
        // input preparation and release; records use monotonic interval offsets.
        std::fs::write(
            &diagnostic_path,
            serde_json::to_vec_pretty(&json!({
                "page": name, "geometry": geometry,
                "samples": result, "rendering": diagnostics,
            }))
            .unwrap(),
        )
        .unwrap();
        let changed = result["changedPositions"].as_u64().unwrap();
        assert!(
            changed >= 30,
            "actual OS window movement stalled: only {changed} positions; diagnostics: {}",
            diagnostic_path.display()
        );
        settle(cx).await;
        let after = window
            .update(cx, |_, _, cx| editor.read(cx).native_state().unwrap())
            .unwrap();
        for key in ["boundsUpdates", "visibilityUpdates", "occlusionUpdates"] {
            assert_eq!(
                after[key], before[key],
                "same-DPI caption drag changed editor {key}"
            );
        }
        result["page"] = json!(name);
        result["nativeBoundsWrites"] = json!(0);
        result["nativeVisibilityWrites"] = json!(0);
        result["nativeOcclusionWrites"] = json!(0);
        results.push(result);
        unsafe {
            SetWindowPos(
                HWND(raw as *mut _),
                None,
                30,
                30,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .unwrap();
        }
        settle(cx).await;
    }
    std::fs::write(
        database.with_file_name("window-drag-performance.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
    exercise_controls(window, view, raw, cx).await;
    std::fs::write(database.with_file_name("window-controls-result.txt"), "PASS: custom titlebar exposes native Min/Max/Drag hit regions; actual minimize/restore, maximize/restore and caption double-click work. Close is covered by full smoke's persisted-draft shutdown.\n").unwrap();
    std::fs::write(database.with_file_name("window-drag-result.txt"), "PASS: real caption drag moved the isolated native window on Today, Notes and Settings; same-DPI movement preserved WebView bounds, visibility and occlusion regions. Timing is UI-thread elapsed time, not sampled CPU or input-to-photon latency.\n").unwrap();
}

async fn expect_native_state(raw: usize, maximized: bool, minimized: bool, cx: &AsyncApp) {
    for _ in 0..100 {
        if unsafe {
            IsZoomed(HWND(raw as *mut _)).as_bool() == maximized
                && IsIconic(HWND(raw as *mut _)).as_bool() == minimized
        } {
            return;
        }
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    panic!("custom titlebar did not reach expected native window state");
}

async fn exercise_controls(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    raw: usize,
    cx: &mut AsyncApp,
) {
    click_window_control(window, view, "window-maximize", HTMAXBUTTON, cx).await;
    expect_native_state(raw, true, false, cx).await;
    settle(cx).await;
    click_window_control(window, view, "window-maximize", HTMAXBUTTON, cx).await;
    expect_native_state(raw, false, false, cx).await;
    settle(cx).await;
    click_window_control(window, view, "window-minimize", HTMINBUTTON, cx).await;
    expect_native_state(raw, false, true, cx).await;
    unsafe {
        let _ = ShowWindow(HWND(raw as *mut _), SW_RESTORE);
    }
    expect_native_state(raw, false, false, cx).await;
    settle(cx).await;
    for maximized in [true, false] {
        let (raw, x, y) = control_point(window, view, "window-title-drag", cx);
        assert_eq!(unsafe { GetForegroundWindow() }, HWND(raw as *mut _));
        unsafe {
            SetCursorPos(x, y).unwrap();
        }
        settle(cx).await;
        assert_hit(raw, x, y, HTCAPTION);
        mouse_button(true);
        mouse_button(false);
        cx.background_executor()
            .timer(Duration::from_millis(60))
            .await;
        mouse_button(true);
        mouse_button(false);
        expect_native_state(raw, maximized, false, cx).await;
        settle(cx).await;
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AnyTooltip, AppContext, CaptionMoveEvent, CaptionMoveState, Context, Entity,
        InteractiveElement, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        ParentElement, Pixels, PlatformInput, Point, Render, RequestFrameOptions,
        StatefulInteractiveElement, StyleRefinement, Styled, Subscription, TestAppContext, Window,
        WindowControlArea, canvas, div, point, px,
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    struct MoveProbe {
        renders: Rc<Cell<usize>>,
        observed: Rc<RefCell<Vec<Point<Pixels>>>>,
        invalidate_on_move: bool,
        _subscription: Subscription,
    }

    impl Render for MoveProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div().child("position-only regression")
        }
    }

    #[test]
    fn caption_scope_requires_press_and_modal_entry_and_completes_once() {
        use CaptionMoveEvent::*;
        let mut state = CaptionMoveState::default();
        // Resize / keyboard move enter and exit without a caption candidate.
        assert!(!state.transition(EnterSizeMove));
        assert!(!state.is_active());
        assert!(!state.transition(ExitSizeMove));
        // A simple caption click / aborted DefWindowProc never activates.
        assert!(!state.transition(CaptionPress));
        assert!(!state.is_active());
        assert!(!state.transition(CaptionReturn));
        for finish in [ExitSizeMove, CaptionReturn] {
            assert!(!state.transition(CaptionPress));
            assert!(!state.transition(EnterSizeMove));
            assert!(state.is_active());
            assert!(state.transition(finish));
            assert!(!state.is_active());
            assert!(!state.transition(CaptionReturn));
            assert!(!state.transition(ExitSizeMove));
        }
    }

    struct CaptionProbe {
        renders: Rc<Cell<usize>>,
        observed: Rc<RefCell<Vec<Point<Pixels>>>>,
        cached: Entity<CaptionContent>,
        tooltip: Option<Entity<CaptionTooltip>>,
        _subscription: Subscription,
    }

    struct CaptionTooltip;
    impl Render for CaptionTooltip {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(20.)).h(px(10.))
        }
    }

    struct CaptionContent(Rc<Cell<usize>>);
    impl Render for CaptionContent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            div().size_full().child(
                div().flex().children([
                    div()
                        .id("caption-drag")
                        .w(px(200.))
                        .h(px(40.))
                        .window_control_area(WindowControlArea::Drag)
                        .on_drag((), |_, _, _, cx| cx.new(|_| CaptionTooltip)),
                    div()
                        .id("caption-close")
                        .w(px(50.))
                        .h(px(40.))
                        .window_control_area(WindowControlArea::Close)
                        .hover(|style| style.bg(gpui_kit::rgb(0xcccccc))),
                    div()
                        .id("caption-max")
                        .w(px(50.))
                        .h(px(40.))
                        .window_control_area(WindowControlArea::Max),
                    div()
                        .id("caption-min")
                        .w(px(50.))
                        .h(px(40.))
                        .window_control_area(WindowControlArea::Min),
                ]),
            )
        }
    }

    impl Render for CaptionProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div()
                .size_full()
                .child(
                    self.cached
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
                .children(self.tooltip.clone().map(|tooltip| {
                    canvas(
                        move |_, window, _| {
                            window.set_tooltip(AnyTooltip {
                                view: tooltip.into(),
                                mouse_position: point(px(150.), px(20.)),
                                check_visible_and_update: Rc::new(|_, _, _| true),
                            });
                        },
                        |_, _, _, _| {},
                    )
                }))
        }
    }

    #[gpui_kit::test]
    fn caption_pointer_jitter_defers_only_refresh_and_flushes_once(cx: &mut TestAppContext) {
        use CaptionMoveEvent::*;
        let renders = Rc::new(Cell::new(0));
        let observed = Rc::new(RefCell::new(Vec::new()));
        let handle = cx.add_window({
            let renders = renders.clone();
            let observed = observed.clone();
            move |window, cx: &mut Context<CaptionProbe>| {
                let subscription = cx.observe_window_bounds(window, |probe, window, _| {
                    probe.observed.borrow_mut().push(window.mouse_position());
                });
                let cached = cx.new(|_| CaptionContent(Rc::new(Cell::new(0))));
                CaptionProbe {
                    renders,
                    observed,
                    cached,
                    tooltip: None,
                    _subscription: subscription,
                }
            }
        });
        handle
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(100.), px(20.)),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                window.reset_render_diagnostics();
            })
            .unwrap();
        cx.set_window_sampled_pointer(handle.into(), point(px(100.), px(20.)));
        cx.simulate_caption_move_event(handle.into(), CaptionPress);
        cx.simulate_caption_move_event(handle.into(), EnterSizeMove);
        let baseline = renders.get();
        let cached = handle.update(cx, |view, _, _| view.cached.clone()).unwrap();
        let cached_baseline = cx.update(|cx| cached.read(cx).0.get());
        for x in [101., 102., 103.] {
            let pointer = point(px(x), px(20.));
            cx.set_window_sampled_pointer(handle.into(), pointer);
            cx.simulate_window_move(handle.into(), point(px(x), px(40.)));
            handle
                .update(cx, |_, window, _| {
                    assert_eq!(window.mouse_position(), pointer)
                })
                .unwrap();
        }
        assert_eq!(observed.borrow().len(), 3);
        assert_eq!(
            observed.borrow().last().copied(),
            Some(point(px(103.), px(20.)))
        );
        assert_eq!(
            renders.get(),
            baseline,
            "caption jitter bypassed the view cache"
        );
        assert_eq!(cx.update(|cx| cached.read(cx).0.get()), cached_baseline);
        // Entity dirtiness still renders normally during the modal move.
        cached.update(cx, |_, cx| cx.notify());
        assert_eq!(renders.get(), baseline + 1);
        assert_eq!(cx.update(|cx| cached.read(cx).0.get()), cached_baseline + 1);
        cx.simulate_window_frame_request(
            handle.into(),
            RequestFrameOptions {
                force_render: true,
                require_presentation: true,
                ..Default::default()
            },
        );
        assert_eq!(cx.update(|cx| cached.read(cx).0.get()), cached_baseline + 2);
        // Actual platform mouse events retain their ordinary hover refresh.
        let before_real_mouse_move = renders.get();
        handle
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(225.), px(20.)),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            })
            .unwrap();
        assert!(renders.get() > before_real_mouse_move);
        let before_exit = renders.get();
        cx.set_window_sampled_pointer(handle.into(), point(px(225.), px(20.)));
        cx.simulate_caption_move_event(handle.into(), ExitSizeMove);
        cx.run_until_parked();
        assert_eq!(
            renders.get(),
            before_exit + 1,
            "pending hover must flush even with the same final pointer"
        );
        cx.simulate_caption_move_event(handle.into(), CaptionReturn);
        cx.simulate_caption_move_event(handle.into(), ExitSizeMove);
        cx.run_until_parked();
        assert_eq!(
            renders.get(),
            before_exit + 1,
            "duplicate completion refreshed twice"
        );
        handle
            .update(cx, |_, window, _| {
                let diagnostic = window.render_diagnostics();
                assert_eq!(diagnostic["captionPointerDeferrals"], 3);
                assert_eq!(diagnostic["captionHoverFlushes"], 1);
                assert_eq!(diagnostic["positionRefreshes"], 0);
            })
            .unwrap();
        // An aborted modal procedure without a delivered exit message must
        // flush once too, then ordinary programmatic hover works again.
        handle
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(100.), px(20.)),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            })
            .unwrap();
        cx.simulate_caption_move_event(handle.into(), CaptionPress);
        cx.simulate_caption_move_event(handle.into(), EnterSizeMove);
        cx.set_window_sampled_pointer(handle.into(), point(px(105.), px(20.)));
        cx.simulate_window_move(handle.into(), point(px(70.), px(60.)));
        let before_cancel = renders.get();
        cx.simulate_caption_move_event(handle.into(), CaptionReturn);
        cx.run_until_parked();
        assert_eq!(renders.get(), before_cancel + 1);
        cx.simulate_caption_move_event(handle.into(), CaptionReturn);
        cx.run_until_parked();
        assert_eq!(renders.get(), before_cancel + 1);
        cx.set_window_sampled_pointer(handle.into(), point(px(106.), px(20.)));
        cx.simulate_window_move(handle.into(), point(px(80.), px(60.)));
        assert_eq!(renders.get(), before_cancel + 2);
    }

    #[gpui_kit::test]
    fn caption_refresh_guards_preserve_buttons_overlays_and_geometry(cx: &mut TestAppContext) {
        use CaptionMoveEvent::*;
        let handle = cx.add_window(|window, cx: &mut Context<CaptionProbe>| {
            let subscription = cx.observe_window_bounds(window, |_, _, _| {});
            CaptionProbe {
                renders: Rc::new(Cell::new(0)),
                observed: Rc::new(RefCell::new(Vec::new())),
                cached: cx.new(|_| CaptionContent(Rc::new(Cell::new(0)))),
                tooltip: None,
                _subscription: subscription,
            }
        });
        let cursor = |cx: &mut TestAppContext, position| {
            handle
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers: Modifiers::default(),
                        }),
                        cx,
                    );
                })
                .unwrap();
            cx.set_window_sampled_pointer(handle.into(), position);
        };
        let reset = |cx: &mut TestAppContext| {
            handle
                .update(cx, |_, window, _| window.reset_render_diagnostics())
                .unwrap();
        };
        let moved = |cx: &mut TestAppContext, position| {
            cx.set_window_sampled_pointer(handle.into(), position);
            cx.simulate_window_move(handle.into(), point(px(60.), px(60.)));
        };
        let assert_immediate = |cx: &mut TestAppContext| {
            handle
                .update(cx, |_, window, _| {
                    let d = window.render_diagnostics();
                    assert_eq!(d["positionRefreshes"], 1);
                    assert_eq!(d["captionPointerDeferrals"], 0);
                })
                .unwrap();
        };
        // Both endpoints must resolve to the rendered Drag area, including
        // its real clipping/occlusion. All three window buttons are excluded.
        for (old, new) in [
            ((100., 20.), (225., 20.)),
            ((100., 20.), (275., 20.)),
            ((100., 20.), (325., 20.)),
            ((100., 20.), (100., 60.)),
            ((225., 20.), (100., 20.)),
        ] {
            cursor(cx, point(px(old.0), px(old.1)));
            cx.simulate_caption_move_event(handle.into(), CaptionPress);
            cx.simulate_caption_move_event(handle.into(), EnterSizeMove);
            reset(cx);
            moved(cx, point(px(new.0), px(new.1)));
            assert_immediate(cx);
            cx.simulate_caption_move_event(handle.into(), ExitSizeMove);
            cx.run_until_parked();
        }
        // Programmatic movement, a simple click, and an unrelated size loop
        // (resize/keyboard move) cannot opt into caption deferral.
        for events in [vec![], vec![CaptionPress], vec![EnterSizeMove]] {
            cursor(cx, point(px(100.), px(20.)));
            for event in events {
                cx.simulate_caption_move_event(handle.into(), event);
            }
            reset(cx);
            moved(cx, point(px(101.), px(20.)));
            assert_immediate(cx);
            cx.simulate_caption_move_event(handle.into(), CaptionReturn);
            cx.run_until_parked();
        }
        cursor(cx, point(px(100.), px(20.)));
        cx.simulate_caption_move_event(handle.into(), CaptionPress);
        cx.simulate_caption_move_event(handle.into(), EnterSizeMove);
        handle
            .update(cx, |view, _, cx| {
                view.tooltip = Some(cx.new(|_| CaptionTooltip));
                cx.notify();
            })
            .unwrap();
        reset(cx);
        moved(cx, point(px(101.), px(20.)));
        assert_immediate(cx);
        handle
            .update(cx, |view, _, cx| {
                view.tooltip = None;
                cx.notify();
            })
            .unwrap();
        // Real GPUI drag, started by the regular press and threshold crossing.
        cursor(cx, point(px(100.), px(20.)));
        handle
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: point(px(100.), px(20.)),
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(110.), px(20.)),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                assert!(cx.has_active_drag());
            })
            .unwrap();
        reset(cx);
        moved(cx, point(px(111.), px(20.)));
        assert_immediate(cx);
        handle
            .update(cx, |_, window, cx| {
                cx.stop_active_drag(window);
            })
            .unwrap();
        cursor(cx, point(px(100.), px(20.)));
        let (viewport, scale) = handle
            .update(cx, |_, window, _| {
                (window.viewport_size(), window.scale_factor())
            })
            .unwrap();
        // Even when native geometry changes arrive in the move callback,
        // rather than a separate resize callback, they must refresh.
        for (size, scale, expected) in [
            (
                gpui_kit::size(viewport.width + px(10.), viewport.height),
                scale,
                "viewportChanges",
            ),
            (
                gpui_kit::size(viewport.width + px(10.), viewport.height),
                scale + 0.25,
                "scaleChanges",
            ),
        ] {
            reset(cx);
            cx.set_window_sampled_pointer(handle.into(), point(px(101.), px(20.)));
            cx.simulate_window_move_with_geometry(
                handle.into(),
                point(px(60.), px(60.)),
                size,
                scale,
            );
            assert_immediate(cx);
            handle
                .update(cx, |_, window, _| {
                    assert_eq!(window.render_diagnostics()[expected], 1)
                })
                .unwrap();
            cursor(cx, point(px(100.), px(20.)));
        }
        // Resize/scale and force-render remain ordinary invalidation paths.
        reset(cx);
        let before = handle.update(cx, |view, _, _| view.renders.get()).unwrap();
        cx.simulate_window_resize(handle.into(), gpui_kit::size(px(500.), px(400.)));
        cx.simulate_window_scale_factor_change(handle.into(), 1.25);
        handle
            .update(cx, |view, window, _| {
                assert!(view.renders.get() >= before + 2);
                assert_eq!(window.scale_factor(), 1.25);
                assert_eq!(window.render_diagnostics()["boundsChanges"], 2);
            })
            .unwrap();
        cx.simulate_caption_move_event(handle.into(), CaptionReturn);
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn position_only_moves_notify_observers_without_rendering(cx: &mut TestAppContext) {
        let renders = Rc::new(Cell::new(0));
        let observed = Rc::new(RefCell::new(Vec::new()));
        let handle = cx.add_window({
            let renders = renders.clone();
            let observed = observed.clone();
            move |window, cx: &mut Context<MoveProbe>| {
                let subscription = cx.observe_window_bounds(window, |probe, window, cx| {
                    probe.observed.borrow_mut().push(window.bounds().origin);
                    if probe.invalidate_on_move {
                        cx.notify();
                    }
                });
                MoveProbe {
                    renders,
                    observed,
                    invalidate_on_move: false,
                    _subscription: subscription,
                }
            }
        });
        cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        let initial_renders = renders.get();
        assert!(initial_renders > 0);
        handle
            .update(cx, |_, window, _| window.reset_render_diagnostics())
            .unwrap();
        for offset in 1..=5 {
            cx.simulate_window_move(handle.into(), point(px(offset as f32 * 10.), px(20.)));
            cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        }
        assert_eq!(
            observed.borrow().len(),
            5,
            "bounds observers lost movement events"
        );
        assert_eq!(
            observed.borrow().last().copied(),
            Some(point(px(50.), px(20.)))
        );
        assert_eq!(
            renders.get(),
            initial_renders,
            "pure desktop movement rebuilt the client scene"
        );
        handle
            .update(cx, |_, window, _| {
                let diagnostics = window.render_diagnostics();
                assert_eq!(diagnostics["positionChanges"], 5);
                assert_eq!(diagnostics["positionRefreshes"], 0);
                assert_eq!(diagnostics["sceneDraws"], 0);
            })
            .unwrap();

        // Anchored overlays and any other bounds observer can still invalidate.
        handle
            .update(cx, |probe, _, _| probe.invalidate_on_move = true)
            .unwrap();
        cx.simulate_window_move(handle.into(), point(px(60.), px(20.)));
        cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        assert_eq!(
            renders.get(),
            initial_renders + 1,
            "observer cx.notify was suppressed"
        );
        handle
            .update(cx, |probe, _, _| probe.invalidate_on_move = false)
            .unwrap();

        // Even a same-size native restore must retain the old resize behavior.
        let viewport = handle
            .update(cx, |_, window, _| window.viewport_size())
            .unwrap();
        cx.simulate_window_resize(handle.into(), viewport);
        cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        assert_eq!(renders.get(), initial_renders + 2);
        cx.simulate_window_scale_factor_change(handle.into(), 3.);
        cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        assert_eq!(renders.get(), initial_renders + 3);
        handle
            .update(cx, |_, window, _| assert_eq!(window.scale_factor(), 3.))
            .unwrap();

        // Programmatic movement under a stationary pointer must update hover.
        handle
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(15.), px(20.)),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            })
            .unwrap();
        cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        let before_mouse_move = renders.get();
        cx.simulate_window_move(handle.into(), point(px(70.), px(20.)));
        cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        assert_eq!(
            renders.get(),
            before_mouse_move + 1,
            "client cursor change did not refresh hover state"
        );
        handle
            .update(cx, |_, window, _| {
                let diagnostics = window.render_diagnostics();
                assert_eq!(diagnostics["positionChanges"], 7);
                assert_eq!(diagnostics["positionRefreshes"], 1);
                assert_eq!(diagnostics["pointerChanges"], 1);
                assert_eq!(diagnostics["scaleChanges"], 0);
                assert_eq!(diagnostics["viewportChanges"], 0);
                assert_eq!(diagnostics["boundsChanges"], 2);
                assert_eq!(diagnostics["observerDirtyTransitions"], 1);
                assert_eq!(diagnostics["sceneDraws"], 4);
                // TestAppContext draws dirty windows when updates finish, so
                // explicit frame requests submit those already prepared scenes.
                assert_eq!(diagnostics["requestFrameDraws"], 0);
                assert_eq!(diagnostics["requestFramePresentOnly"], 4);
                assert_eq!(diagnostics["presentations"], 4);
                assert_eq!(diagnostics["positions"][6]["pointerChanged"], true);
                assert_eq!(
                    diagnostics["positions"][6]["previousPointer"],
                    serde_json::json!([15., 20.])
                );
                window.reset_render_diagnostics();
                assert_eq!(window.render_diagnostics()["positionChanges"], 0);
            })
            .unwrap();
        cx.simulate_window_frame_request(
            handle.into(),
            RequestFrameOptions {
                force_render: true,
                require_presentation: true,
                ..Default::default()
            },
        );
        handle
            .update(cx, |_, window, _| {
                let diagnostics = window.render_diagnostics();
                assert_eq!(diagnostics["requestFrameDraws"], 1);
                assert_eq!(diagnostics["sceneDraws"], 1);
                assert_eq!(diagnostics["presentations"], 1);
                window.reset_render_diagnostics();
            })
            .unwrap();
        for offset in 0..300 {
            cx.simulate_window_move(handle.into(), point(px(offset as f32), px(20.)));
        }
        cx.simulate_window_frame_request(handle.into(), RequestFrameOptions::default());
        cx.simulate_window_frame_request(
            handle.into(),
            RequestFrameOptions {
                require_presentation: true,
                ..Default::default()
            },
        );
        handle
            .update(cx, |_, window, _| {
                let diagnostics = window.render_diagnostics();
                assert_eq!(diagnostics["positionChanges"], 300);
                assert_eq!(diagnostics["positions"].as_array().unwrap().len(), 256);
                assert_eq!(diagnostics["sceneDraws"], 0);
                assert_eq!(diagnostics["requestFrameNoops"], 1);
                assert_eq!(diagnostics["requestFramePresentOnly"], 1);
                assert_eq!(diagnostics["presentations"], 1);
            })
            .unwrap();
    }
}
