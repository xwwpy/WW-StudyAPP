//! Device-local window state; never part of synced preferences or timer ticks.
use anyhow::{Context, Result, ensure};
use gpui_kit::{Bounds, Pixels, point, px, size};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetWindowState {
    pub version: u32,
    pub open: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: u8,
}
impl Default for WidgetWindowState {
    fn default() -> Self {
        Self {
            version: 1,
            open: false,
            x: 80,
            y: 80,
            width: 380,
            height: 390,
            scale: 100,
        }
    }
}
impl WidgetWindowState {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.try_exists()? {
            return Ok(Self::default());
        }
        let file = crate::storage_backend::plain_handle(path, false, true)?;
        ensure!(file.metadata()?.len() <= 8192, "计时小组件窗口配置过大");
        let mut bytes = Vec::new();
        file.take(8193).read_to_end(&mut bytes)?;
        let state: Self = serde_json::from_slice(&bytes).context("计时小组件窗口配置无效")?;
        ensure!(state.version == 1, "计时小组件窗口配置版本不受支持");
        Ok(state)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::app_migration::atomic_json(path, self)
    }
    pub fn capture(&mut self, bounds: Bounds<Pixels>) {
        self.x = f32::from(bounds.origin.x).round() as i32;
        self.y = f32::from(bounds.origin.y).round() as i32;
        self.width = f32::from(bounds.size.width).round().max(1.) as u32;
        self.height = f32::from(bounds.size.height).round().max(1.) as u32;
    }
    pub fn clamp(&mut self, displays: &[Bounds<Pixels>]) {
        let Some(display) = displays.iter().max_by_key(|display| {
            let left = i64::from(self.x).max(f32::from(display.origin.x) as i64);
            let top = i64::from(self.y).max(f32::from(display.origin.y) as i64);
            let right =
                (i64::from(self.x) + i64::from(self.width)).min(f32::from(display.right()) as i64);
            let bottom = (i64::from(self.y) + i64::from(self.height))
                .min(f32::from(display.bottom()) as i64);
            (right - left).max(0) * (bottom - top).max(0)
        }) else {
            return;
        };
        let width = (f32::from(display.size.width).max(1.)) as u32;
        let height = (f32::from(display.size.height).max(1.)) as u32;
        self.width = self.width.clamp(140, 1800).min(width);
        self.height = self.height.clamp(100, 2000).min(height);
        let x = f32::from(display.origin.x).round() as i32;
        let y = f32::from(display.origin.y).round() as i32;
        self.x = self
            .x
            .clamp(x, x.saturating_add((width - self.width) as i32));
        self.y = self
            .y
            .clamp(y, y.saturating_add((height - self.height) as i32));
    }
    /// Apply a new preference relative to the last saved zoom, not timer ticks.
    pub fn rescale(&mut self, scale: u8) -> bool {
        let old = if (50..=200).contains(&self.scale) {
            self.scale
        } else {
            100
        };
        let scale = scale.clamp(50, 200);
        if scale == old {
            self.scale = scale;
            return false;
        }
        let factor = f32::from(scale) / f32::from(old);
        self.width = (self.width as f32 * factor).round().clamp(140., 1800.) as u32;
        self.height = (self.height as f32 * factor).round().clamp(100., 2000.) as u32;
        self.scale = scale;
        true
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.x as f32), px(self.y as f32)),
            size(px(self.width as f32), px(self.height as f32)),
        )
    }
}

/// Fit on opening or changed content/width, not every timer tick or manual
/// vertical resize. Round once to avoid subpixel oscillation.
#[derive(Default)]
pub struct WidgetContentFit {
    last: Option<(u32, u32)>,
}
impl WidgetContentFit {
    pub fn height(&mut self, width: Pixels, natural: Pixels, available: Pixels) -> Option<Pixels> {
        let (width, natural, available) =
            (f32::from(width), f32::from(natural), f32::from(available));
        if !width.is_finite()
            || !natural.is_finite()
            || !available.is_finite()
            || width <= 0.
            || natural <= 0.
            || available <= 0.
        {
            return None;
        }
        let maximum = available.floor().clamp(1., 1000.) as u32;
        let desired = (natural.ceil().min(1000.) as u32).clamp(100.min(maximum), maximum);
        let key = (width.round() as u32, desired);
        if self.last == Some(key) {
            return None;
        }
        self.last = Some(key);
        Some(px(desired as f32))
    }
}

