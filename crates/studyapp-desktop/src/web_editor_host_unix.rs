//! System WebKit host. Linux deliberately uses X11 (including XWayland).
use super::{EDITOR_PAGE, editor_page_allowed, embedded_response, ipc_allowed};
#[cfg(target_os = "macos")]
use anyhow::ensure;
use anyhow::{Context as _, Result};
use gpui_kit::{Bounds, Pixels, Window, px};
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    path::PathBuf,
    rc::Rc,
    sync::mpsc::Sender,
};
use studyapp_ui::bubble::OverlayShape;
use wry::{
    NewWindowResponse, PermissionResponse, WebContext, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
    raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle},
};

#[derive(Clone)]
pub struct ParentWindow {
    raw: RawWindowHandle,
    _ui_thread: PhantomData<Rc<()>>,
}

impl ParentWindow {
    pub fn capture(window: &Window) -> Result<Self> {
        let raw = HasWindowHandle::window_handle(window)?.as_raw();
        #[cfg(target_os = "macos")]
        ensure!(
            matches!(raw, RawWindowHandle::AppKit(_)),
            "编辑器需要 macOS 原生窗口"
        );
        #[cfg(target_os = "linux")]
        let raw = xlib_parent(raw)?;
        Ok(Self {
            raw,
            _ui_thread: PhantomData,
        })
    }
}

#[cfg(target_os = "linux")]
fn xlib_parent(raw: RawWindowHandle) -> Result<RawWindowHandle> {
    match raw {
        RawWindowHandle::Xlib(_) => Ok(raw),
        RawWindowHandle::Xcb(xcb) => {
            // XCB and Xlib use the same server-side XIDs. Wry accepts only the
            // Xlib representation, while GPUI exposes its XCB connection.
            let mut xlib = wry::raw_window_handle::XlibWindowHandle::new(xcb.window.get().into());
            xlib.visual_id = xcb.visual_id.map_or(0, |id| id.get().into());
            Ok(RawWindowHandle::Xlib(xlib))
        }
        _ => anyhow::bail!("编辑器需要 X11 或 XWayland 会话"),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn gpui_xcb_window_becomes_wry_xlib_without_changing_server_ids() {
        let mut xcb = wry::raw_window_handle::XcbWindowHandle::new(123_u32.try_into().unwrap());
        xcb.visual_id = Some(456_u32.try_into().unwrap());
        let RawWindowHandle::Xlib(xlib) = xlib_parent(RawWindowHandle::Xcb(xcb)).unwrap() else {
            panic!("Wry requires Xlib");
        };
        assert_eq!(xlib.window, 123);
        assert_eq!(xlib.visual_id, 456);
    }
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        // SAFETY: GPUI owns the parent for the entire lifetime of its editor entity.
        // This handle and the child remain on the foreground thread.
        Ok(unsafe { WindowHandle::borrow_raw(self.raw) })
    }
}

pub struct HostedEditor {
    webview: WebView,
    _context: WebContext,
    _parent: ParentWindow,
    bounds: Cell<Option<Bounds<Pixels>>>,
    requested_visible: Cell<bool>,
    applied_visible: Cell<bool>,
    occlusions: RefCell<Vec<OverlayShape>>,
}

