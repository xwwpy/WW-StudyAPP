//! Isolated Win32 character delivery across WebView and GPUI focus boundaries.
//! Diagnostic evidence precedes assertions; no focus correction is performed.
mod ime;

use crate::{
    app::{Page, StudyApp},
    markdown_editor::MarkdownEditor,
    settings_ui::{AppearancePage, SettingsSection, StoragePage},
    smoke_library::until,
    smoke_visibility::{assert_click_focused, click_body, send, type_text, visible},
};
use gpui_kit::{
    component::{WindowExt, input::InputState},
    test::TestWindowExt,
    *,
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use studyapp_core::Command;
use windows::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::ClientToScreen,
    UI::{
        Input::KeyboardAndMouse::{
            GetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
            KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL,
            MOUSEINPUT, VIRTUAL_KEY, VK_A, VK_BACK, VK_C, VK_CONTROL, VK_END, VK_LEFT, VK_RIGHT,
            VK_SHIFT, VK_TAB, VK_V, VK_Z,
        },
        WindowsAndMessaging::{GetForegroundWindow, SetCursorPos, WindowFromPoint},
    },
};

// Read-only diagnostics from the app's own UI thread. A present IMM context
// does not by itself prove that a Chinese IME or a TSF profile is active.
#[link(name = "user32")]
unsafe extern "system" {
    fn GetKeyboardLayout(thread: u32) -> isize;
}
#[link(name = "imm32")]
unsafe extern "system" {
    fn ImmGetContext(hwnd: isize) -> isize;
    fn ImmReleaseContext(hwnd: isize, context: isize) -> i32;
    fn ImmIsIME(layout: isize) -> i32;
}

async fn tick(cx: &AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(50))
        .await;
}

fn persist(database: &Path, evidence: &[Value]) {
    std::fs::write(
        database.with_file_name("native-inputs-evidence.json"),
        serde_json::to_vec_pretty(evidence).unwrap(),
    )
    .unwrap();
}

fn record(
    window: AnyWindowHandle,
    editor: &Entity<MarkdownEditor>,
    input: &Entity<InputState>,
    stage: &str,
    evidence: &mut Vec<Value>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let state = window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let parent = editor.read(cx).native_state().unwrap()["parentHwnd"]
                .as_u64()
                .unwrap() as isize;
            let layout = unsafe { GetKeyboardLayout(0) };
            let context = unsafe { ImmGetContext(parent) };
            if context != 0 {
                unsafe { ImmReleaseContext(parent, context) };
            }
            let logical_owner = window.focused_input(cx).map(|owner| {
                owner
                    .as_input()
                    .is_some_and(|owner| owner.entity_id() == input.entity_id())
            });
            json!({
                "stage": stage,
                "targetEntity": format!("{:?}", input.entity_id()),
                "targetEditable": input.read(cx).is_editable(),
                "targetFocused": input.focus_handle(cx).is_focused(window),
                "focusedInputMatchesTarget": logical_owner,
                "value": input.read(cx).value().as_str(),
                "focusHwnd": unsafe { GetFocus() }.0 as usize,
                "foregroundHwnd": unsafe { GetForegroundWindow() }.0 as usize,
                "keyboardLayout": format!("{layout:#x}"),
                "activeLayoutIsImmIme": unsafe { ImmIsIME(layout) } != 0,
                "parentImmContextPresent": context != 0,
                "native": editor.read(cx).native_state().unwrap(),
                "editorText": editor.read(cx).value(),
            })
        })
        .unwrap();
    evidence.push(state);
    persist(database, evidence);
}

/// Click the real parent HWND at the input's center. Neither GPUI focus() nor
/// Win32 SetFocus is used: their disagreement is what this scenario measures.
fn click_input(
    window: AnyWindowHandle,
    input: &Entity<InputState>,
    native: &Value,
    cx: &mut AsyncApp,
) {
    click_entity(window, input.entity_id(), native, cx);
}

fn click_entity(window: AnyWindowHandle, input_id: EntityId, native: &Value, cx: &mut AsyncApp) {
    let center = window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.find(("input", input_id)).bounds().center()
        })
        .unwrap();
    let parent = HWND(native["parentHwnd"].as_u64().unwrap() as usize as *mut _);
    let scale = native["logicalBounds"][4].as_f64().unwrap() / 96.;
    let mut position = POINT {
        x: (f64::from(f32::from(center.x)) * scale).round() as i32,
        y: (f64::from(f32::from(center.y)) * scale).round() as i32,
    };
    assert!(unsafe { ClientToScreen(parent, &mut position) }.as_bool());
    assert_eq!(unsafe { GetForegroundWindow() }, parent);
    assert_eq!(
        unsafe { WindowFromPoint(position) },
        parent,
        "input is occluded by a native child or unrelated window"
    );
    unsafe { SetCursorPos(position.x, position.y).unwrap() };
    let events = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                ..Default::default()
            },
        },
    });
    send(native, &events);
}