#[cfg(all(target_os = "linux", not(test)))]
#[path = "timer_widget_pin_linux.rs"]
mod linux_pin;
#[cfg(all(target_os = "macos", not(test)))]
#[path = "timer_widget_pin_macos.rs"]
mod macos_pin;

pub fn decorate(window: &mut gpui_kit::Window, pinned: bool, request: bool) -> Result<bool> {
    #[cfg(all(windows, not(test)))]
    {
        use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(raw) = HasWindowHandle::window_handle(window)?.as_raw() else {
            anyhow::bail!("计时窗口句柄无效")
        };
        let hwnd = HWND(raw.hwnd.get() as *mut _);
        // Only this application's new floating window is modified.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if request {
                crate::card_window::disable_native_frame(window)?;
                SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    (style | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize),
                );
                SetWindowPos(
                    hwnd,
                    Some(if pinned { HWND_TOPMOST } else { HWND_NOTOPMOST }),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                )?;
            }
            let applied = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            ensure!(
                applied & WS_EX_TOOLWINDOW.0 as isize != 0
                    && applied & WS_EX_APPWINDOW.0 as isize == 0,
                "计时窗口装饰未生效"
            );
            Ok((applied & WS_EX_TOPMOST.0 as isize != 0) == pinned)
        }
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::AppKit(raw) = HasWindowHandle::window_handle(window)?.as_raw() else {
            anyhow::bail!("计时小组件需要原生 AppKit 窗口")
        };
        // SAFETY: this foreground GPUI window owns and retains its NSView.
        unsafe { macos_pin::apply(raw.ns_view, pinned, request) }
    }
    #[cfg(all(target_os = "linux", not(test)))]
    {
        use wry::raw_window_handle::{
            HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
        };
        let handle = HasWindowHandle::window_handle(window)?;
        let display = HasDisplayHandle::display_handle(window)?;
        let (RawWindowHandle::Xcb(raw), RawDisplayHandle::Xcb(display)) =
            (handle.as_raw(), display.as_raw())
        else {
            anyhow::bail!("计时小组件置顶需要应用自己的 X11/XWayland 窗口连接")
        };
        let connection = display.connection.context("应用的 X11 连接不可用")?;
        // SAFETY: both live handle borrows remain in scope; no ownership or
        // event queue is transferred to the temporary XCB wrapper.
        unsafe { linux_pin::apply(connection, raw.window.get(), pinned, request) }
    }
    #[cfg(test)]
    {
        let _ = (window, pinned, request);
        Ok(true)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos", test)))]
    {
        let _ = (window, pinned, request);
        anyhow::bail!("当前平台不支持计时小组件置顶")
    }
}