impl HostedEditor {
    pub fn new(parent: ParentWindow, sender: Sender<String>, profile: PathBuf) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            if !gtk::is_initialized() {
                // GDK otherwise prefers Wayland even when the GPUI parent is
                // X11. Wry's child-window embedding requires an X11 GDK display.
                gtk::gdk::set_allowed_backends("x11");
            }
            gtk::init().context("无法初始化 GTK，请在 X11/XWayland 桌面会话运行")?;
        }
        std::fs::create_dir_all(&profile)?;
        let mut context = WebContext::new(Some(profile));
        let builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_custom_protocol("studyapp".into(), |_, request| embedded_response(request))
            .with_url(EDITOR_PAGE)
            .with_visible(false)
            .with_focused(false)
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(0., 0.).into(),
                size: LogicalSize::new(1., 1.).into(),
            })
            .with_devtools(false)
            .with_hotkeys_zoom(false)
            .with_clipboard(false)
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_navigation_handler(|url| editor_page_allowed(&url))
            .with_ipc_handler(move |request| {
                if !ipc_allowed(&request.uri().to_string(), request.body().len()) {
                    return;
                }
                // OS file paths must never be accepted from page JavaScript.
                #[derive(serde::Deserialize)]
                struct Kind<'a> {
                    #[serde(rename = "type", borrow)]
                    kind: &'a str,
                }
                if serde_json::from_str::<Kind<'_>>(request.body())
                    .is_ok_and(|message| message.kind != "externalDrag")
                {
                    let _ = sender.send(request.into_body());
                }
            });
        #[cfg(feature = "ui-test")]
        let builder = builder.with_initialization_script("window.__STUDYAPP_EDITOR_TEST__ = true;");
        let webview = builder
            .build_as_child(&parent)
            .context("无法启动系统 WebKit 编辑器")?;
        Ok(Self {
            webview,
            _context: context,
            _parent: parent,
            bounds: Cell::new(None),
            requested_visible: Cell::new(false),
            applied_visible: Cell::new(false),
            occlusions: RefCell::new(Vec::new()),
        })
    }

    /// Called outside GPUI entity/window borrows: GTK callbacks may dispatch native events.
    #[cfg(target_os = "linux")]
    pub fn pump_events() {
        if gtk::is_initialized_main_thread() {
            for _ in 0..64 {
                if !gtk::events_pending() {
                    break;
                }
                gtk::main_iteration_do(false);
            }
        }
    }

    pub fn evaluate(&self, script: &str) -> Result<()> {
        self.webview.evaluate_script(script)?;
        Ok(())
    }

    pub fn set_bounds(&self, bounds: Bounds<Pixels>) -> Result<()> {
        if self.bounds.get() != Some(bounds) {
            self.webview.set_bounds(wry::Rect {
                position: LogicalPosition::new(
                    f64::from(bounds.origin.x),
                    f64::from(bounds.origin.y),
                )
                .into(),
                size: LogicalSize::new(
                    f64::from(bounds.size.width.max(px(1.))),
                    f64::from(bounds.size.height.max(px(1.))),
                )
                .into(),
            })?;
            self.bounds.set(Some(bounds));
        }
        self.update_visibility()
    }

    pub fn set_occlusions(&self, shapes: Vec<OverlayShape>) -> Result<()> {
        *self.occlusions.borrow_mut() = shapes;
        self.update_visibility()
    }

    pub fn set_visible(&self, visible: bool) -> Result<()> {
        self.requested_visible.set(visible);
        self.update_visibility()
    }

    fn update_visibility(&self) -> Result<()> {
        // Native child views paint above GPUI. Hide only while an overlapping
        // floating surface is present; the document/selection stays mounted.
        let covered = self.bounds.get().is_some_and(|editor| {
            self.occlusions.borrow().iter().any(|shape| {
                let bounds = match shape {
                    OverlayShape::RoundedRect { bounds, .. } => *bounds,
                    OverlayShape::Triangle(points) => {
                        let min = gpui_kit::point(
                            points.iter().map(|p| p.x).min().unwrap(),
                            points.iter().map(|p| p.y).min().unwrap(),
                        );
                        let max = gpui_kit::point(
                            points.iter().map(|p| p.x).max().unwrap(),
                            points.iter().map(|p| p.y).max().unwrap(),
                        );
                        Bounds::from_corners(min, max)
                    }
                };
                let intersection = editor.intersect(&bounds);
                intersection.size.width > px(0.) && intersection.size.height > px(0.)
            })
        });
        let visible = self.requested_visible.get() && !covered;
        if self.applied_visible.get() != visible {
            self.webview.set_visible(visible)?;
            self.applied_visible.set(visible);
        }
        Ok(())
    }
}
