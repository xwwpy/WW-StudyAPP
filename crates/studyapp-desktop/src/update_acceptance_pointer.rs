//! Native event injection only in the explicitly enabled remote acceptance fixture.
use gpui_kit::{Pixels, Point, Window};
use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub fn capture(window: &mut Window, path: &std::path::Path) {
    capture_inner(window, path, true);
}
pub fn capture_diagnostic(window: &mut Window, path: &std::path::Path) {
    capture_inner(window, path, false);
}
fn capture_inner(window: &mut Window, path: &std::path::Path, require_foreground: bool) {
    #[cfg(target_os = "macos")]
    {
        let _ = (window, require_foreground);
        assert!(
            std::process::Command::new("/usr/sbin/screencapture")
                .arg("-x")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::{HWND, RECT},
            Graphics::Gdi::*,
            UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect},
        };
        let RawWindowHandle::Win32(raw) = window.window_handle().unwrap().as_raw() else {
            panic!("Win32 parent required")
        };
        let parent = HWND(raw.hwnd.get() as *mut _);
        // SAFETY: snapshot of the live foreground window; all temporary GDI objects
        // are released before returning and no native child is excluded.
        unsafe {
            if require_foreground {
                assert_eq!(GetForegroundWindow(), parent);
            }
            let mut bounds = RECT::default();
            GetWindowRect(parent, &mut bounds).unwrap();
            let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
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
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let mut bits = std::ptr::null_mut();
            let bitmap =
                CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            let result = BitBlt(
                memory,
                0,
                0,
                width,
                height,
                Some(screen),
                bounds.left,
                bounds.top,
                SRCCOPY,
            );
            let mut pixels =
                std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize)
                    .to_vec();
            let _ = SelectObject(memory, previous);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            result.unwrap();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            image::RgbaImage::from_raw(width as u32, height as u32, pixels)
                .unwrap()
                .save(path)
                .unwrap();
        }
    }
}

#[cfg(windows)]
pub fn click(window: &mut Window, point: Point<Pixels>) {
    use windows::Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::ClientToScreen,
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
                SendInput,
            },
            WindowsAndMessaging::{
                GA_ROOT, GetAncestor, GetForegroundWindow, SetCursorPos, SetForegroundWindow,
                WindowFromPoint,
            },
        },
    };
    let RawWindowHandle::Win32(raw) = window.window_handle().unwrap().as_raw() else {
        panic!("Win32 parent required")
    };
    let parent = HWND(raw.hwnd.get() as *mut _);
    let scale = f64::from(window.scale_factor());
    let mut point = POINT {
        x: (f64::from(point.x) * scale).round() as i32,
        y: (f64::from(point.y) * scale).round() as i32,
    };
    // SAFETY: this runs on the owning UI thread; parent remains alive for this call.
    unsafe {
        let _ = SetForegroundWindow(parent);
        assert_eq!(
            GetForegroundWindow(),
            parent,
            "Acceptance window must be foreground"
        );
        assert!(ClientToScreen(parent, &mut point).as_bool());
        SetCursorPos(point.x, point.y).unwrap();
        assert_eq!(
            GetAncestor(WindowFromPoint(point), GA_ROOT),
            parent,
            "Another native window covers the control"
        );
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
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32),
            2,
            "Native pointer injection failed"
        );
    }
}

#[cfg(target_os = "macos")]
pub fn click(window: &mut Window, point: Point<Pixels>) {
    assert_eq!(
        mac_event(window, point, true)["ready"],
        true,
        "Native control is not frontmost"
    );
}

#[cfg(target_os = "macos")]
pub fn foreground(window: &mut Window, point: Point<Pixels>) -> serde_json::Value {
    mac_event(window, point, false)
}

