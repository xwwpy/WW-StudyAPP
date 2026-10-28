//! Real Windows clipboard -> Ctrl+V -> WebView -> durable managed image proof.
//! Prior clipboard formats stay in memory and are restored, never logged/saved.
use crate::{
    app::{Page, StudyApp},
    markdown_editor::MarkdownEditor,
    smoke_editor::bridge,
};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::{Value, json};
use std::{io::Cursor, path::Path, time::Duration};
use windows::Win32::{
    Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
    Graphics::Gdi::{DeleteObject, HGDIOBJ},
    System::{DataExchange::*, Memory::*},
    UI::{
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::{CopyImage, GetForegroundWindow, IMAGE_BITMAP, LR_CREATEDIBSECTION},
    },
};

struct ClipboardOpen;
impl ClipboardOpen {
    fn open(owner: HWND) -> anyhow::Result<Self> {
        for _ in 0..20 {
            if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
                return Ok(Self);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        anyhow::bail!("cannot open Windows clipboard for isolated smoke test")
    }
}
impl Drop for ClipboardOpen {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

struct ClipboardCopy {
    format: u32,
    handle: Option<HANDLE>,
    bitmap: bool,
}
impl Drop for ClipboardCopy {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                if self.bitmap {
                    let _ = DeleteObject(HGDIOBJ(handle.0));
                } else {
                    let _ = GlobalFree(Some(HGLOBAL(handle.0)));
                }
            }
        }
    }
}
fn copy_bytes(bytes: &[u8]) -> anyhow::Result<HANDLE> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let memory = GlobalLock(handle);
        if memory.is_null() {
            let _ = GlobalFree(Some(handle));
            anyhow::bail!("clipboard allocation could not be locked");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), memory.cast(), bytes.len());
        let _ = GlobalUnlock(handle);
        Ok(HANDLE(handle.0))
    }
}
struct ClipboardBackup {
    owner: HWND,
    entries: Vec<ClipboardCopy>,
    expected_sequence: Option<u32>,
}
impl ClipboardBackup {
    fn capture(owner: HWND) -> anyhow::Result<Self> {
        let _open = ClipboardOpen::open(owner)?;
        let mut backup = Self {
            owner,
            entries: Vec::new(),
            expected_sequence: None,
        };
        let mut format = 0;
        let mut total = 0;
        loop {
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            let source = unsafe { GetClipboardData(format)? };
            // CF_BITMAP is a GDI object. Other native handle types are refused
            // before modifying the clipboard, so unsupported state is preserved.
            let bitmap = format == 2;
            let handle = if bitmap {
                unsafe { CopyImage(source, IMAGE_BITMAP, 0, 0, LR_CREATEDIBSECTION)? }
            } else {
                anyhow::ensure!(
                    ![3, 9, 14].contains(&format),
                    "clipboard contains a native object this fixture cannot safely preserve"
                );
                let length = unsafe { GlobalSize(HGLOBAL(source.0)) };
                anyhow::ensure!(length > 0, "clipboard format cannot be safely preserved");
                total += length;
                anyhow::ensure!(
                    total <= 64 * 1024 * 1024,
                    "clipboard backup exceeds isolated test budget"
                );
                let memory = unsafe { GlobalLock(HGLOBAL(source.0)) };
                anyhow::ensure!(!memory.is_null(), "clipboard format cannot be read safely");
                let result =
                    copy_bytes(unsafe { std::slice::from_raw_parts(memory.cast(), length) });
                unsafe {
                    let _ = GlobalUnlock(HGLOBAL(source.0));
                }
                result?
            };
            backup.entries.push(ClipboardCopy {
                format,
                handle: Some(handle),
                bitmap,
            });
        }
        Ok(backup)
    }
    fn record_write(&mut self) {
        self.expected_sequence = Some(unsafe { GetClipboardSequenceNumber() });
    }
    fn restore(&mut self) -> anyhow::Result<()> {
        let Some(expected) = self.expected_sequence.take() else {
            return Ok(());
        };
        let _open = ClipboardOpen::open(self.owner)?;
        // Preserve newer clipboard content if the user copied something during
        // the smoke run. Do not overwrite another application's newer value.
        if unsafe { GetClipboardSequenceNumber() } != expected {
            return Ok(());
        }
        unsafe {
            EmptyClipboard()?;
        }
        for entry in &mut self.entries {
            unsafe {
                SetClipboardData(entry.format, entry.handle)?;
            }
            entry.handle = None; // Ownership transfers to Windows.
        }
        Ok(())
    }
}
impl Drop for ClipboardBackup {
    fn drop(&mut self) {
        if self.restore().is_err() {
            eprintln!("clipboard restoration failed; no clipboard contents were logged");
        }
    }
}

