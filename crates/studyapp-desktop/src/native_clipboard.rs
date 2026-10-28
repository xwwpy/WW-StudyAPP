//! Narrow fallback for the PNG-only format omitted from WebView2 paste events.
//! Never enumerate/read text, files, or arbitrary registered clipboard formats.
use anyhow::{Context as _, Result, ensure};
use gpui_kit::Window;
use windows::Win32::{
    Foundation::{HGLOBAL, HWND},
    System::{
        DataExchange::{
            CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
            RegisterClipboardFormatW,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
    },
    UI::WindowsAndMessaging::GetForegroundWindow,
};
use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};

struct ClipboardOpen;
impl Drop for ClipboardOpen {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn allowed_size(length: usize) -> Result<()> {
    ensure!(
        (8..=crate::media::MAX_IMAGE_BYTES).contains(&length),
        "剪贴板图片须小于 10 MiB"
    );
    Ok(())
}

pub(super) fn png(window: &Window) -> Result<Option<Vec<u8>>> {
    let handle = HasWindowHandle::window_handle(window).context("无法取得编辑器窗口")?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return Ok(None);
    };
    let owner = HWND(raw.hwnd.get() as *mut _);
    // A queued IPC request must not read clipboard state after focus moves to
    // another application. This path never activates or changes any window.
    if unsafe { GetForegroundWindow() } != owner {
        return Ok(None);
    }
    let format = unsafe { RegisterClipboardFormatW(windows::core::w!("PNG")) };
    ensure!(format != 0, "无法读取剪贴板图片格式");
    if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
        return Ok(None);
    }
    unsafe { OpenClipboard(Some(owner)) }.context("剪贴板正忙，请稍后重试")?;
    let _open = ClipboardOpen;
    let handle = HGLOBAL(
        unsafe { GetClipboardData(format) }
            .context("无法读取剪贴板图片")?
            .0,
    );
    let length = unsafe { GlobalSize(handle) };
    allowed_size(length)?; // Bound allocation before locking/copying native data.
    let memory = unsafe { GlobalLock(handle) };
    ensure!(!memory.is_null(), "无法读取剪贴板图片");
    let bytes = unsafe { std::slice::from_raw_parts(memory.cast::<u8>(), length) };
    let png = bytes
        .starts_with(b"\x89PNG\r\n\x1a\n")
        .then(|| bytes.to_vec());
    unsafe {
        let _ = GlobalUnlock(handle);
    }
    ensure!(png.is_some(), "剪贴板 PNG 图片损坏");
    // Decode and size/dimension validation happen on the existing media worker.
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::allowed_size;
    #[test]
    fn native_png_clipboard_is_bounded_before_allocation() {
        assert!(allowed_size(0).is_err());
        assert!(allowed_size(7).is_err());
        assert!(allowed_size(8).is_ok());
        assert!(allowed_size(crate::media::MAX_IMAGE_BYTES).is_ok());
        assert!(allowed_size(crate::media::MAX_IMAGE_BYTES + 1).is_err());
        assert!(allowed_size(usize::MAX).is_err());
    }
}
