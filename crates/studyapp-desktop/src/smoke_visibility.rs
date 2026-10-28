//! Actual HWND presentation and Windows input checks. DOM queries and CDP
//! screenshots alone cannot detect a hidden or occluded native child window.
use crate::{
    app::{Page, StudyApp},
    markdown_editor::MarkdownEditor,
};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::Value;
use std::{
    cell::Cell,
    path::Path,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HWND, POINT},
    UI::{
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::{
            GetForegroundWindow, HWND_NOTOPMOST, HWND_TOPMOST, IsChild, SW_SHOWNORMAL,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetCursorPos, SetForegroundWindow,
            SetWindowPos, ShowWindow, WindowFromPoint,
        },
    },
};

async fn pause(cx: &AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(50))
        .await;
}

async fn snapshot(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    cx: &mut AsyncApp,
) -> Value {
    window
        .update(cx, |_, _, cx| {
            editor
                .read(cx)
                .test(serde_json::json!({
                    "type": "snapshot", "request": "native-visibility-state"
                }))
                .unwrap();
        })
        .unwrap();
    for _ in 0..100 {
        pause(cx).await;
        if let Some(value) = window
            .update(cx, |_, _, cx| {
                editor.update(cx, |editor, _| {
                    editor.test_results.remove("native-visibility-state")
                })
            })
            .unwrap()
        {
            return value;
        }
    }
    panic!("native visibility snapshot timed out");
}

fn parent(state: &Value) -> HWND {
    HWND(state["parentHwnd"].as_u64().unwrap() as usize as *mut _)
}

fn activate(state: &Value) {
    let hwnd = parent(state);
    // Only the current isolated smoke process's captured parent HWND is used.
    // Hidden startup avoids a console flash; visible testing is explicit here.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
        .unwrap();
        let _ = SetForegroundWindow(hwnd);
    }
}

#[derive(Clone, Copy)]
enum CaptureScope {
    Editor,
    Window,
}

fn usable_viewport(state: &Value, scope: CaptureScope) -> bool {
    let own_window = state["parentHwnd"]
        .as_u64()
        .is_some_and(|parent| parent != 0 && state["hitRootHwnd"].as_u64() == Some(parent));
    state["visible"] == true
        && state["controllerVisible"] == true
        && state["insideParent"] == true
        && own_window
        && (matches!(scope, CaptureScope::Window) || state["hitIsEditor"] == true)
        && state["bounds"]["width"].as_i64().unwrap_or(0) > 200
        && state["bounds"]["height"].as_i64().unwrap_or(0) > 120
}

pub async fn visible(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    cx: &mut AsyncApp,
) -> Value {
    visible_in_scope(window, editor, CaptureScope::Editor, cx).await
}

async fn visible_in_scope(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    scope: CaptureScope,
    cx: &mut AsyncApp,
) -> Value {
    for attempt in 0..240 {
        pause(cx).await;
        let state = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let editor = editor.read(cx);
                assert!(editor.error().is_none(), "{:?}", editor.error());
                editor.ready().then(|| editor.native_state().unwrap())
            })
            .unwrap();
        if let Some(state) = state {
            activate(&state);
            if usable_viewport(&state, scope) {
                if state["foregroundHwnd"] != state["parentHwnd"] {
                    // Foreground lock can reject SetForegroundWindow for a
                    // background-launched test. A verified click on our own
                    // visible child activates it as a user interaction.
                    // A whole-window screenshot may intentionally include a
                    // notification over the editor. Never click through it to
                    // acquire focus; wait for activation to be reflected.
                    if matches!(scope, CaptureScope::Editor) {
                        click_body(&state, cx).await;
                    }
                    continue;
                }
                return state;
            }
            if attempt == 239 {
                eprintln!("native failed viewport: {state}");
                let path = std::env::current_dir()
                    .unwrap()
                    .join(".scratch/native-first-failure.bmp");
                // A collapsed/hidden child has no screenshot target. Preserve
                // the actual viewport failure instead of replacing it with a
                // WindowFromPoint assertion about another application.
                if state["visible"] == true
                    && state["bounds"]["width"].as_i64().unwrap_or(0) > 0
                    && state["bounds"]["height"].as_i64().unwrap_or(0) > 0
                {
                    capture_screen(&state, &path);
                }
                panic!("native editor is not visibly usable: {state}");
            }
        } else {
            assert!(attempt < 239, "editor did not become ready");
        }
    }
    unreachable!()
}