#[cfg(target_os = "macos")]
fn mac_event(window: &mut Window, point: Point<Pixels>, post_click: bool) -> serde_json::Value {
    use std::ffi::{c_char, c_void};
    type Obj = *mut c_void;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Position {
        x: f64,
        y: f64,
    }
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Obj;
        fn sel_registerName(name: *const c_char) -> Obj;
        fn objc_msgSend();
    }
    let RawWindowHandle::AppKit(raw) = window.window_handle().unwrap().as_raw() else {
        panic!("AppKit parent required")
    };
    // SAFETY: selectors and function signatures match the AppKit ABI. The raw view
    // is owned by the live GPUI window on this main thread. Events enter NSApplication's
    // queue and NSWindow's normal NSView hit testing, never GPUI's test dispatcher.
    unsafe {
        let send0: unsafe extern "C" fn(Obj, Obj) -> Obj =
            std::mem::transmute(objc_msgSend as *const ());
        let send_bool: unsafe extern "C" fn(Obj, Obj) -> bool =
            std::mem::transmute(objc_msgSend as *const ());
        let send_arg: unsafe extern "C" fn(Obj, Obj, Obj) =
            std::mem::transmute(objc_msgSend as *const ());
        let send_flag: unsafe extern "C" fn(Obj, Obj, bool) =
            std::mem::transmute(objc_msgSend as *const ());
        let convert: unsafe extern "C" fn(Obj, Obj, Position, Obj) -> Position =
            std::mem::transmute(objc_msgSend as *const ());
        let number: unsafe extern "C" fn(Obj, Obj) -> isize =
            std::mem::transmute(objc_msgSend as *const ());
        let event: unsafe extern "C" fn(
            Obj,
            Obj,
            usize,
            Position,
            usize,
            f64,
            isize,
            Obj,
            isize,
            isize,
            f32,
        ) -> Obj = std::mem::transmute(objc_msgSend as *const ());
        let post: unsafe extern "C" fn(Obj, Obj, Obj, bool) =
            std::mem::transmute(objc_msgSend as *const ());
        let view = raw.ns_view.as_ptr();
        let native = send0(view, sel_registerName(c"window".as_ptr()));
        assert!(!native.is_null());
        let app = send0(
            objc_getClass(c"NSApplication".as_ptr()),
            sel_registerName(c"sharedApplication".as_ptr()),
        );
        send_flag(
            app,
            sel_registerName(c"activateIgnoringOtherApps:".as_ptr()),
            true,
        );
        send_arg(
            native,
            sel_registerName(c"makeKeyAndOrderFront:".as_ptr()),
            std::ptr::null_mut(),
        );
        let flipped = send_bool(view, sel_registerName(c"isFlipped".as_ptr()));
        let local = Position {
            x: f64::from(point.x),
            y: if flipped {
                f64::from(point.y)
            } else {
                f64::from(window.viewport_size().height) - f64::from(point.y)
            },
        };
        let position = convert(
            view,
            sel_registerName(c"convertPoint:toView:".as_ptr()),
            local,
            std::ptr::null_mut(),
        );
        let window_number = number(native, sel_registerName(c"windowNumber".as_ptr()));
        let screen: unsafe extern "C" fn(Obj, Obj, Position) -> Position =
            std::mem::transmute(objc_msgSend as *const ());
        let hit: unsafe extern "C" fn(Obj, Obj, Position, isize) -> isize =
            std::mem::transmute(objc_msgSend as *const ());
        let screen_point = screen(
            native,
            sel_registerName(c"convertPointToScreen:".as_ptr()),
            position,
        );
        let front_window = hit(
            objc_getClass(c"NSWindow".as_ptr()),
            sel_registerName(c"windowNumberAtPoint:belowWindowWithWindowNumber:".as_ptr()),
            screen_point,
            0,
        );
        let active = send_bool(app, sel_registerName(c"isActive".as_ptr()));
        let key = send_bool(native, sel_registerName(c"isKeyWindow".as_ptr()));
        let visible = send_bool(native, sel_registerName(c"isVisible".as_ptr()));
        let observation = serde_json::json!({"ready": active && key && visible && front_window == window_number,
            "active":active,"key":key,"visible":visible,"window":window_number,"frontWindow":front_window,
            "screenX":screen_point.x,"screenY":screen_point.y});
        if !post_click || observation["ready"] != true {
            return observation;
        }
        for kind in [1, 2] {
            let value = event(objc_getClass(c"NSEvent".as_ptr()), sel_registerName(c"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:".as_ptr()),
                kind, position, 0, 0.0, window_number, std::ptr::null_mut(), 1, 1, if kind == 1 { 1.0 } else { 0.0 });
            assert!(!value.is_null());
            post(
                app,
                sel_registerName(c"postEvent:atStart:".as_ptr()),
                value,
                false,
            );
        }
        observation
    }
}