#[cfg(all(feature = "ui-test", not(test)))]
pub(crate) fn pin_observation(window: &gpui_kit::Window) -> Result<serde_json::Value> {
    use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
        "原生置顶验收仅允许在 GitHub runner 执行"
    );
    let handle = HasWindowHandle::window_handle(window)?;
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::{HWND, RECT},
            Graphics::Dwm::{DWMWA_NCRENDERING_ENABLED, DwmGetWindowAttribute},
            UI::WindowsAndMessaging::*,
        };
        let RawWindowHandle::Win32(raw) = handle.as_raw() else {
            anyhow::bail!("计时窗口句柄无效")
        };
        let hwnd = HWND(raw.hwnd.get() as *mut _);
        let mut bounds = RECT::default();
        unsafe {
            GetWindowRect(hwnd, &mut bounds)?;
            let mut non_client_rendering = 0u32;
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_NCRENDERING_ENABLED,
                (&mut non_client_rendering as *mut u32).cast(),
                std::mem::size_of_val(&non_client_rendering) as u32,
            )?;
            Ok(serde_json::json!({
                "nonClientRendering": non_client_rendering != 0,
                "pinned": GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize != 0,
                "focus": GetForegroundWindow().0 as usize,
                "bounds": [bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top],
                "managed": true,
            }))
        }
    }
    #[cfg(target_os = "macos")]
    {
        let RawWindowHandle::AppKit(raw) = handle.as_raw() else {
            anyhow::bail!("计时小组件需要原生 AppKit 窗口")
        };
        // SAFETY: the foreground window keeps the borrowed NSView alive.
        unsafe { macos_pin::observe(raw.ns_view) }
    }
    #[cfg(target_os = "linux")]
    {
        use wry::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        let display = HasDisplayHandle::display_handle(window)?;
        let (RawWindowHandle::Xcb(raw), RawDisplayHandle::Xcb(display)) =
            (handle.as_raw(), display.as_raw())
        else {
            anyhow::bail!("计时小组件需要应用自己的 X11 连接")
        };
        // SAFETY: both native handle borrows outlive this passive observation.
        unsafe {
            linux_pin::observe(
                display.connection.context("应用的 X11 连接不可用")?,
                raw.window.get(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_state_roundtrip_defaults_and_bad_files_are_non_destructive() {
        let root = tempfile::tempdir().unwrap().keep();
        let path = root.join("timer-widget-window.json");
        assert!(!WidgetWindowState::load(&path).unwrap().open);
        let state = WidgetWindowState {
            open: true,
            x: -1200,
            width: 420,
            ..Default::default()
        };
        state.save(&path).unwrap();
        let loaded = WidgetWindowState::load(&path).unwrap();
        assert!(loaded.open);
        assert_eq!(loaded.x, -1200);
        assert_eq!(loaded.width, 420);
        for contents in [
            "{broken".to_owned(),
            r#"{"version":2}"#.to_owned(),
            " ".repeat(8193),
        ] {
            std::fs::write(&path, &contents).unwrap();
            assert!(WidgetWindowState::load(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
    }

    #[test]
    fn legacy_window_state_and_zoom_roundtrip_preserve_manual_size_between_ticks() {
        let mut state: WidgetWindowState =
            serde_json::from_str(r#"{"version":1,"width":400,"height":300,"x":85,"y":90}"#)
                .unwrap();
        assert_eq!(state.scale, 100);
        for scale in [50, 73, 175, 200, 100] {
            assert!(state.rescale(scale));
            assert!((state.width as f32 - 400. * f32::from(scale) / 100.).abs() <= 2.);
            assert!((state.height as f32 - 300. * f32::from(scale) / 100.).abs() <= 2.);
            assert_eq!((state.x, state.y), (85, 90));
            assert!(!state.rescale(scale));
            state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            assert!(!state.rescale(scale));
        }
        state.capture(Bounds::new(
            point(px(85.), px(90.)),
            size(px(460.), px(370.)),
        ));
        assert!(!state.rescale(100));
        assert_eq!((state.width, state.height), (460, 370));
        assert!(state.rescale(200));
        assert_eq!((state.width, state.height), (920, 740));
    }

    #[test]
    fn content_fit_compacts_old_height_without_fighting_manual_vertical_resize() {
        let mut fit = WidgetContentFit::default();
        assert_eq!(fit.height(px(380.), px(283.25), px(900.)), Some(px(284.)));
        // Old/manual window height is deliberately not part of the key.
        assert_eq!(fit.height(px(380.), px(283.25), px(900.)), None);
        assert_eq!(fit.height(px(380.), px(283.26), px(900.)), None);
        // Moving the window must not undo a manual height when content fits.
        assert_eq!(fit.height(px(380.), px(283.25), px(800.)), None);
        assert_eq!(fit.height(px(420.), px(283.25), px(900.)), Some(px(284.)));
        assert_eq!(fit.height(px(420.), px(190.), px(900.)), Some(px(190.)));
        assert_eq!(fit.height(px(420.), px(1200.), px(430.)), Some(px(430.)));
        assert_eq!(fit.height(px(420.), px(190.), px(150.)), Some(px(150.)));
        assert_eq!(fit.height(px(420.), px(f32::NAN), px(900.)), None);
        assert_eq!(fit.height(px(420.), px(0.), px(900.)), None);
    }

    #[test]
    fn offscreen_and_oversized_bounds_stay_on_a_work_area() {
        let mut state = WidgetWindowState {
            x: i32::MAX,
            y: i32::MIN,
            width: u32::MAX,
            height: u32::MAX,
            ..Default::default()
        };
        state.clamp(&[Bounds::new(
            point(px(-1920.), px(0.)),
            size(px(1920.), px(1040.)),
        )]);
        assert!(state.x >= -1920 && state.x + state.width as i32 <= 0);
        assert!(state.y >= 0 && state.y + state.height as i32 <= 1040);
    }
}
