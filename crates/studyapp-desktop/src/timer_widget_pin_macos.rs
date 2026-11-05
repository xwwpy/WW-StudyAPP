//! Change only this widget's NSWindow level, without activation or reparenting.
use anyhow::{Context, Result, ensure};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSFloatingWindowLevel, NSNormalWindowLevel, NSView};
use std::{ffi::c_void, ptr::NonNull};

/// # Safety
/// `view` is the live AppKit NSView from this widget's borrowed GPUI handle.
/// The caller retains that handle on the foreground thread until return.
pub(super) unsafe fn apply(view: NonNull<c_void>, pinned: bool, request: bool) -> Result<bool> {
    ensure!(
        MainThreadMarker::new().is_some(),
        "置顶操作需要在应用主线程执行"
    );
    // SAFETY: the caller pins a GPUI AppKit handle for this entire operation.
    let view = unsafe { view.cast::<NSView>().as_ref() };
    let window = view.window().context("计时小组件尚未关联原生窗口")?;
    let level = if pinned {
        NSFloatingWindowLevel
    } else {
        NSNormalWindowLevel
    };
    if request {
        window.setLevel(level);
        ensure!(window.level() == level, "计时小组件原生置顶状态未生效");
    }
    Ok(window.level() == level)
}

/// Read NSWindow level/frame and the application's actual key window, without
/// activation, reparenting or another setLevel call.
/// # Safety
/// The same live-NSView/foreground requirements as `apply` hold.
#[cfg(feature = "ui-test")]
pub(super) unsafe fn observe(view: NonNull<c_void>) -> Result<serde_json::Value> {
    let mtm = MainThreadMarker::new().context("置顶检查需要在应用主线程执行")?;
    let view = unsafe { view.cast::<NSView>().as_ref() };
    let window = view.window().context("计时小组件尚未关联原生窗口")?;
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let frame = window.frame();
    Ok(serde_json::json!({
        "pinned": window.level() == NSFloatingWindowLevel,
        "level": window.level(),
        "focus": [serde_json::json!(app.isActive()), serde_json::json!(app.keyWindow().map(|key| key.windowNumber()))],
        "bounds": [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height],
        "managed": true,
    }))
}