/// Move the OS pointer, so native WM_MOUSEMOVE and delayed GPUI tooltips agree.
pub async fn hover(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    id: SharedString,
    cx: &mut AsyncApp,
) {
    let state = visible(window, editor, cx).await;
    let center = window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).bounds().center()
        })
        .unwrap();
    let scale = state["logicalBounds"][4].as_f64().unwrap() / 96.0;
    let left = state["parentBounds"]["x"].as_i64().unwrap() as i32;
    let top = state["parentBounds"]["y"].as_i64().unwrap() as i32;
    let width = state["parentBounds"]["width"].as_i64().unwrap() as i32;
    let height = state["parentBounds"]["height"].as_i64().unwrap() as i32;
    let x = left + (f64::from(f32::from(center.x)) * scale).round() as i32;
    let y = top + (f64::from(f32::from(center.y)) * scale).round() as i32;
    assert!(
        (left..left + width).contains(&x) && (top..top + height).contains(&y),
        "hover point is outside the test window: ({x}, {y}), {state}"
    );
    let parent = parent(&state);
    let target = unsafe { WindowFromPoint(POINT { x, y }) };
    assert!(
        target == parent || unsafe { IsChild(parent, target).as_bool() },
        "hover point belongs to another window: ({x}, {y}), {state}"
    );
    assert_eq!(unsafe { GetForegroundWindow() }, parent);
    unsafe { SetCursorPos(x, y).unwrap() };
    pause(cx).await;
}

/// Verify the OS cursor, not only a style value in the GPUI tree.
pub async fn assert_cursor(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    id: SharedString,
    expected: CursorStyle,
    cx: &mut AsyncApp,
) {
    use windows::Win32::UI::WindowsAndMessaging::{
        CURSORINFO, GetCursor, GetCursorInfo, IDC_ARROW, IDC_HAND, IDC_SIZEWE, LoadCursorW,
    };
    hover(window, editor, id.clone(), cx).await;
    let resource = match expected {
        CursorStyle::PointingHand => IDC_HAND,
        CursorStyle::Arrow => IDC_ARROW,
        CursorStyle::ResizeLeftRight | CursorStyle::ResizeColumn => IDC_SIZEWE,
        _ => panic!("unsupported smoke cursor expectation"),
    };
    let expected = unsafe { LoadCursorW(None, resource).unwrap() };
    for attempt in 0..20 {
        pause(cx).await;
        let mut cursor = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetCursorInfo(&mut cursor).unwrap() };
        // GetCursor reads the current UI thread's actual Win32 cursor. A
        // disconnected runner desktop can return a null global cursor even
        // after WM_MOUSEMOVE and SetCursor have updated this thread. Never
        // replace the OS check with the declared GPUI style or set the expected
        // cursor from the test; a wrong or null thread cursor must still fail.
        let thread_cursor = unsafe { GetCursor() };
        if thread_cursor == expected && (cursor.hCursor.0.is_null() || cursor.hCursor == expected) {
            eprintln!(
                "native cursor verified over {id}: thread={thread_cursor:?}, global={:?}, flags={:?}",
                cursor.hCursor, cursor.flags
            );
            return;
        }
        assert!(
            attempt < 19,
            "unexpected OS cursor over {id}: thread={thread_cursor:?}, global={:?}, flags={:?}, expected={expected:?}",
            cursor.hCursor,
            cursor.flags
        );
    }
}

pub(crate) fn send(state: &Value, inputs: &[INPUT]) {
    // Never deliver input to an unrelated foreground application.
    assert_eq!(unsafe { GetForegroundWindow() }, parent(state));
    assert_eq!(
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) },
        inputs.len() as u32
    );
}