fn wheel_at_input(
    window: AnyWindowHandle,
    input: &Entity<InputState>,
    native: &Value,
    delta: i32,
    cx: &mut AsyncApp,
) {
    let center = window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.find(("input", input.entity_id())).bounds().center()
        })
        .unwrap();
    let parent = HWND(native["parentHwnd"].as_u64().unwrap() as usize as *mut _);
    let scale = native["logicalBounds"][4].as_f64().unwrap() / 96.;
    let mut position = POINT {
        x: (f64::from(f32::from(center.x)) * scale).round() as i32,
        y: (f64::from(f32::from(center.y)) * scale).round() as i32,
    };
    assert!(unsafe { ClientToScreen(parent, &mut position) }.as_bool());
    assert_eq!(unsafe { GetForegroundWindow() }, parent);
    assert_eq!(
        unsafe { WindowFromPoint(position) },
        parent,
        "virtual-row input is not visibly over the GPUI parent"
    );
    unsafe { SetCursorPos(position.x, position.y).unwrap() };
    wheel_at_pointer(native, delta);
}

fn wheel_at_pointer(native: &Value, delta: i32) {
    send(
        native,
        &[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: delta as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    ..Default::default()
                },
            },
        }],
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

fn press(native: &Value, code: VIRTUAL_KEY) {
    send(
        native,
        &[key(code, KEYBD_EVENT_FLAGS(0)), key(code, KEYEVENTF_KEYUP)],
    );
}

fn chord(native: &Value, modifier: VIRTUAL_KEY, code: VIRTUAL_KEY) {
    send(
        native,
        &[
            key(modifier, KEYBD_EVENT_FLAGS(0)),
            key(code, KEYBD_EVENT_FLAGS(0)),
            key(code, KEYEVENTF_KEYUP),
            key(modifier, KEYEVENTF_KEYUP),
        ],
    );
}

async fn settle(cx: &AsyncApp) {
    for _ in 0..8 {
        tick(cx).await;
    }
}

struct ProbeContext<'a> {
    window: AnyWindowHandle,
    editor: &'a Entity<MarkdownEditor>,
    native: &'a Value,
    database: &'a Path,
}

async fn probe(
    name: &str,
    target: &Entity<InputState>,
    expected: &str,
    context: &ProbeContext<'_>,
    evidence: &mut Vec<Value>,
    failures: &mut Vec<String>,
    cx: &mut AsyncApp,
) {
    let window = context.window;
    let editor = context.editor;
    let native = context.native;
    let database = context.database;
    let body = window
        .update(cx, |_, _, cx| editor.read(cx).value().to_owned())
        .unwrap();
    record(
        window,
        editor,
        target,
        &format!("{name}:before-click"),
        evidence,
        database,
        cx,
    );
    click_input(window, target, native, cx);
    settle(cx).await;
    record(
        window,
        editor,
        target,
        &format!("{name}:after-click"),
        evidence,
        database,
        cx,
    );
    type_text(native, expected);
    settle(cx).await;
    record(
        window,
        editor,
        target,
        &format!("{name}:after-type"),
        evidence,
        database,
        cx,
    );
    let passed = window
        .update(cx, |_, window, cx| {
            target.read(cx).is_editable()
                && target.focus_handle(cx).is_focused(window)
                && target.read(cx).value().as_str() == expected
                && editor.read(cx).value() == body
                && unsafe { GetFocus() }.0 as usize
                    == native["parentHwnd"].as_u64().unwrap() as usize
        })
        .unwrap();
    if !passed {
        failures.push(name.to_owned());
    }
}

async fn prepare_input_fixture(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) -> (
    Entity<MarkdownEditor>,
    Entity<InputState>,
    Entity<InputState>,
) {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let runner_temp = std::env::var_os("RUNNER_TEMP").expect("isolated runner temp required");
    assert!(
        database.starts_with(&runner_temp),
        "input fixture must be under RUNNER_TEMP"
    );
    assert_ne!(
        std::env::var("WW_STUDYAPP_SMOKE_CDP_INPUT").as_deref(),
        Ok("1"),
        "input diagnostics require real Windows character delivery"
    );
    until(
        window,
        view,
        "input fixture startup",
        |app, _| app.snapshot.is_some() && !app.draft_loading && app.preferences_ui.saved.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            view.update(cx, |app, cx| {
                app.send(
                    Command::CreateNote {
                        title: "Input boundary fixture".into(),
                        content: "# Input boundary fixture\n\nNative focus handoff.\n".into(),
                    },
                    cx,
                );
            });
        })
        .unwrap();
    until(
        window,
        view,
        "managed input fixture",
        |app, _| {
            app.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .notes
                    .iter()
                    .any(|note| note.managed && note.title == "Input boundary fixture")
            })
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::resize(window, size(px(1024.), px(720.)));
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.selected_note = app
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .notes
                    .iter()
                    .find(|note| note.title == "Input boundary fixture")
                    .map(|note| note.id.clone());
                app.clear_notices(cx);
                cx.notify();
            });
            window.render_frame(cx);
            let app = view.read(cx);
            (
                app.note_content.clone(),
                app.search.clone(),
                app.interval.clone(),
            )
        })
        .unwrap()
}

