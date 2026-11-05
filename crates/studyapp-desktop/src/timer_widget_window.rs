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
        self.width = self.width.clamp(280, 900).min(width);
        self.height = self.height.clamp(200, 1000).min(height);
        let x = f32::from(display.origin.x).round() as i32;
        let y = f32::from(display.origin.y).round() as i32;
        self.x = self
            .x
            .clamp(x, x.saturating_add((width - self.width) as i32));
        self.y = self
            .y
            .clamp(y, y.saturating_add((height - self.height) as i32));
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
        let desired = (natural.ceil().min(1000.) as u32).clamp(200.min(maximum), maximum);
        let key = (width.round() as u32, desired);
        if self.last == Some(key) {
            return None;
        }
        self.last = Some(key);
        Some(px(desired as f32))
    }
}

pub fn decorate(window: &mut gpui_kit::Window, pinned: bool) -> Result<()> {
    #[cfg(all(windows, not(test)))]
    {
        use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(raw) = window.window_handle()?.as_raw() else {
            anyhow::bail!("计时窗口句柄无效")
        };
        let hwnd = HWND(raw.hwnd.get() as *mut _);
        // Only this application's new floating window is modified.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
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
            let applied = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            ensure!(
                applied & WS_EX_TOOLWINDOW.0 as isize != 0
                    && applied & WS_EX_APPWINDOW.0 as isize == 0,
                "计时窗口装饰未生效"
            );
            ensure!(
                (applied & WS_EX_TOPMOST.0 as isize != 0) == pinned,
                "计时窗口置顶状态未生效"
            );
        }
    }
    #[cfg(any(not(windows), test))]
    {
        let _ = (window, pinned);
    }
    Ok(())
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
    fn content_fit_compacts_old_height_without_fighting_manual_vertical_resize() {
        let mut fit = WidgetContentFit::default();
        assert_eq!(fit.height(px(380.), px(283.25), px(900.)), Some(px(284.)));
        // Old/manual window height is deliberately not part of the key.
        assert_eq!(fit.height(px(380.), px(283.25), px(900.)), None);
        assert_eq!(fit.height(px(380.), px(283.26), px(900.)), None);
        // Moving the window must not undo a manual height when content fits.
        assert_eq!(fit.height(px(380.), px(283.25), px(800.)), None);
        assert_eq!(fit.height(px(420.), px(283.25), px(900.)), Some(px(284.)));
        assert_eq!(fit.height(px(420.), px(190.), px(900.)), Some(px(200.)));
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
