//! Native policy for this application's own transparent floating cards.
use anyhow::Result;
use gpui_kit::{App, Bounds, Pixels, Window, point, px, size};
use studyapp_core::NoticePosition;

pub fn notification_bounds(
    screen: Bounds<Pixels>,
    width: Pixels,
    natural: Pixels,
    position: NoticePosition,
) -> Bounds<Pixels> {
    let margin = px(16.)
        .min(screen.size.width / 4.)
        .min(screen.size.height / 4.)
        .max(px(0.));
    let width = width
        .ceil()
        .max(px(1.))
        .min((screen.size.width - margin * 2.).max(px(1.)));
    let height = natural
        .ceil()
        .max(px(1.))
        .min((screen.size.height - margin * 2.).max(px(1.)));
    let x = if matches!(
        position,
        NoticePosition::TopLeft | NoticePosition::BottomLeft
    ) {
        screen.left() + margin
    } else {
        screen.right() - margin - width
    };
    let y = if matches!(position, NoticePosition::TopLeft | NoticePosition::TopRight) {
        screen.top() + margin
    } else {
        screen.bottom() - margin - height
    };
    Bounds::new(point(x, y), size(width, height))
}

/// DWM must not paint an opaque non-client frame over our rounded card.
/// This changes neither hit testing nor activation and never finds other windows.
pub fn disable_native_frame(window: &Window) -> Result<()> {
    #[cfg(all(windows, not(test)))]
    {
        use windows::Win32::{
            Foundation::HWND,
            Graphics::Dwm::{DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute},
        };
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(raw) = HasWindowHandle::window_handle(window)?.as_raw() else {
            anyhow::bail!("浮层窗口句柄无效")
        };
        let policy = DWMNCRP_DISABLED;
        // SAFETY: this foreground Window retains its own HWND through the call.
        unsafe {
            DwmSetWindowAttribute(
                HWND(raw.hwnd.get() as *mut _),
                DWMWA_NCRENDERING_POLICY,
                std::ptr::from_ref(&policy).cast(),
                std::mem::size_of_val(&policy) as u32,
            )?;
        }
    }
    #[cfg(any(not(windows), test))]
    let _ = window;
    Ok(())
}

/// Fit and anchor without activation. GPUI's Windows resize currently omits
/// SWP_NOACTIVATE, so a passive popup cannot use that generic resize path.
pub fn fit_passive(window: &mut Window, bounds: Bounds<Pixels>, cx: &mut App) -> Result<()> {
    #[cfg(all(windows, not(test)))]
    {
        use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(raw) = HasWindowHandle::window_handle(window)?.as_raw() else {
            anyhow::bail!("通知窗口句柄无效")
        };
        let scale = window.scale_factor();
        // SAFETY: only the borrowed, application-owned popup is positioned.
        unsafe {
            SetWindowPos(
                HWND(raw.hwnd.get() as *mut _),
                None,
                (f32::from(bounds.origin.x) * scale).round() as i32,
                (f32::from(bounds.origin.y) * scale).round() as i32,
                (f32::from(bounds.size.width) * scale).round() as i32,
                (f32::from(bounds.size.height) * scale).round() as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )?;
        }
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use anyhow::Context;
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::AppKit(raw) = HasWindowHandle::window_handle(window)?.as_raw() else {
            anyhow::bail!("通知窗口句柄无效")
        };
        anyhow::ensure!(
            objc2::MainThreadMarker::new().is_some(),
            "通知布局需要应用主线程"
        );
        let current = window.window_bounds().get_bounds();
        // SAFETY: the foreground Window keeps its borrowed NSView alive.
        let view = unsafe { raw.ns_view.cast::<objc2_app_kit::NSView>().as_ref() };
        let native = view.window().context("通知尚未关联原生窗口")?;
        let mut frame = native.frame();
        frame.origin.x += f64::from(f32::from(bounds.left() - current.left()));
        frame.origin.y -= f64::from(f32::from(
            bounds.top() - current.top() + bounds.size.height - current.size.height,
        ));
        frame.size.width = f64::from(f32::from(bounds.size.width));
        frame.size.height = f64::from(f32::from(bounds.size.height));
        native.setFrame_display(frame, true);
    }
    #[cfg(all(target_os = "linux", not(test)))]
    {
        use anyhow::Context;
        use wry::raw_window_handle::{
            HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
        };
        use x11rb::{
            connection::Connection,
            protocol::xproto::{ConfigureWindowAux, ConnectionExt},
            xcb_ffi::XCBConnection,
        };
        let native = HasWindowHandle::window_handle(window)?;
        let display = HasDisplayHandle::display_handle(window)?;
        let (RawWindowHandle::Xcb(raw), RawDisplayHandle::Xcb(display)) =
            (native.as_raw(), display.as_raw())
        else {
            anyhow::bail!("通知布局需要应用自己的 X11 连接")
        };
        // SAFETY: borrow the live connection without taking ownership or events.
        let connection = unsafe {
            XCBConnection::from_raw_xcb_connection(
                display.connection.context("应用 X11 连接不可用")?.as_ptr(),
                false,
            )?
        };
        let scale = window.scale_factor();
        connection
            .configure_window(
                raw.window.get(),
                &ConfigureWindowAux::new()
                    .x((f32::from(bounds.left()) * scale).round() as i32)
                    .y((f32::from(bounds.top()) * scale).round() as i32)
                    .width((f32::from(bounds.size.width) * scale).round().max(1.) as u32)
                    .height((f32::from(bounds.size.height) * scale).round().max(1.) as u32),
            )?
            .check()?;
        connection.flush()?;
    }
    #[cfg(test)]
    window.resize(bounds.size);
    // Native resize callbacks may run synchronously while this Window is leased.
    // Reconcile the viewport from the platform after the native call returns;
    // otherwise the measured fit can succeed while layout still uses the old size.
    window.bounds_changed(cx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_fit_real_content_and_remain_anchored_on_small_negative_origin_displays() {
        for screen in [
            Bounds::new(point(px(-1600.), px(-200.)), size(px(1600.), px(900.))),
            Bounds::new(point(px(100.), px(200.)), size(px(220.), px(160.))),
        ] {
            for position in [
                NoticePosition::TopLeft,
                NoticePosition::TopRight,
                NoticePosition::BottomLeft,
                NoticePosition::BottomRight,
            ] {
                let short = notification_bounds(screen, px(376.), px(119.1), position);
                assert!(short.left() >= screen.left() && short.top() >= screen.top());
                assert!(short.right() <= screen.right() && short.bottom() <= screen.bottom());
                assert_eq!(short.size.height, px(120.));
                let long = notification_bounds(screen, px(496.), px(1800.), position);
                assert!(long.right() <= screen.right() && long.bottom() <= screen.bottom());
                if matches!(
                    position,
                    NoticePosition::BottomLeft | NoticePosition::BottomRight
                ) {
                    assert_eq!(short.bottom(), long.bottom());
                } else {
                    assert_eq!(short.top(), long.top());
                }
                assert_eq!(
                    notification_bounds(screen, px(376.), px(119.1), position),
                    short
                );
            }
        }
    }
}
