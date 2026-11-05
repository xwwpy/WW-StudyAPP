//! Runner-only verification of the real desktop widget and its shared timer.
use crate::native_test_pointer as pointer;
use crate::{app::StudyApp, smoke_library::until};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::json;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use studyapp_ui::timer_card::TimerCardAction;
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT},
    Graphics::Gdi::{ClientToScreen, GetDC, GetPixel, ReleaseDC},
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
            SendInput,
        },
        WindowsAndMessaging::*,
    },
};
use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};

fn raw(window: &Window) -> usize {
    let RawWindowHandle::Win32(raw) = HasWindowHandle::window_handle(window).unwrap().as_raw()
    else {
        panic!("Win32 fixture required")
    };
    raw.hwnd.get() as usize
}
fn rect(raw: usize) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        GetWindowRect(HWND(raw as *mut _), &mut rect).unwrap();
    }
    rect
}
fn point(window: &mut Window, id: &str, cx: &mut App) -> POINT {
    window.render_frame(cx);
    let center = window
        .find(SharedString::from(id.to_owned()))
        .bounds()
        .center();
    let mut point = POINT {
        x: (f32::from(center.x) * window.scale_factor()).round() as i32,
        y: (f32::from(center.y) * window.scale_factor()).round() as i32,
    };
    assert!(unsafe { ClientToScreen(HWND(raw(window) as *mut _), &mut point) }.as_bool());
    point
}
fn hit(raw: usize, point: POINT) -> isize {
    unsafe {
        SendMessageW(
            HWND(raw as *mut _),
            WM_NCHITTEST,
            None,
            Some(LPARAM(
                ((point.y as u32) << 16 | (point.x as u32 & 0xffff)) as isize,
            )),
        )
    }
    .0
}
fn button(down: bool) {
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
fn gesture(raw: usize, start: POINT, dx: i32, dy: i32, expected: u32) {
    let hwnd = HWND(raw as *mut _);
    assert_eq!(
        unsafe { GetForegroundWindow() },
        hwnd,
        "gesture must target this isolated widget"
    );
    unsafe {
        SetCursorPos(start.x, start.y).unwrap();
    }
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(hit(raw, start), expected as isize);
    button(true);
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            button(false);
        }
    }
    let release = Release;
    for i in 1..=20 {
        unsafe {
            SetCursorPos(start.x + dx * i / 20, start.y + dy * i / 20).unwrap();
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    drop(release);
}
fn pixel(point: POINT) -> u32 {
    unsafe {
        let dc = GetDC(None);
        assert!(!dc.is_invalid(), "screen DC must be available");
        let color = GetPixel(dc, point.x, point.y).0;
        ReleaseDC(None, dc);
        assert_ne!(color, u32::MAX, "screen pixel must be readable");
        color
    }
}
async fn settle(cx: &AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(450))
        .await;
}

async fn until_native(label: &str, cx: &AsyncApp, ready: impl Fn() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "native transition: {label}"
        );
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}