fn formats() -> Value {
    unsafe {
        let png = RegisterClipboardFormatW(windows::core::w!("PNG"));
        json!({"PNG": IsClipboardFormatAvailable(png).is_ok(), "CF_DIB": IsClipboardFormatAvailable(8).is_ok(), "CF_DIBV5": IsClipboardFormatAvailable(17).is_ok()})
    }
}
fn ctrl_v(state: &Value) {
    let owner = HWND(state["parentHwnd"].as_u64().unwrap() as usize as *mut _);
    assert_eq!(
        unsafe { GetForegroundWindow() },
        owner,
        "paste target must be the isolated smoke window"
    );
    let inputs = [
        (VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        // Each fixture appends through real keyboard navigation. Repeated
        // focus clicks may otherwise form a double-click text selection.
        (VK_END, KEYBD_EVENT_FLAGS(0)),
        (VK_END, KEYEVENTF_KEYUP),
        (VK_V, KEYBD_EVENT_FLAGS(0)),
        (VK_V, KEYEVENTF_KEYUP),
        (VK_CONTROL, KEYEVENTF_KEYUP),
    ]
    .map(|(key, flags)| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                ..Default::default()
            },
        },
    });
    assert_eq!(
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) },
        inputs.len() as u32
    );
}
fn dib(version5: bool) -> Vec<u8> {
    let size = if version5 { 124 } else { 40 };
    let mut bytes = vec![0_u8; size + 40 * 40 * 4];
    bytes[0..4].copy_from_slice(&(size as u32).to_le_bytes());
    bytes[4..8].copy_from_slice(&40_i32.to_le_bytes());
    bytes[8..12].copy_from_slice(&(-40_i32).to_le_bytes());
    bytes[12..14].copy_from_slice(&1_u16.to_le_bytes());
    bytes[14..16].copy_from_slice(&32_u16.to_le_bytes());
    if version5 {
        bytes[16..20].copy_from_slice(&3_u32.to_le_bytes());
        for (offset, value) in [
            (40, 0x00ff0000_u32),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
            (56, 0x73524742),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    for pixel in bytes[size..].as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(if version5 {
            &[180, 70, 35, 255]
        } else {
            &[35, 90, 190, 255]
        });
    }
    bytes
}
async fn paste(window: AnyWindowHandle, editor: &Entity<MarkdownEditor>, cx: &mut AsyncApp) {
    crate::smoke_visibility::focus_body(window, editor, cx).await;
    let state = crate::smoke_visibility::visible(window, editor, cx).await;
    ctrl_v(&state);
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    let editor = window
        .update(cx, |_, window, cx| {
            let settings = &view.read(cx).snapshot.as_ref().unwrap().settings.images;
            assert_eq!(
                settings.storage,
                studyapp_core::ImageStorage::Local,
                "clipboard fixture requires isolated local image storage"
            );
            assert!(
                settings.root.is_none(),
                "clipboard fixture must write inside its smoke library"
            );
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.clear_notices(cx);
                cx.notify();
            });
            window.render_frame(cx);
            crate::smoke_library::library_add(window, false, view, cx);
            view.update(cx, |app, cx| {
                app.note_title.update(cx, |title, cx| {
                    title.set_value("Windows 剪贴板图片测试", window, cx)
                })
            });
            view.read(cx).note_content.clone()
        })
        .unwrap();
    crate::smoke_library::until(
        window,
        view,
        "editable clipboard fixture",
        |app, cx| {
            app.note_editor.is_some()
                && app.note_content.read(cx).editable()
                && app.note_content.read(cx).ready()
        },
        cx,
    )
    .await;
    let state = crate::smoke_visibility::visible(window, &editor, cx).await;
    let owner = HWND(state["parentHwnd"].as_u64().unwrap() as usize as *mut _);
    let mut backup = ClipboardBackup::capture(owner)
        .expect("preserve every existing clipboard format before testing");
    window
        .update(cx, |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string("剪贴板纯文本 fixture\n\n".into()))
        })
        .unwrap();
    backup.record_write();
    paste(window, &editor, cx).await;
    crate::smoke_library::until(
        window,
        view,
        "real Ctrl+V text",
        |app, cx| {
            app.note_content
                .read(cx)
                .value()
                .contains("剪贴板纯文本 fixture")
        },
        cx,
    )
    .await;

    let image = image::RgbaImage::from_pixel(40, 40, image::Rgba([30, 120, 200, 255]));
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let png = Image::from_bytes(ImageFormat::Png, png);
    let mut evidence = Vec::new();
    for (index, format) in ["PNG", "CF_DIB", "CF_DIBV5"].into_iter().enumerate() {
        if index == 0 {
            window
                .update(cx, |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_image(&png))
                })
                .unwrap();
        } else {
            let _open = ClipboardOpen::open(owner).unwrap();
            unsafe {
                EmptyClipboard().unwrap();
                let handle = copy_bytes(&dib(index == 2)).unwrap();
                SetClipboardData(if index == 2 { 17 } else { 8 }, Some(handle)).unwrap();
            }
        }
        backup.record_write();
        let available = formats();
        paste(window, &editor, cx).await;
        cx.background_executor()
            .timer(Duration::from_millis(500))
            .await;
        let browser = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
        // Keep a diagnostic checkpoint even if the following assertion fails.
        // Select metadata explicitly; never serialize document/clipboard text.
        let diagnostic = json!({"source":format,"nativeFormats":available,"browserPastes":browser["observedPastes"],"generation":browser["generation"],"readonly":browser["readonly"],"pendingFlush":browser["pendingFlush"],"editorFocused":browser["editorFocused"],"imageReferences":browser["text"].as_str().unwrap_or("").matches("studyapp-media:").count()});
        std::fs::write(
            database.with_file_name(format!("clipboard-{format}-diagnostic.json")),
            serde_json::to_vec_pretty(&diagnostic).unwrap(),
        )
        .unwrap();
        crate::smoke_library::until(
            window,
            view,
            &format!("real Ctrl+V {format} image"),
            |app, cx| {
                app.note_content
                    .read(cx)
                    .value()
                    .matches("studyapp-media:")
                    .count()
                    == index + 1
                    && !app.media_busy()
            },
            cx,
        )
        .await;
        let text = window
            .update(cx, |_, _, cx| editor.read(cx).value().to_owned())
            .unwrap();
        // The native bridge may only insert a reference after the worker's
        // durable asset write. Verify every reference immediately on receipt.
        for tail in text.split("studyapp-media:").skip(1) {
            let key = tail.split(')').next().unwrap();
            let file = database.parent().unwrap().join("images").join(key);
            assert!(file.is_file(), "markdown referenced a missing image asset");
            image::open(file).expect("durable clipboard asset is decodable");
        }
        let observed = browser["observedPastes"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(
            observed["trusted"], true,
            "image must originate in a real native paste"
        );
        let fallback = observed["files"].as_array().unwrap().is_empty();
        assert!(!fallback || (format == "PNG" && available["PNG"] == true));
        evidence.push(json!({"source":format, "nativeFormats":available, "imageReferences":index+1, "durableAssets":true, "trustedPaste":true, "nativePngFallback":fallback}));
    }
    backup
        .restore()
        .expect("restore original clipboard without logging its content");
    bridge(window, &editor, json!({"type":"select", "anchor":0}), cx).await;
    for attempt in 0..100 {
        let snapshot = bridge(window, &editor, json!({"type":"snapshot"}), cx).await;
        if snapshot["images"].as_u64().unwrap_or(0) >= 3 {
            break;
        }
        assert!(attempt < 99, "clipboard images did not render: {snapshot}");
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
    crate::smoke_visibility::capture(
        window,
        &editor,
        &database.with_file_name("native-clipboard.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| window.click("save-note", cx))
        .unwrap();
    crate::smoke_library::until(
        window,
        view,
        "clipboard note saved",
        |app, _| {
            !app.note_saving
                && app.note_editor.is_none()
                && app
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.notes.iter().any(|n| n.title == "Windows 剪贴板图片测试"))
        },
        cx,
    )
    .await;
    let saved = window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .find(|n| n.title == "Windows 剪贴板图片测试")
                .unwrap()
                .path
                .clone()
        })
        .unwrap();
    let content = std::fs::read_to_string(&saved).unwrap();
    assert_eq!(content.matches("studyapp-media:").count(), 3);
    assert!(content.contains("剪贴板纯文本 fixture"));
    let fallback_used = evidence
        .iter()
        .any(|item| item["nativePngFallback"] == true);
    std::fs::write(database.with_file_name("clipboard-evidence.json"), serde_json::to_vec_pretty(&json!({"input":"WIN32_SENDINPUT_CTRL_V", "plainText":true, "images":evidence, "savedNote":saved, "nativeRendered":true, "fallbackUsed":fallback_used, "restartVerified":false})).unwrap()).unwrap();
}