fn key(code: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: code,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

pub(crate) async fn click_body(state: &Value, cx: &AsyncApp) {
    // Activation and caret-placement are separate single-click scenarios. Real
    // Windows input combines consecutive clicks within the system interval into
    // a word-selecting double click, so pace the fixture against that interval.
    thread_local! {
        static LAST_BODY_CLICK: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    let interval = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }) + 50);
    if let Some(remaining) = LAST_BODY_CLICK.with(|last| {
        last.get()
            .and_then(|clicked| interval.checked_sub(clicked.elapsed()))
    }) {
        cx.background_executor().timer(remaining).await;
    }
    let x = state["bounds"]["x"].as_i64().unwrap() as i32 + 75;
    let y = state["bounds"]["y"].as_i64().unwrap() as i32 + 105;
    let child = HWND(state["hwnd"].as_u64().unwrap() as usize as *mut _);
    let target = unsafe { WindowFromPoint(POINT { x, y }) };
    assert!(
        target == child || unsafe { IsChild(child, target).as_bool() },
        "mouse target is not our test editor: {state}"
    );
    unsafe { SetCursorPos(x, y).unwrap() };
    let inputs = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                ..Default::default()
            },
        },
    });
    assert_eq!(
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) },
        inputs.len() as u32
    );
    LAST_BODY_CLICK.with(|last| last.set(Some(Instant::now())));
}

pub(crate) async fn assert_click_focused(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    cx: &mut AsyncApp,
) {
    // WebView focus is delivered asynchronously across its renderer process.
    // Wait for the original real click; never set focus or synthesize another.
    for attempt in 0..20 {
        pause(cx).await;
        let focus = window
            .update(cx, |_, _, cx| editor.read(cx).native_state().unwrap())
            .unwrap();
        if focus["focused"] == true {
            return;
        }
        assert!(attempt < 19, "click did not focus editor: {focus}");
    }
}

pub(crate) fn type_text(state: &Value, text: &str) {
    let inputs: Vec<_> = text
        .encode_utf16()
        .flat_map(|unit| {
            [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP].map(|flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wScan: unit,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            })
        })
        .collect();
    send(state, &inputs);
}

fn cdp_character_input() -> bool {
    std::env::var("WW_STUDYAPP_SMOKE_CDP_INPUT").as_deref() == Ok("1")
}

/// The alternate mode is explicit, test-only, and never a SendInput fallback.
async fn enter_characters(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    state: &Value,
    text: &str,
    cx: &mut AsyncApp,
) {
    if !cdp_character_input() {
        eprintln!("smoke character input mode: strict Windows SendInput");
        type_text(state, text);
        return;
    }
    eprintln!(
        "smoke character input mode: explicit CDP Input.insertText; OS character keyboard input is NOT verified"
    );
    let result = window
        .update(cx, |_, _, cx| editor.read(cx).insert_text(text).unwrap())
        .unwrap();
    for _ in 0..100 {
        pause(cx).await;
        match result.try_recv() {
            Ok(result) => {
                result.expect("explicit smoke CDP character input failed");
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("explicit smoke CDP character input callback disconnected")
            }
        }
    }
    panic!("explicit smoke CDP character input timed out");
}

/// Capture screen pixels of this foreground editor. Unlike CDP or GPUI GPU
/// captures, this includes the native child as the desktop actually displays it.
fn capture_screen(state: &Value, path: &Path) -> usize {
    use windows::Win32::Graphics::Gdi::*;
    let x = state["bounds"]["x"].as_i64().unwrap() as i32;
    let y = state["bounds"]["y"].as_i64().unwrap() as i32;
    let width = state["bounds"]["width"].as_i64().unwrap() as i32;
    let height = state["bounds"]["height"].as_i64().unwrap() as i32;
    let child = HWND(state["hwnd"].as_u64().unwrap() as usize as *mut _);
    let target = unsafe {
        WindowFromPoint(POINT {
            x: x + width / 2,
            y: y + height / 2,
        })
    };
    assert!(
        target == child || unsafe { IsChild(child, target).as_bool() },
        "screenshot target is obscured by another application"
    );
    let length = (width * height * 4) as usize;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // Resources are paired in this scope before the captured bytes leave it.
    let pixels = unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .expect("native screenshot bitmap");
        let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
        let result = BitBlt(memory, 0, 0, width, height, Some(screen), x, y, SRCCOPY);
        let pixels = std::slice::from_raw_parts(bits as *const u8, length).to_vec();
        let _ = SelectObject(memory, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        result.expect("native screen copy");
        pixels
    };
    let mut bmp = Vec::with_capacity(54 + length);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54 + length) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54_u32.to_le_bytes());
    bmp.extend_from_slice(&40_u32.to_le_bytes());
    bmp.extend_from_slice(&width.to_le_bytes());
    bmp.extend_from_slice(&(-height).to_le_bytes());
    bmp.extend_from_slice(&1_u16.to_le_bytes());
    bmp.extend_from_slice(&32_u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    bmp.extend_from_slice(&pixels);
    std::fs::write(path, bmp).unwrap();
    // White/transparent-only child regions previously passed CDP checks. The
    // fixture has several visible text lines below the toolbar.
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .skip(width as usize * 65)
        .filter(|pixel| pixel[0] < 180 && pixel[1] < 180 && pixel[2] < 180)
        .count()
}