pub async fn exercise(
    main: AnyWindowHandle,
    app: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    until(
        main,
        app,
        "timer fixture snapshot",
        |app, _| app.snapshot.is_some() && !app.draft_loading,
        cx,
    )
    .await;
    main.update(cx, |_, window, cx| {
        app.update(cx, |app, cx| {
            app.navigate(crate::app::Page::Today, window, cx);
            app.timer_action(
                TimerCardAction::ChooseKind(studyapp_core::TimerKind::CountUp),
                window,
                cx,
            );
            // A pre-upgrade tall window must compact to real card content.
            app.widget_state.height = 750;
        });
        window.render_frame(cx);
        let timer = window.find("today-timer").bounds();
        let body = window.find("page-scroll").bounds();
        assert!(timer.bottom() <= body.top());
        assert!(body.size.height >= px(160.));
        assert_eq!(window.find("timer-show-desktop").checked(), Some(false));
    })
    .unwrap();
    until(
        main,
        app,
        "single countup timer",
        |app, _| {
            app.snapshot
                .as_ref()
                .is_some_and(|s| s.timer.kind == studyapp_core::TimerKind::CountUp)
        },
        cx,
    )
    .await;
    until(
        main,
        app,
        "countup fixture storage settled",
        |app, _| !app.storage_busy,
        cx,
    )
    .await;
    main.update(cx, |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find("timer-show-desktop").bounds();
        let viewport = window.viewport_size();
        pointer::capture_diagnostic(
            window,
            &database.with_file_name("timer-widget-before-open.bmp"),
        );
        eprintln!(
            "native timer entry: bounds={bounds:?}, viewport={viewport:?}, scale={}",
            window.scale_factor()
        );
        assert!(bounds.left() >= px(0.) && bounds.right() <= viewport.width);
        assert!(bounds.top() >= px(0.) && bounds.bottom() <= viewport.height);
        pointer::click(window, bounds.center());
    })
    .unwrap();
    until(
        main,
        app,
        "fixed timer opens desktop widget",
        |app, _| app.timer_widget.is_some(),
        cx,
    )
    .await;
    main.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("timer-show-desktop").checked(), Some(true));
        assert_eq!(window.find("timer-show-desktop").label(), Some("隐藏桌面小组件"));
        let timer = window.find("today-timer").bounds();
        let body = window.find("page-scroll").bounds();
        assert!(timer.bottom() <= body.top());
        assert_eq!(unsafe { GetForegroundWindow() }, HWND(raw(window) as *mut _), "opening the widget must not take focus");
        pointer::capture(window, &database.parent().unwrap().join("timer-widget-today-fixed.bmp"));
        std::fs::write(database.with_file_name("timer-widget-today-fixed.json"), serde_json::to_vec_pretty(&json!({
            "fixedBounds": [f32::from(timer.left()), f32::from(timer.top()), f32::from(timer.size.width), f32::from(timer.size.height)],
            "bodyTop": f32::from(body.top()),
            "desktopOpen": window.find("timer-show-desktop").checked(),
            "timerValue": window.find("today-timer-value").label()
        })).unwrap()).unwrap();
    }).unwrap();
    let sessions_before = app.read_with(cx, |app, _| app.snapshot.as_ref().unwrap().sessions.len());
    let widget = app.read_with(cx, |app, _| app.timer_widget.unwrap());
    settle(cx).await;
    let (handle, caption) = widget
        .update(cx, |_, window, cx| {
            window.activate_window();
            let card = window.find("timer-widget-card").bounds();
            assert_eq!(card.origin, gpui_kit::point(px(0.), px(0.)));
            assert_eq!(card.size, window.viewport_size());
            assert!(
                card.size.height < px(700.),
                "old blank height must be removed"
            );
            let content = window.find("timer-widget-content").bounds();
            let body = window.find("timer-widget-scroll").bounds();
            assert!(content.size.height <= body.size.height + px(1.));
            assert!(body.size.height - content.size.height < px(70.));
            for id in [
                "timer-widget-drag",
                "widget-pin",
                "widget-open-main",
                "widget-settings",
                "widget-close",
                "widget-exit",
            ] {
                let bounds = window.find(id).bounds();
                assert!(bounds.left() >= card.left() && bounds.right() <= card.right());
                assert!(bounds.top() >= card.top() && bounds.bottom() <= card.bottom());
            }
            (raw(window), point(window, "timer-widget-drag", cx))
        })
        .unwrap();
    settle(cx).await;
    let style = unsafe { GetWindowLongPtrW(HWND(handle as *mut _), GWL_EXSTYLE) };
    assert_ne!(style & WS_EX_TOOLWINDOW.0 as isize, 0);
    assert_ne!(style & WS_EX_TOPMOST.0 as isize, 0);
    widget
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let center = window.find("widget-pin").bounds().center();
            pointer::click(window, center);
        })
        .unwrap();
    until(
        main,
        app,
        "widget pin button persists off",
        |app, _| {
            app.snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.settings.timer_widget.pinned)
        },
        cx,
    )
    .await;
    until_native("native widget unpin", cx, || unsafe {
        GetWindowLongPtrW(HWND(handle as *mut _), GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize == 0
    })
    .await;
    widget
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let center = window.find("widget-pin").bounds().center();
            pointer::click(window, center);
        })
        .unwrap();
    until(
        main,
        app,
        "widget pin button persists on",
        |app, _| {
            app.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.settings.timer_widget.pinned)
        },
        cx,
    )
    .await;
    until_native("native widget pin", cx, || unsafe {
        GetWindowLongPtrW(HWND(handle as *mut _), GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize != 0
    })
    .await;
    assert_eq!(
        handle,
        widget.update(cx, |_, window, _| raw(window)).unwrap()
    );
    let before = rect(handle);
    cx.background_executor()
        .spawn(async move {
            gesture(handle, caption, 60, 35, HTCAPTION);
        })
        .await;
    settle(cx).await;
    let moved = rect(handle);
    assert!(
        (moved.left - before.left).abs() > 20 || (moved.top - before.top).abs() > 20,
        "native caption drag must move the window"
    );
    let corner = POINT {
        x: moved.right - 2,
        y: moved.bottom - 2,
    };
    cx.background_executor()
        .spawn(async move {
            gesture(handle, corner, 55, 45, HTBOTTOMRIGHT);
        })
        .await;
    settle(cx).await;
    let resized = rect(handle);
    assert!(
        resized.right - resized.left > moved.right - moved.left + 20,
        "native edge gesture must resize the window"
    );
    let points = [
        (
            "left",
            POINT {
                x: resized.left + 1,
                y: (resized.top + resized.bottom) / 2,
            },
            HTLEFT,
        ),
        (
            "right",
            POINT {
                x: resized.right - 2,
                y: (resized.top + resized.bottom) / 2,
            },
            HTRIGHT,
        ),
        (
            "top",
            POINT {
                x: (resized.left + resized.right) / 2,
                y: resized.top + 1,
            },
            HTTOP,
        ),
        (
            "bottom",
            POINT {
                x: (resized.left + resized.right) / 2,
                y: resized.bottom - 2,
            },
            HTBOTTOM,
        ),
        (
            "top-left",
            POINT {
                x: resized.left + 1,
                y: resized.top + 1,
            },
            HTTOPLEFT,
        ),
        (
            "top-right",
            POINT {
                x: resized.right - 2,
                y: resized.top + 1,
            },
            HTTOPRIGHT,
        ),
        (
            "bottom-left",
            POINT {
                x: resized.left + 1,
                y: resized.bottom - 2,
            },
            HTBOTTOMLEFT,
        ),
        (
            "bottom-right",
            POINT {
                x: resized.right - 2,
                y: resized.bottom - 2,
            },
            HTBOTTOMRIGHT,
        ),
    ];
    let mut hits = Vec::new();
    for (name, point, expected) in points {
        unsafe {
            SetCursorPos(point.x, point.y).unwrap();
        }
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
        let actual = hit(handle, point);
        hits.push(json!({"edge":name,"hit":actual}));
        assert_eq!(
            actual, expected as isize,
            "native widget resize edge: {name}"
        );
    }
    // The card now fills the client area; only its rounded corner is clear.
    // Compare that corner with the same pixel behind the hidden widget.
    // Never require a separate transparent toolbar/padding outside the card.
    let clear = widget
        .update(cx, |_, window, _| {
            let mut client = RECT::default();
            unsafe {
                GetClientRect(HWND(handle as *mut _), &mut client).unwrap();
            }
            let mut point = POINT {
                x: (0.5 * window.scale_factor()).floor() as i32,
                y: (0.5 * window.scale_factor()).floor() as i32,
            };
            assert!(unsafe { ClientToScreen(HWND(handle as *mut _), &mut point) }.as_bool());
            point
        })
        .unwrap();
    unsafe {
        let _ = ShowWindow(HWND(handle as *mut _), SW_HIDE);
    }
    settle(cx).await;
    let behind = pixel(clear);
    unsafe {
        let _ = ShowWindow(HWND(handle as *mut _), SW_SHOWNOACTIVATE);
    }
    settle(cx).await;
    let through = pixel(clear);
    std::fs::write(
        database
            .parent()
            .unwrap()
            .join("timer-widget-transparency.json"),
        serde_json::to_vec_pretty(&json!({"behind":behind,"through":through,
            "point":{"x":clear.x,"y":clear.y}}))
        .unwrap(),
    )
    .unwrap();
    widget
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            pointer::capture_diagnostic(
                window,
                &database
                    .parent()
                    .unwrap()
                    .join("timer-widget-diagnostic.bmp"),
            );
            // GPUI's Windows activation is an executor task. Submit it on this
            // turn, then await the actual OS transition before a strict capture.
            window.activate_window();
        })
        .unwrap();
    until_native("widget foreground after re-show", cx, || unsafe {
        GetForegroundWindow() == HWND(handle as *mut _)
    })
    .await;
    widget
        .update(cx, |_, window, _| {
            pointer::capture(window, &database.parent().unwrap().join("timer-widget.bmp"));
        })
        .unwrap();
    assert_eq!(
        through, behind,
        "the rounded card corner must expose the underlying desktop"
    );
    widget
        .update(cx, |_, window, _| {
            let center = window.find("widget-pause").bounds().center();
            pointer::click(window, center);
        })
        .unwrap();
    until(
        main,
        app,
        "widget pause shares core state",
        |app, _| app.snapshot.as_ref().is_some_and(|s| s.timer.paused),
        cx,
    )
    .await;
    let paused = app.read_with(cx, |app, _| {
        app.snapshot.as_ref().unwrap().timer.active_seconds
    });
    cx.background_executor()
        .timer(Duration::from_millis(1200))
        .await;
    assert_eq!(
        paused,
        app.read_with(cx, |app, _| app
            .snapshot
            .as_ref()
            .unwrap()
            .timer
            .active_seconds)
    );
    main.update(cx, |_, window, cx| {
        app.update(cx, |app, cx| {
            app.timer_action(TimerCardAction::Toggle, window, cx);
            assert!(!app.handle_close(window, cx));
            assert!(
                !app.close_requested,
                "minimize must not freeze the draft-close barrier"
            );
        })
    })
    .unwrap();
    let main_handle = main.update(cx, |_, window, _| raw(window)).unwrap();
    // minimize_window uses ShowWindowAsync on Windows as well.
    until_native("main minimized", cx, || unsafe {
        IsIconic(HWND(main_handle as *mut _)).as_bool()
    })
    .await;
    cx.background_executor()
        .timer(Duration::from_millis(2200))
        .await;
    assert!(
        app.read_with(cx, |app, _| app
            .snapshot
            .as_ref()
            .unwrap()
            .timer
            .active_seconds)
            > paused,
        "the minimized main window must keep the single worker polling"
    );
    assert_eq!(
        sessions_before,
        app.read_with(cx, |app, _| app.snapshot.as_ref().unwrap().sessions.len()),
        "generic timer must not create learning records"
    );
    main.update(cx, |_, window, _| window.activate_window())
        .unwrap();
    let state = app.read_with(cx, |app, _| app.widget_state.clone());
    let path = StudyApp::widget_state_path().unwrap();
    assert!(
        crate::timer_widget_window::WidgetWindowState::load(&path)
            .unwrap()
            .open
    );
    main.update(cx, |_, _, cx| {
        app.update(cx, |app, cx| app.close_widget(false, cx))
    })
    .unwrap();
    assert!(
        !crate::timer_widget_window::WidgetWindowState::load(&path)
            .unwrap()
            .open
    );
    main.update(cx, |_, _, cx| app.update(cx, |app, cx| app.open_widget(cx)))
        .unwrap();
    settle(cx).await;
    let reopened = app.read_with(cx, |app, _| app.widget_state.clone());
    assert_eq!(
        (state.x, state.y, state.width),
        (reopened.x, reopened.y, reopened.width)
    );
    // Opening compacts height again; manual resizing remains available while
    // open, but does not bring old blank window regions back on reopening.
    app.read_with(cx, |app, _| app.timer_widget.unwrap())
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let body = window.find("timer-widget-scroll").bounds();
            let content = window.find("timer-widget-content").bounds();
            assert!(content.size.height <= body.size.height + px(1.));
            assert!(body.size.height - content.size.height < px(70.));
        })
        .unwrap();
    // End through the real explicit-exit path, not cx.quit: it must drain the
    // current native editor, then remove both windows and retain restore state.
    main.update(cx, |_, window, cx| {
        app.update(cx, |app, cx| {
            app.page = crate::app::Page::Notes;
            app.start_new_draft(None, window, cx);
        })
    })
    .unwrap();
    until(
        main,
        app,
        "exit draft editor",
        |app, cx| {
            app.note_editor.is_some() && !app.draft_loading && app.note_content.read(cx).ready()
        },
        cx,
    )
    .await;
    let editor = app.read_with(cx, |app, _| app.note_content.clone());
    let marker = "Widget explicit exit keeps this draft: 计时草稿";
    crate::smoke_editor::bridge(
        main,
        &editor,
        json!({"type":"replace","from":0,"to":0,"text":marker}),
        cx,
    )
    .await;
    let live = crate::smoke_editor::bridge(main, &editor, json!({"type":"snapshot"}), cx).await;
    assert_eq!(live["text"], marker);
    let disk = app.read_with(cx, |app, _| {
        crate::drafts::DraftStore::record_path(
            &app.last_library,
            app.active_draft_id.as_ref().expect("active exit draft"),
        )
        .unwrap()
    });
    let report_path = database.parent().unwrap().join("timer-widget-result.json");
    let owner = app.clone();
    let report = json!({"pass":true,"singleTimer":true,"nativeDrag":true,"nativeResize":true,"resizeEdges":hits,
        "topmostTogglesWithoutRecreate":true,"toolWindow":true,"transparentPixel":{"behind":behind,"through":through},
        "minimizedMainKeepsTicking":true,"geometryRestored":true,
        "explicitExitClosesBothWindows":true,"exitDraftDurable":true,"restoreAfterExit":true});
    cx.update(|cx| {
        cx.on_window_closed(move |cx, _| {
            if !cx.windows().is_empty() {
                return;
            }
            let app = owner.read(cx);
            assert!(app.close_allowed && app.timer_widget.is_none());
            assert!(app.draft_close_drain.is_none());
            assert!(
                crate::timer_widget_window::WidgetWindowState::load(&path)
                    .unwrap()
                    .open
            );
            let stored: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&disk).unwrap()).unwrap();
            assert_eq!(stored["draft"]["content"], marker);
            std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        })
        .detach()
    });
    main.update(cx, |_, window, cx| {
        app.update(cx, |app, cx| {
            app.timer_action(TimerCardAction::ExitApp, window, cx)
        });
    })
    .unwrap();
}