pub async fn exercise_ime(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let (editor, _, _) = prepare_input_fixture(window, view, database, cx).await;
    let native = visible(window, &editor, cx).await;
    ime::exercise(window, view, &native, database, cx)
        .await
        .expect("real Pinyin IME acceptance failed; see native-ime-evidence.json");
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let (editor, search, interval) = prepare_input_fixture(window, view, database, cx).await;
    let mut evidence = Vec::new();
    let mut failures = Vec::new();
    // A search field on the same page keeps the native child visible. This is
    // independent of the page-switch case below, which hides that child.
    for (name, input, expected) in [
        ("same-page-search", search.clone(), "input_37_中文"),
        ("settings-number", interval.clone(), "37"),
    ] {
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |app, cx| {
                    app.page = Page::Notes;
                    app.clear_notices(cx);
                    app.search
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    cx.notify();
                });
                window.render_frame(cx);
            })
            .unwrap();
        let native = visible(window, &editor, cx).await;
        click_body(&native, cx).await;
        assert_click_focused(window, &editor, cx).await;
        record(
            window,
            &editor,
            &input,
            &format!("{name}:webview-focused"),
            &mut evidence,
            database,
            cx,
        );
        let original_editor = window
            .update(cx, |_, _, cx| editor.read(cx).value().to_owned())
            .unwrap();
        if name == "settings-number" {
            window
                .update(cx, |_, window, cx| {
                    // Programmatic navigation sets up the page without repairing
                    // native focus through window activation or a second click.
                    view.update(cx, |app, cx| {
                        app.navigate(Page::Settings, window, cx);
                    });
                    window.render_frame(cx);
                })
                .unwrap();
            // Leaving an open editor flushes and retains its draft asynchronously,
            // even when no text changed. Wait for real navigation before looking
            // up the settings input; never repair native focus in this setup.
            until(
                window,
                view,
                "input settings navigation",
                |app, _| app.page == Page::Settings && !app.note_saving,
                cx,
            )
            .await;
            window
                .update(cx, |_, window, cx| {
                    view.update(cx, |app, cx| {
                        app.select_settings_section(SettingsSection::Learning, cx);
                    });
                    window.render_frame(cx);
                })
                .unwrap();
        }
        window
            .update(cx, |_, window, cx| {
                input.update(cx, |input, cx| input.set_value("", window, cx));
                window.render_frame(cx);
            })
            .unwrap();
        record(
            window,
            &editor,
            &input,
            &format!("{name}:before-click"),
            &mut evidence,
            database,
            cx,
        );
        click_input(window, &input, &native, cx);
        for _ in 0..4 {
            tick(cx).await;
        }
        record(
            window,
            &editor,
            &input,
            &format!("{name}:after-click"),
            &mut evidence,
            database,
            cx,
        );
        type_text(&native, expected);
        for _ in 0..20 {
            tick(cx).await;
        }
        record(
            window,
            &editor,
            &input,
            &format!("{name}:after-type"),
            &mut evidence,
            database,
            cx,
        );
        let passed = window
            .update(cx, |_, window, cx| {
                input.focus_handle(cx).is_focused(window)
                    && input.read(cx).is_editable()
                    && input.read(cx).value().as_str() == expected
                    && editor.read(cx).value() == original_editor
                    && unsafe { GetFocus() }.0 as usize
                        == native["parentHwnd"].as_u64().unwrap() as usize
            })
            .unwrap();
        if !passed {
            failures.push(name.to_owned());
        }
    }

    // Re-enter two existing entities after their pages unmount. Neither value
    // is restored by the test after re-entry; this checks draft retention.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
    let native = visible(window, &editor, cx).await;
    click_body(&native, cx).await;
    assert_click_focused(window, &editor, cx).await;
    let probe_context = ProbeContext {
        window,
        editor: &editor,
        native: &native,
        database,
    };
    probe(
        "notes-search-reentry-seed",
        &search,
        "retained",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Settings;
                app.select_settings_section(SettingsSection::Learning, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    let search_entity = search.entity_id();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
    evidence.push(json!({"stage":"notes-search-reentry:retained",
        "sameEntity": window.update(cx, |_, _, cx| view.read(cx).search.entity_id() == search_entity).unwrap(),
        "value": window.update(cx, |_, _, cx| search.read(cx).value().to_string()).unwrap()}));
    persist(database, &evidence);
    if window
        .update(cx, |_, _, cx| {
            search.read(cx).value().as_str() != "retained"
                || view.read(cx).search.entity_id() != search_entity
        })
        .unwrap()
    {
        failures.push("notes-search-reentry-retention".into());
    }
    window
        .update(cx, |_, window, cx| {
            search.update(cx, |input, cx| input.set_value("", window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    probe(
        "notes-search-reentry",
        &search,
        "more",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;

    // The Learning controls give a stable text/number family and adjacent Tab
    // route. Each key is delivered by Windows, never by GPUI test dispatch.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Settings;
                app.select_settings_section(SettingsSection::Learning, cx);
                app.deadline
                    .update(cx, |input, cx| input.set_value("", window, cx));
                app.review_days
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    let (deadline, review_days) = window
        .update(cx, |_, _, cx| {
            (
                view.read(cx).deadline.clone(),
                view.read(cx).review_days.clone(),
            )
        })
        .unwrap();
    probe(
        "learning-deadline",
        &deadline,
        "12",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    probe(
        "learning-review-days",
        &review_days,
        "2, 4",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            interval.update(cx, |input, cx| input.set_value("", window, cx));
            deadline.update(cx, |input, cx| input.set_value("", window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    click_input(window, &interval, &native, cx);
    settle(cx).await;
    press(&native, VK_TAB);
    settle(cx).await;
    let tab_target = window
        .update(cx, |_, window, cx| {
            view.read(cx).deadline.focus_handle(cx).is_focused(window)
        })
        .unwrap();
    record(
        window,
        &editor,
        &deadline,
        "learning-tab:after-navigation",
        &mut evidence,
        database,
        cx,
    );
    type_text(&native, "3");
    settle(cx).await;
    chord(&native, VK_SHIFT, VK_TAB);
    settle(cx).await;
    record(
        window,
        &editor,
        &interval,
        "learning-shift-tab:after-navigation",
        &mut evidence,
        database,
        cx,
    );
    let shift_target = window
        .update(cx, |_, window, cx| {
            interval.focus_handle(cx).is_focused(window)
                && deadline.read(cx).value().as_str() == "3"
        })
        .unwrap();
    type_text(&native, "4");
    settle(cx).await;
    record(
        window,
        &editor,
        &interval,
        "learning-shift-tab:after-type",
        &mut evidence,
        database,
        cx,
    );
    if !tab_target
        || !shift_target
        || window
            .update(cx, |_, _, cx| interval.read(cx).value().as_str() != "4")
            .unwrap()
    {
        failures.push("learning-tab-shift-tab".into());
    }

    // A recording control is intentionally not a text box. Leaving its page
    // must release interception before a normal search field receives text.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_settings_section(SettingsSection::Shortcuts, cx);
                app.start_shortcut_recording("app.new", false, window, cx);
                app.select_settings_section(SettingsSection::Learning, cx);
                app.interval
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    evidence.push(json!({"stage":"shortcut-recording-exit",
        "recordingCleared": window.update(cx, |_, _, cx| view.read(cx).preferences_ui.recording.is_none()).unwrap()}));
    persist(database, &evidence);
    probe(
        "after-recording-exit",
        &interval,
        "19",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    if window
        .update(cx, |_, _, cx| {
            view.read(cx).preferences_ui.recording.is_some()
        })
        .unwrap()
    {
        failures.push("shortcut-recording-exit".into());
    }

    // Different settings surfaces reuse their input entities across section
    // switches, collapsed directory editors, and cached virtual rows.
    let (settings_search, image_root, theme_name, cloud_endpoint) = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_settings_section(SettingsSection::Storage, cx);
                app.select_storage_page(StoragePage::Images, cx);
                app.preferences_ui.image_directory_expanded = Some(true);
                if let Some(draft) = &mut app.preferences_ui.draft {
                    draft.images.storage = studyapp_core::media_settings::ImageStorage::Cloud;
                }
                if let Some(cloud) = &mut app.preferences_ui.image_cloud {
                    cloud.kind = 0;
                }
                cx.notify();
            });
            window.render_frame(cx);
            let app = view.read(cx);
            (
                app.preferences_ui.search.clone().unwrap(),
                app.preferences_ui.image_root.clone().unwrap(),
                app.preferences_ui.appearance_fields["name"].clone(),
                app.preferences_ui.image_cloud.as_ref().unwrap().fields["endpoint"].clone(),
            )
        })
        .unwrap();
    for input in [&settings_search, &image_root, &cloud_endpoint] {
        window
            .update(cx, |_, window, cx| {
                input.update(cx, |input, cx| input.set_value("", window, cx));
                window.render_frame(cx);
            })
            .unwrap();
    }
    probe(
        "settings-search",
        &settings_search,
        "input",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            settings_search.update(cx, |input, cx| input.set_value("", window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    probe(
        "image-local-directory",
        &image_root,
        "C:\\fixture-images",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.preferences_ui.image_directory_expanded = Some(false);
                cx.notify();
            });
            window.render_frame(cx);
            view.update(cx, |app, cx| {
                app.preferences_ui.image_directory_expanded = Some(true);
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
    evidence.push(json!({"stage":"image-local-directory:expanded-again",
        "value":window.update(cx, |_, _, cx| image_root.read(cx).value().to_string()).unwrap()}));
    persist(database, &evidence);
    if window
        .update(cx, |_, _, cx| {
            image_root.read(cx).value().as_str() != "C:\\fixture-images"
        })
        .unwrap()
    {
        failures.push("image-local-directory-retention".into());
    }
    window
        .update(cx, |_, window, cx| {
            image_root.update(cx, |input, cx| input.set_value("", window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    probe(
        "image-local-directory-reopen",
        &image_root,
        "D:\\new-images",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    probe(
        "image-cloud-endpoint",
        &cloud_endpoint,
        "https://s3.example.test",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_settings_section(SettingsSection::Appearance, cx);
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);
                app.preferences_ui.appearance_fields["name"]
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    probe(
        "appearance-cached-name",
        &theme_name,
        "Native theme",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_appearance_page(AppearancePage::Overview, cx);
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    evidence.push(json!({"stage":"appearance-cached-name:reentry",
        "value":window.update(cx, |_, _, cx| theme_name.read(cx).value().to_string()).unwrap()}));
    persist(database, &evidence);
    if window
        .update(cx, |_, _, cx| {
            theme_name.read(cx).value().as_str() != "Native theme"
        })
        .unwrap()
    {
        failures.push("appearance-cached-name-retention".into());
    }

    // A focused virtual row is intentionally kept mounted by VirtualRows.
    // Click the global search input first to blur it, then scroll with real
    // Windows wheel messages from the name input's visible center.
    click_input(window, &settings_search, &native, cx);
    settle(cx).await;
    let virtual_id = theme_name.entity_id();
    let initially_visible = window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let viewport = window.find("settings-section-scroll").bounds();
            window
                .try_find(("input", virtual_id))
                .is_some_and(|target| viewport.contains(&target.bounds().center()))
        })
        .unwrap();
    evidence.push(json!({"stage":"appearance-virtual-scroll:before-wheel",
        "targetEntity":format!("{virtual_id:?}"), "visible":initially_visible,
        "value":window.update(cx, |_, _, cx| theme_name.read(cx).value().to_string()).unwrap()}));
    persist(database, &evidence);
    if !initially_visible {
        failures.push("appearance-virtual-scroll-initial-visibility".into());
    } else {
        wheel_at_input(window, &theme_name, &native, -120, cx);
        let mut unmounted = false;
        for step in 0..48 {
            tick(cx).await;
            unmounted = window
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    window.try_find(("input", virtual_id)).is_none()
                })
                .unwrap();
            if unmounted {
                break;
            }
            if step % 8 == 7 {
                evidence.push(json!({"stage":"appearance-virtual-scroll:descending",
                    "wheelEvents":step+1, "mounted":true}));
                persist(database, &evidence);
            }
            wheel_at_pointer(&native, -120);
        }
        evidence.push(json!({"stage":"appearance-virtual-scroll:unmounted",
            "unmounted":unmounted,
            "sameEntity":window.update(cx, |_, _, cx| view.read(cx).preferences_ui.appearance_fields["name"].entity_id() == virtual_id).unwrap(),
            "value":window.update(cx, |_, _, cx| theme_name.read(cx).value().to_string()).unwrap()}));
        persist(database, &evidence);
        if !unmounted {
            failures.push("appearance-virtual-scroll-never-unmounted".into());
        }
        let mut remounted = false;
        for step in 0..48 {
            wheel_at_pointer(&native, 120);
            tick(cx).await;
            remounted = window
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    let viewport = window.find("settings-section-scroll").bounds();
                    window
                        .try_find(("input", virtual_id))
                        .is_some_and(|target| viewport.contains(&target.bounds().center()))
                })
                .unwrap();
            if remounted {
                break;
            }
            if step % 8 == 7 {
                evidence.push(json!({"stage":"appearance-virtual-scroll:ascending",
                    "wheelEvents":step+1, "visible":false}));
                persist(database, &evidence);
            }
        }
        evidence.push(json!({"stage":"appearance-virtual-scroll:remounted",
            "remounted":remounted,
            "sameEntity":window.update(cx, |_, _, cx| view.read(cx).preferences_ui.appearance_fields["name"].entity_id() == virtual_id).unwrap(),
            "value":window.update(cx, |_, _, cx| theme_name.read(cx).value().to_string()).unwrap()}));
        persist(database, &evidence);
        if !remounted
            || !window
                .update(cx, |_, _, cx| {
                    view.read(cx).preferences_ui.appearance_fields["name"].entity_id() == virtual_id
                })
                .unwrap()
            || window
                .update(cx, |_, _, cx| {
                    theme_name.read(cx).value().as_str() != "Native theme"
                })
                .unwrap()
        {
            failures.push("appearance-virtual-scroll-retention".into());
        }
        if remounted {
            let body = window
                .update(cx, |_, _, cx| editor.read(cx).value().to_owned())
                .unwrap();
            click_input(window, &theme_name, &native, cx);
            settle(cx).await;
            press(&native, VK_END);
            type_text(&native, "!");
            settle(cx).await;
            record(
                window,
                &editor,
                &theme_name,
                "appearance-virtual-scroll:after-type",
                &mut evidence,
                database,
                cx,
            );
            if !window
                .update(cx, |_, window, cx| {
                    theme_name.focus_handle(cx).is_focused(window)
                        && theme_name.read(cx).is_editable()
                        && theme_name.read(cx).value().as_str() == "Native theme!"
                        && editor.read(cx).value() == body
                        && unsafe { GetFocus() }.0 as usize
                            == native["parentHwnd"].as_u64().unwrap() as usize
                })
                .unwrap()
            {
                failures.push("appearance-virtual-scroll-after-type".into());
            }
        }
    }

    // The HEX input exists only while its palette popup is mounted. The
    // synthetic click here opens the fixture; typing still uses Win32 input.
    let accent = window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("color-open-accent", cx);
            window.render_frame(cx);
            let accent = view.read(cx).preferences_ui.appearance_fields["accent"].clone();
            accent.update(cx, |input, cx| input.set_value("", window, cx));
            accent
        })
        .unwrap();
    probe(
        "appearance-color-popup",
        &accent,
        "#215F99",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_appearance_page(AppearancePage::Overview, cx);
                app.select_appearance_page(AppearancePage::ThemeEditor, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    evidence.push(json!({"stage":"appearance-color-popup:closed-reentry",
        "value":window.update(cx, |_, _, cx| accent.read(cx).value().to_string()).unwrap()}));
    persist(database, &evidence);
    if window
        .update(cx, |_, _, cx| accent.read(cx).value().as_str() != "#215F99")
        .unwrap()
    {
        failures.push("appearance-color-popup-retention".into());
    }

    // Sync URL/account and the disabled automatic interval represent a second
    // settings control family with a distinct entity owner.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.select_settings_section(SettingsSection::Sync, cx)
            });
            let sync_panel = view.read(cx).sync_panel.clone();
            sync_panel.update(cx, |panel, cx| {
                panel.target = 2;
                panel
                    .url
                    .update(cx, |input, cx| input.set_value("", window, cx));
                panel
                    .username
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    let (sync_url, sync_user) = window
        .update(cx, |_, _, cx| {
            let panel = view.read(cx).sync_panel.read(cx);
            (panel.url.clone(), panel.username.clone())
        })
        .unwrap();
    probe(
        "sync-webdav-url",
        &sync_url,
        "https://dav.example.test",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    probe(
        "sync-webdav-user",
        &sync_user,
        "username",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
    click_input(window, &search, &native, cx);
    settle(cx).await;
    type_text(&native, "abcd");
    settle(cx).await;
    press(&native, VK_LEFT);
    press(&native, VK_BACK);
    settle(cx).await;
    record(
        window,
        &editor,
        &search,
        "editing-navigation-backspace",
        &mut evidence,
        database,
        cx,
    );
    let navigation_ok = window
        .update(cx, |_, _, cx| search.read(cx).value().as_str() == "abd")
        .unwrap();
    chord(&native, VK_CONTROL, VK_Z);
    settle(cx).await;
    record(
        window,
        &editor,
        &search,
        "editing-undo",
        &mut evidence,
        database,
        cx,
    );
    let undo_ok = window
        .update(cx, |_, _, cx| search.read(cx).value().as_str() == "abcd")
        .unwrap();
    chord(&native, VK_CONTROL, VK_A);
    type_text(&native, "xyz");
    settle(cx).await;
    record(
        window,
        &editor,
        &search,
        "editing-select-all-replace",
        &mut evidence,
        database,
        cx,
    );
    let selection_ok = window
        .update(cx, |_, _, cx| search.read(cx).value().as_str() == "xyz")
        .unwrap();
    if !navigation_ok || !undo_ok || !selection_ok {
        failures.push("editing-navigation-selection-undo".into());
    }
    // This entire scenario is guarded above by GITHUB_ACTIONS and RUNNER_TEMP.
    // Only the disposable CI desktop receives these OS keystrokes. It checks
    // the GPUI Input path that the WebView clipboard suite cannot cover.
    chord(&native, VK_CONTROL, VK_A);
    chord(&native, VK_CONTROL, VK_C);
    press(&native, VK_RIGHT);
    chord(&native, VK_CONTROL, VK_V);
    settle(cx).await;
    record(
        window,
        &editor,
        &search,
        "editing-gpui-copy-paste",
        &mut evidence,
        database,
        cx,
    );
    if window
        .update(cx, |_, _, cx| search.read(cx).value().as_str() != "xyzxyz")
        .unwrap()
    {
        failures.push("editing-gpui-copy-paste".into());
    }

    // Folder creation is a modal InputState. Fixture setup may open the modal;
    // the assertion still depends on real OS click and character delivery.
    let folder_name = window
        .update(cx, |_, window, cx| {
            let folders = view.read(cx).folders.clone();
            folders.update(cx, |folders, cx| folders.begin_create_folder(window, cx));
            window.render_frame(cx);
            window
                .focused_input(cx)
                .and_then(|input| input.as_input().cloned())
                .expect("folder modal did not expose its text input")
        })
        .unwrap();
    probe(
        "folder-create-modal",
        &folder_name,
        "Input folder",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            let folders = view.read(cx).folders.clone();
            folders.update(cx, |folders, cx| folders.cancel_action(cx));
            window.render_frame(cx);
        })
        .unwrap();
    evidence.push(json!({"stage":"folder-modal-close-search-retention",
        "value":window.update(cx, |_, _, cx| search.read(cx).value().to_string()).unwrap()}));
    persist(database, &evidence);
    if window
        .update(cx, |_, _, cx| search.read(cx).value().as_str() != "xyzxyz")
        .unwrap()
    {
        failures.push("folder-modal-close-search-retention".into());
    }

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.open_editor(crate::app::EditorMode::Manual, window, cx);
                app.start
                    .update(cx, |input, cx| input.set_value("", window, cx));
                app.minutes
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    let (record_start, record_minutes) = window
        .update(cx, |_, _, cx| {
            (view.read(cx).start.clone(), view.read(cx).minutes.clone())
        })
        .unwrap();
    probe(
        "record-start",
        &record_start,
        "2026-10-27 09:30",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    probe(
        "record-minutes",
        &record_minutes,
        "35",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    let summary = window
        .update(cx, |_, _, cx| view.read(cx).summary.clone())
        .unwrap();
    let original_body = window
        .update(cx, |_, _, cx| editor.read(cx).value().to_owned())
        .unwrap();
    click_entity(window, summary.entity_id(), &native, cx);
    settle(cx).await;
    type_text(&native, "summary 中文");
    settle(cx).await;
    let summary_state = window
        .update(cx, |_, window, cx| {
            json!({
                "stage":"record-summary-textarea:after-type",
                "targetEntity":format!("{:?}", summary.entity_id()),
                "targetFocused":summary.focus_handle(cx).is_focused(window),
                "targetEditable":summary.read(cx).is_editable(),
                "value":summary.read(cx).value().as_str(),
                "focusHwnd":unsafe { GetFocus() }.0 as usize,
                "editorText":editor.read(cx).value(),
            })
        })
        .unwrap();
    evidence.push(summary_state);
    persist(database, &evidence);
    if !window
        .update(cx, |_, window, cx| {
            summary.focus_handle(cx).is_focused(window)
                && summary.read(cx).is_editable()
                && summary.read(cx).value().as_str() == "summary 中文"
                && editor.read(cx).value() == original_body
                && unsafe { GetFocus() }.0 as usize
                    == native["parentHwnd"].as_u64().unwrap() as usize
        })
        .unwrap()
    {
        failures.push("record-summary-textarea".into());
    }
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.editor = None;
                app.page = Page::Stats;
                app.record_date
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
    let record_date = window
        .update(cx, |_, _, cx| view.read(cx).record_date.clone())
        .unwrap();
    // The statistics date filter is below the charts on compact desktops.
    // Scroll its actual parent viewport before using the same native click probe.
    let mut date_visible = false;
    for wheel_events in 0..=48 {
        let (target, viewport, offset, maximum) = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let scroll = &view.read(cx).page_scroll;
                (
                    window.find(("input", record_date.entity_id())).bounds(),
                    window.find("page-scroll").bounds(),
                    scroll.offset(),
                    scroll.max_offset(),
                )
            })
            .unwrap();
        let horizontal_fit = target.left() >= viewport.left() + px(8.)
            && target.right() <= viewport.right() - px(8.);
        date_visible = horizontal_fit
            && target.top() >= viewport.top() + px(8.)
            && target.bottom() <= viewport.bottom() - px(8.);
        evidence.push(json!({
            "stage": if wheel_events == 0 { "record-date-filter:before-wheel" }
                else if date_visible { "record-date-filter:after-wheel" }
                else { "record-date-filter:scrolling" },
            "wheelEvents": wheel_events,
            "visible": date_visible,
            "bounds": [f32::from(target.left()), f32::from(target.top()),
                f32::from(target.right()), f32::from(target.bottom())],
            "viewport": [f32::from(viewport.left()), f32::from(viewport.top()),
                f32::from(viewport.right()), f32::from(viewport.bottom())],
            "scrollOffset": [f32::from(offset.x), f32::from(offset.y)],
            "maxOffset": [f32::from(maximum.x), f32::from(maximum.y)],
        }));
        persist(database, &evidence);
        assert!(
            horizontal_fit,
            "date filter is outside its horizontal viewport"
        );
        if date_visible || wheel_events == 48 {
            break;
        }
        // Stay inside the page's left padding, away from nested chart scrollers.
        let parent = HWND(native["parentHwnd"].as_u64().unwrap() as usize as *mut _);
        let scale = native["logicalBounds"][4].as_f64().unwrap() / 96.;
        let mut pointer = POINT {
            x: (f64::from(f32::from(viewport.left() + px(8.))) * scale).round() as i32,
            y: (f64::from(f32::from(viewport.center().y)) * scale).round() as i32,
        };
        assert!(unsafe { ClientToScreen(parent, &mut pointer) }.as_bool());
        assert_eq!(unsafe { GetForegroundWindow() }, parent);
        assert_eq!(
            unsafe { WindowFromPoint(pointer) },
            parent,
            "date-filter scroll viewport is occluded by another native window"
        );
        unsafe { SetCursorPos(pointer.x, pointer.y).unwrap() };
        let delta = if target.top() < viewport.top() + px(8.) {
            120
        } else {
            -120
        };
        wheel_at_pointer(&native, delta);
        tick(cx).await;
    }
    assert!(
        date_visible,
        "date filter did not become visible within 48 native wheel events"
    );
    probe(
        "record-date-filter",
        &record_date,
        "2026-10-27",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;

    // Existing-note title policy is checked separately from editable controls.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.selected_note = app.snapshot.as_ref().and_then(|snapshot| {
                    snapshot
                        .notes
                        .iter()
                        .find(|note| note.managed && note.title == "Input boundary fixture")
                        .map(|note| note.id.clone())
                });
                assert!(
                    app.selected_note.is_some(),
                    "managed note fixture disappeared"
                );
                app.open_note_editor(false, window, cx);
                cx.notify();
            });
            window.render_frame(cx);
            let app = view.read(cx);
            assert!(app.note_editor.as_ref().unwrap().note_id.is_some());
            assert!(
                !app.note_title.read(cx).is_editable(),
                "saved-note title became editable"
            );
            evidence.push(json!({
                "stage": "saved-note-title-policy", "expectedEditable": false,
                "actualEditable": app.note_title.read(cx).is_editable(),
            }));
        })
        .unwrap();
    persist(database, &evidence);
    let draft_title = window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.note_editor = None;
                app.start_new_draft(None, window, cx);
                app.note_title
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
            window.render_frame(cx);
            view.read(cx).note_title.clone()
        })
        .unwrap();
    probe(
        "new-draft-title",
        &draft_title,
        "Editable title",
        &probe_context,
        &mut evidence,
        &mut failures,
        cx,
    )
    .await;
    if !failures.is_empty() {
        crate::smoke_visibility::capture_window(
            window,
            &editor,
            &database.with_file_name("native-inputs-failure.bmp"),
            cx,
        )
        .await;
    }
    assert!(
        failures.is_empty(),
        "native input boundary failures: {failures:?}; see native-inputs-evidence.json"
    );
    let ime_requested = std::env::var("WW_STUDYAPP_SMOKE_IME").as_deref() == Ok("1");
    if ime_requested {
        ime::exercise(window, view, &native, database, cx)
            .await
            .expect("real Pinyin IME acceptance failed; see native-ime-evidence.json");
    }
    let ime_result = if ime_requested {
        "Real Pinyin composition, candidate confirmation and single form submission verified."
    } else {
        "Chinese IME composition was not exercised."
    };
    std::fs::write(database.with_file_name("native-inputs-result.txt"),
        format!("PASS: strict Win32 SendInput reached native GPUI text and number controls through focus, re-entry, modal, navigation, recording exit, and editing probes; exact values and unchanged editor body checked. Saved-note title remains intentionally disabled. {ime_result}\n")).unwrap();
}