async fn capture_visible(state: &Value, path: &Path, cx: &AsyncApp) {
    // Readiness only acknowledges JavaScript. Give desktop presentation its own
    // bounded condition wait; no arbitrary sleep can turn blank pixels green.
    for attempt in 0..60 {
        pause(cx).await;
        if capture_screen(state, path) > 200 {
            return;
        }
        assert!(
            attempt < 59,
            "native screenshot has no visible body text: {path:?}"
        );
    }
}

pub async fn capture(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    path: &Path,
    cx: &mut AsyncApp,
) {
    let state = visible(window, editor, cx).await;
    capture_visible(&state, path, cx).await;
}

/// Actual desktop pixels for the full client area, including native children.
pub async fn capture_window(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    path: &Path,
    cx: &mut AsyncApp,
) {
    let mut state = visible_in_scope(window, editor, CaptureScope::Window, cx).await;
    state["bounds"] = state["parentBounds"].clone();
    state["hwnd"] = state["parentHwnd"].clone();
    capture_visible(&state, path, cx).await;
}

pub(crate) async fn focus_body(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    cx: &mut AsyncApp,
) {
    let state = visible(window, editor, cx).await;
    click_body(&state, cx).await;
    assert_click_focused(window, editor, cx).await;
}

pub async fn type_into(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    text: &str,
    cx: &mut AsyncApp,
) {
    let browser = snapshot(window, editor, cx).await;
    assert_eq!(
        browser["readonly"], false,
        "editor is not editable: {browser}"
    );
    let state = visible(window, editor, cx).await;
    click_body(&state, cx).await;
    assert_click_focused(window, editor, cx).await;
    enter_characters(window, editor, &state, text, cx).await;
    for attempt in 0..80 {
        pause(cx).await;
        if window
            .update(cx, |_, _, cx| {
                let editor = editor.read(cx);
                assert!(editor.error().is_none(), "{:?}", editor.error());
                editor.value().contains(text)
            })
            .unwrap()
        {
            return;
        }
        assert!(
            attempt < 79,
            "smoke character input did not reach persistence: {text:?}"
        );
    }
}

pub async fn first_open(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    std::fs::write(
        database.with_file_name("smoke-input-mode.txt"),
        if cdp_character_input() {
            "WEBVIEW2_CDP_INPUT\n"
        } else {
            "WIN32_SENDINPUT\n"
        },
    )
    .unwrap();
    let (editor, original) = window
        .update(cx, |_, window, cx| {
            if std::env::var_os("WW_STUDYAPP_SMOKE_SMALL_WINDOW").is_some() {
                crate::smoke_library::resize(window, size(px(1024.), px(720.)));
            }
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.page = Page::Notes;
                app.selected_note = app
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.notes.first().map(|note| note.id.clone()));
                cx.notify();
            });
            window.render_frame(cx);
            let editor = view.read(cx).note_content.clone();
            let original = editor.read(cx).value().to_owned();
            (editor, original)
        })
        .unwrap();
    let state = visible(window, &editor, cx).await;
    std::fs::write(
        database.with_file_name("native-first-reader.json"),
        serde_json::to_string_pretty(&state).unwrap(),
    )
    .unwrap();
    capture_visible(
        &state,
        &database.with_file_name("native-first-reader.bmp"),
        cx,
    )
    .await;
    // The first fixture is an externally tracked source. A body click must
    // preserve readonly policy; the explicit action creates a managed copy.
    let readonly = snapshot(window, &editor, cx).await;
    assert_eq!(readonly["readonly"], true);
    click_body(&state, cx).await;
    for _ in 0..4 {
        pause(cx).await;
    }
    assert!(
        window
            .update(cx, |_, _, cx| view.read(cx).note_editor.is_none())
            .unwrap(),
        "external source body click entered editing"
    );
    assert_eq!(snapshot(window, &editor, cx).await["readonly"], true);
    window
        .update(cx, |_, window, cx| window.click("edit-note", cx))
        .unwrap();
    crate::smoke_library::until(
        window,
        view,
        "explicit edit creates managed copy draft",
        |app, _| app.note_editor.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx))
        })
        .unwrap();
    let state = visible(window, &editor, cx).await;
    click_body(&state, cx).await;
    assert_click_focused(window, &editor, cx).await;
    let browser = snapshot(window, &editor, cx).await;
    assert_eq!(
        browser["readonly"], false,
        "editor still readonly: {browser}"
    );
    assert_eq!(
        browser["text"], readonly["text"],
        "body click changed document content"
    );
    assert!(
        browser["selection"]["anchor"].as_u64().unwrap() > 0,
        "body click lost its position and put the caret at the start"
    );
    assert_eq!(browser["selection"]["anchor"], browser["selection"]["head"]);
    let insertion = browser["selection"]["head"].as_u64().unwrap() as usize;
    let state = visible(window, &editor, cx).await;
    assert_click_focused(window, &editor, cx).await;
    enter_characters(window, &editor, &state, "native_keyboard_🧠", cx).await;
    for attempt in 0..80 {
        pause(cx).await;
        let content = window
            .update(cx, |_, _, cx| editor.read(cx).value().to_owned())
            .unwrap();
        if content.contains("native_keyboard_🧠") {
            let prefix = String::from_utf16_lossy(
                &original.encode_utf16().take(insertion).collect::<Vec<_>>(),
            );
            assert!(
                content.starts_with(&format!("{prefix}native_keyboard_🧠")),
                "typed text did not land at restored clicked caret"
            );
            break;
        }
        if attempt == 79 {
            capture_screen(&state, &database.with_file_name("native-input-failure.bmp"));
            let browser = snapshot(window, &editor, cx).await;
            let after = window
                .update(cx, |_, _, cx| editor.read(cx).native_state().unwrap())
                .unwrap();
            let keyboard = unsafe {
                (
                    GetKeyboardLayout(0),
                    GetAsyncKeyState(VK_CONTROL.0 as i32),
                    GetAsyncKeyState(VK_SHIFT.0 as i32),
                    GetAsyncKeyState(VK_MENU.0 as i32),
                    GetAsyncKeyState(VK_LWIN.0 as i32),
                )
            };
            panic!(
                "smoke character input did not reach editable content; cdp_mode={}, native={content:?}, browser={browser}, after={after}, keyboard={keyboard:?}",
                cdp_character_input()
            );
        }
    }
    capture_visible(
        &state,
        &database.with_file_name("native-first-editor.bmp"),
        cx,
    )
    .await;
    // Native input may form several history groups depending on event timing.
    // Keep exercising the real shortcut until all test-only input is undone.
    // An intermediate half-surrogate may intentionally leave canonical text
    // unchanged until the next group repairs it, so use a bounded overall wait.
    let mut undo_states = Vec::new();
    for attempt in 0..32 {
        let current = window
            .update(cx, |_, _, cx| {
                let editor = editor.read(cx);
                assert!(editor.error().is_none(), "{:?}", editor.error());
                editor.value().to_owned()
            })
            .unwrap();
        if current == original {
            break;
        }
        undo_states.push(current);
        assert!(
            attempt < 31,
            "native undo did not restore original Markdown: expected={original:?}, history={undo_states:?}"
        );
        if cdp_character_input() {
            window
                .update(cx, |_, _, cx| {
                    editor
                        .read(cx)
                        .test(serde_json::json!({
                            "type": "command", "name": "undo", "request": "explicit-smoke-undo"
                        }))
                        .unwrap();
                })
                .unwrap();
        } else {
            send(
                &state,
                &[
                    key(VK_CONTROL, KEYBD_EVENT_FLAGS::default()),
                    key(VK_Z, KEYBD_EVENT_FLAGS::default()),
                    key(VK_Z, KEYEVENTF_KEYUP),
                    key(VK_CONTROL, KEYEVENTF_KEYUP),
                ],
            );
        }
        pause(cx).await;
        pause(cx).await;
    }
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::editor_action(window, view, true, cx);
            window.click("confirm-note-action", cx);
        })
        .unwrap();
    for attempt in 0..80 {
        pause(cx).await;
        if window
            .update(cx, |_, _, cx| {
                let app = view.read(cx);
                app.note_editor.is_none() && app.draft_discard.is_none()
            })
            .unwrap()
        {
            break;
        }
        assert!(
            attempt < 79,
            "native input check did not finish discarding its test draft"
        );
    }
    let state = visible(window, &editor, cx).await;
    unsafe {
        SetWindowPos(
            parent(&state),
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
        .unwrap();
    }
    if cdp_character_input() {
        eprintln!(
            "native first-open reading, click focus and undo passed; characters explicitly used CDP, OS character keyboard input was NOT verified"
        );
    } else {
        eprintln!("native first-open reading, click focus, keyboard input and undo passed");
    }
}

/// Automatic click-to-edit applies only to a managed current note. Keep this
/// separate from first_open's external-source policy and input/undo fixture.
pub(crate) async fn managed_body_edit(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    editor: &Entity<MarkdownEditor>,
    cx: &mut AsyncApp,
) {
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx))
        })
        .unwrap();
    let native = visible(window, editor, cx).await;
    let readonly = snapshot(window, editor, cx).await;
    assert_eq!(readonly["readonly"], true);
    assert_eq!(
        readonly["canRequestEdit"], true,
        "managed current note did not allow click-to-edit"
    );
    click_body(&native, cx).await;
    crate::smoke_library::until(
        window,
        view,
        "managed body click starts draft",
        |app, _| app.note_editor.is_some(),
        cx,
    )
    .await;
    let editable = snapshot(window, editor, cx).await;
    assert_eq!(editable["readonly"], false);
    assert_eq!(editable["text"], readonly["text"]);
    assert_eq!(
        editable["selection"]["head"], readonly["bodyClickPosition"],
        "managed edit lost the actual readonly click position"
    );
    assert_eq!(
        editable["selection"]["anchor"],
        editable["selection"]["head"]
    );
    assert!(editable["selection"]["head"].as_u64().unwrap() > 0);
    assert_click_focused(window, editor, cx).await;
}

#[cfg(test)]
mod viewport_tests {
    use super::{CaptureScope, usable_viewport};
    use serde_json::json;

    #[test]
    fn window_capture_accepts_own_notification_but_rejects_foreign_occlusion() {
        // Native CI state with two notification cards clipping the editor's
        // center. Its remaining pixels and the application window are visible.
        let mut state = json!({
            "visible": true, "controllerVisible": true, "insideParent": true,
            "hitIsEditor": false, "hitRootHwnd": 196950, "parentHwnd": 196950,
            "bounds": {"width": 548, "height": 629}
        });
        assert!(usable_viewport(&state, CaptureScope::Window));
        assert!(!usable_viewport(&state, CaptureScope::Editor));
        state["hitRootHwnd"] = json!(42);
        assert!(!usable_viewport(&state, CaptureScope::Window));
        state["hitRootHwnd"] = state["parentHwnd"].clone();
        state["hitIsEditor"] = json!(true);
        assert!(usable_viewport(&state, CaptureScope::Editor));
        state["controllerVisible"] = json!(false);
        assert!(!usable_viewport(&state, CaptureScope::Window));
        assert!(!usable_viewport(&state, CaptureScope::Editor));
    }
}
