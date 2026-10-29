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
    #[cfg(target_os = "linux")]
    scale_factor: f64,
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
            #[cfg(target_os = "linux")]
            scale_factor: f64::from(window.scale_factor()),
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
    // Fields drop in declaration order: destroy WebKit before its GTK host.
    #[cfg(target_os = "linux")]
    gtk_host: LinuxHost,
    _context: WebContext,
    _parent: ParentWindow,
    bounds: Cell<Option<Bounds<Pixels>>>,
    requested_visible: Cell<bool>,
    applied_visible: Cell<bool>,
    occlusions: RefCell<Vec<OverlayShape>>,
}

/// GTK owns this native window and its frame clock. Only the GPUI parent is
/// foreign; replacing GtkWindow's own GdkWindow breaks GTK's rendering lifecycle.
#[cfg(target_os = "linux")]
struct LinuxHost {
    window: gtk::Window,
    container: gtk::Box,
    native: gtk::gdk::Window,
    _parent: gtk::gdk::Window,
    display: gdkx11::X11Display,
    parent_id: std::os::raw::c_ulong,
    gpui_scale: f64,
}

#[cfg(target_os = "linux")]
impl LinuxHost {
    fn new(parent: &ParentWindow) -> Result<Self> {
        use gtk::{glib::translate::*, prelude::*};
        let RawWindowHandle::Xlib(raw) = parent.raw else {
            anyhow::bail!("编辑器需要 X11 父窗口");
        };
        let display = gtk::gdk::Display::default()
            .and_then(|display| display.downcast::<gdkx11::X11Display>().ok())
            .context("编辑器需要 X11 GDK 显示连接")?;
        // The C function may return null when the GPUI window has closed.
        let foreign: Option<gtk::gdk::Window> = unsafe {
            from_glib_full(gdkx11::ffi::gdk_x11_window_foreign_new_for_display(
                display.to_glib_none().0,
                raw.window,
            ))
        };
        let foreign = foreign.context("编辑器父窗口已不可用")?;
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_decorated(false);
        window.set_focus_on_map(false);
        window.set_accept_focus(true);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_default_size(1, 1);
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&container);
        window.realize();
        let Some(native) = window.window() else {
            // GTK retains toplevels; dropping a Rust reference alone is not enough.
            unsafe { window.destroy() };
            anyhow::bail!("无法创建 GTK 编辑器窗口");
        };
        let host = Self {
            window,
            container,
            native,
            _parent: foreign,
            display,
            parent_id: raw.window,
            gpui_scale: parent.scale_factor,
        };
        // Before first map, embed the GTK-owned window. GDK preserves its
        // toplevel/frame-clock bookkeeping when the parent is foreign.
        host.native.reparent(&host._parent, 0, 0);
        anyhow::ensure!(host.native.frame_clock().is_some(), "GTK 编辑器缺少帧时钟");
        Ok(host)
    }

    fn geometry(&self, bounds: Bounds<Pixels>) -> (LogicalPosition<i32>, LogicalSize<i32>) {
        use gtk::prelude::*;
        // GPUI and GDK may choose different scale factors. Map through physical
        // pixels so the embedded rectangle stays aligned with the GPUI canvas.
        let ratio = self.gpui_scale / f64::from(self.window.scale_factor().max(1));
        (
            LogicalPosition::new(
                f64::from(bounds.origin.x) * ratio,
                f64::from(bounds.origin.y) * ratio,
            )
            .cast(),
            LogicalSize::new(
                f64::from(bounds.size.width.max(px(1.))) * ratio,
                f64::from(bounds.size.height.max(px(1.))) * ratio,
            )
            .cast(),
        )
    }

    fn set_bounds(&self, bounds: Bounds<Pixels>) {
        use gtk::prelude::*;
        let (position, size) = self.geometry(bounds);
        self.window.resize(size.width.max(1), size.height.max(1));
        self.native.move_resize(
            position.x,
            position.y,
            size.width.max(1),
            size.height.max(1),
        );
        self.window.size_allocate(&gtk::Allocation::new(
            0,
            0,
            size.width.max(1),
            size.height.max(1),
        ));
    }

    fn restore_parent_focus(&self) {
        use gtk::{glib::translate::ToGlibPtr, prelude::*};
        // Restore only focus in our GTK subtree, never another application's.
        unsafe {
            let display = gdkx11::ffi::gdk_x11_display_get_xdisplay(self.display.to_glib_none().0);
            let own = gdkx11::ffi::gdk_x11_window_get_xid(self.native.as_ptr().cast());
            let mut focused = 0;
            let mut revert = 0;
            self.display.error_trap_push();
            x11::xlib::XGetInputFocus(display, &mut focused, &mut revert);
            let mut candidate = focused;
            let mut ours = false;
            for _ in 0..32 {
                if candidate == own {
                    ours = true;
                    break;
                }
                if candidate <= x11::xlib::PointerRoot as x11::xlib::Window
                    || candidate == self.parent_id
                {
                    break;
                }
                let (mut root, mut parent) = (0, 0);
                let mut children = std::ptr::null_mut();
                let mut count = 0;
                let ok = x11::xlib::XQueryTree(
                    display,
                    candidate,
                    &mut root,
                    &mut parent,
                    &mut children,
                    &mut count,
                );
                if !children.is_null() {
                    x11::xlib::XFree(children.cast());
                }
                if ok == 0 || parent == candidate || candidate == root {
                    break;
                }
                candidate = parent;
            }
            if ours {
                x11::xlib::XSetInputFocus(
                    display,
                    self.parent_id,
                    x11::xlib::RevertToParent,
                    x11::xlib::CurrentTime,
                );
            }
            self.display.error_trap_pop_ignored();
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for LinuxHost {
    fn drop(&mut self) {
        use gtk::prelude::*;
        self.restore_parent_focus();
        // The WebView field has already been dropped on the GTK foreground thread.
        unsafe { self.window.destroy() };
        // GPUI uses a separate XCB connection. Complete GTK's destruction requests
        // before the caller can destroy the GPUI parent on that connection.
        self.display.sync();
    }
}

#[cfg(all(target_os = "linux", feature = "ui-test"))]
struct PaintDiagnostic {
    widget: gtk::glib::WeakRef<gtk::Widget>,
    draws: Rc<Cell<u64>>,
}

#[cfg(all(target_os = "linux", feature = "ui-test"))]
thread_local! {
    static PAINT_DIAGNOSTICS: RefCell<Vec<PaintDiagnostic>> = const { RefCell::new(Vec::new()) };
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
        #[cfg(target_os = "linux")]
        let gtk_host = LinuxHost::new(&parent)?;
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
            .with_drag_drop_handler(super::native_drag::handler(sender.clone()))
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
        #[cfg(not(target_os = "linux"))]
        let webview = builder
            .build_as_child(&parent)
            .context("无法启动系统 WebKit 编辑器")?;
        #[cfg(target_os = "linux")]
        let webview = {
            use gtk::{glib::translate::ToGlibPtr, prelude::*};
            use wry::{WebViewBuilderExtUnix, WebViewExtUnix};
            let webview = builder
                .build_gtk(&gtk_host.container)
                .context("无法启动 GTK WebKit 编辑器")?;
            let native = gtk_host.native.downgrade();
            let display = gtk_host.display.clone();
            webview
                .webview()
                .connect_button_press_event(move |_, event| {
                    if let Some(native) = native.upgrade() {
                        // A real click transfers keyboard focus into the embedded
                        // window. Do not ask the WM to activate an unmanaged child.
                        unsafe {
                            display.error_trap_push();
                            x11::xlib::XSetInputFocus(
                                gdkx11::ffi::gdk_x11_display_get_xdisplay(display.to_glib_none().0),
                                gdkx11::ffi::gdk_x11_window_get_xid(native.as_ptr().cast()),
                                x11::xlib::RevertToParent,
                                event.time().into(),
                            );
                            display.error_trap_pop_ignored();
                        }
                    }
                    gtk::glib::Propagation::Proceed
                });
            webview
        };
        #[cfg(all(target_os = "linux", feature = "ui-test"))]
        {
            use gtk::prelude::*;
            use wry::WebViewExtUnix;
            let widget: gtk::Widget = webview.webview().upcast();
            let draws = Rc::new(Cell::new(0_u64));
            let counter = draws.clone();
            widget.connect_draw(move |_, _| {
                counter.set(counter.get() + 1);
                gtk::glib::Propagation::Proceed
            });
            PAINT_DIAGNOSTICS.with(|entries| {
                entries.borrow_mut().push(PaintDiagnostic {
                    widget: widget.downgrade(),
                    draws,
                });
            });
            Self::diagnostic_windows("constructed");
        }
        Ok(Self {
            webview,
            #[cfg(target_os = "linux")]
            gtk_host,
            _context: context,
            _parent: parent,
            bounds: Cell::new(None),
            requested_visible: Cell::new(false),
            applied_visible: Cell::new(false),
            occlusions: RefCell::new(Vec::new()),
        })
    }

    /// Return owned GDK references so diagnostics can dispatch draw callbacks
    /// after all GPUI entity/window borrows have ended. Never retains the widget.
    #[cfg(all(target_os = "linux", feature = "ui-test"))]
    pub fn diagnostic_windows(stage: &str) -> Vec<gtk::gdk::Window> {
        use gtk::prelude::*;
        PAINT_DIAGNOSTICS.with(|entries| {
            let mut entries = entries.borrow_mut();
            entries.retain(|entry| entry.widget.upgrade().is_some());
            entries.iter().filter_map(|entry| {
                let widget = entry.widget.upgrade()?;
                eprintln!(
                    "native paint [{stage}]: realized={} mapped={} visible={} allocation={:?} clip={:?} widget_clock={} draws={}",
                    widget.is_realized(), widget.is_mapped(), widget.is_visible(),
                    widget.allocation(), widget.clip(), widget.frame_clock().is_some(), entry.draws.get()
                );
                let mut ancestor = widget.parent();
                while let Some(parent) = ancestor {
                    eprintln!(
                        "native paint [{stage}]: parent={} mapped={} visible={} allocation={:?} clip={:?} clock={}",
                        parent.type_().name(), parent.is_mapped(), parent.is_visible(),
                        parent.allocation(), parent.clip(), parent.frame_clock().is_some()
                    );
                    ancestor = parent.parent();
                }
                let window = widget.window()?;
                let top = window.toplevel();
                eprintln!(
                    "native paint [{stage}]: window={:?} size={}x{} visible={} viewable={} clock={} top={:?} top_clock={}",
                    window.window_type(), window.width(), window.height(),
                    window.is_visible(), window.is_viewable(), window.frame_clock().is_some(),
                    top.window_type(), top.frame_clock().is_some()
                );
                Some(top)
            }).collect()
        })
    }

    #[cfg(all(target_os = "linux", feature = "ui-test"))]
    pub fn assert_visible_content_allocated() {
        use gtk::prelude::*;
        PAINT_DIAGNOSTICS.with(|entries| {
            let entries = entries.borrow();
            let visible: Vec<_> = entries
                .iter()
                .filter_map(|entry| entry.widget.upgrade())
                .filter(|widget| widget.is_visible())
                .collect();
            assert!(!visible.is_empty(), "no visible native editor");
            for widget in visible {
                assert!(widget.is_mapped(), "native editor is not mapped");
                assert!(
                    widget.frame_clock().is_some(),
                    "native editor has no GTK frame clock"
                );
                let size = (widget.allocated_width(), widget.allocated_height());
                assert!(size.0 > 1 && size.1 > 1, "native editor is only {size:?}");
                let container = widget.parent().expect("native editor GTK container");
                let top = container.parent().expect("native editor GTK window");
                for parent in [container, top] {
                    assert!(
                        parent.frame_clock().is_some(),
                        "parent {} has no GTK frame clock",
                        parent.type_().name()
                    );
                    assert_eq!(
                        size,
                        (parent.allocated_width(), parent.allocated_height()),
                        "native editor does not fill parent {} (clip {:?})",
                        parent.type_().name(),
                        parent.clip()
                    );
                }
            }
        });
    }

    #[cfg(all(target_os = "linux", feature = "ui-test"))]
    pub fn visible_native_test_window() -> u64 {
        use gtk::prelude::*;
        PAINT_DIAGNOSTICS.with(|entries| {
            let widget = entries
                .borrow()
                .iter()
                .filter_map(|entry| entry.widget.upgrade())
                .find(|widget| widget.is_visible() && widget.is_mapped())
                .expect("visible native editor for remote input regression");
            let window = widget.window().expect("realized native editor");
            // GTK's X11 backend owns this live window; GTK retains its reference.
            unsafe { gdkx11::ffi::gdk_x11_window_get_xid(window.as_ptr().cast()) as u64 }
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
            #[cfg(target_os = "linux")]
            self.gtk_host.set_bounds(bounds);
            #[cfg(not(target_os = "linux"))]
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

    #[cfg(target_os = "linux")]
    pub fn restore_focus_for_parent_click(&self, position: gpui_kit::Point<Pixels>) {
        // XInput events from the child can also reach the GPUI parent. Never
        // undo the native click-to-focus bridge while the click is in its area.
        if self.applied_visible.get()
            && self
                .bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&position))
        {
            return;
        }
        // This checks the X11 focus ancestry before returning focus, so a click
        // cannot take focus away from a different application or another child.
        self.gtk_host.restore_parent_focus();
    }

    #[cfg(target_os = "linux")]
    fn allocate_linux_content(&self) -> Result<()> {
        use gtk::prelude::*;
        use wry::WebViewExtUnix;

        let Some(bounds) = self.bounds.get() else {
            return Ok(());
        };
        let (_, size) = self.gtk_host.geometry(bounds);
        let widget = self.webview.webview();
        let container = widget
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Box>().ok())
            .context("系统 WebKit 编辑器缺少 GTK 布局容器")?;
        let top = container
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Window>().ok())
            .context("系统 WebKit 编辑器缺少 GTK 宿主窗口")?;
        let allocation = gtk::Allocation::new(0, 0, size.width, size.height);
        if top.allocation() != allocation
            || container.allocation() != allocation
            || widget.allocated_width() != size.width
            || widget.allocated_height() != size.height
        {
            // Apply GPUI's layout synchronously after showing hidden children;
            // subsequent page repaints use GTK's normal frame-clock lifecycle.
            top.size_allocate(&allocation);
            container.size_allocate(&allocation);
            #[cfg(feature = "ui-test")]
            Self::diagnostic_windows("allocated");
        }
        Ok(())
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
            #[cfg(target_os = "linux")]
            {
                use gtk::prelude::*;
                if visible {
                    self.gtk_host.window.show_all();
                    if let Some(bounds) = self.bounds.get() {
                        self.gtk_host.set_bounds(bounds);
                    }
                } else {
                    self.gtk_host.restore_parent_focus();
                    self.gtk_host.window.hide();
                }
            }
            self.webview.set_visible(visible)?;
            self.applied_visible.set(visible);
        }
        #[cfg(target_os = "linux")]
        if visible {
            self.allocate_linux_content()?;
        }
        Ok(())
    }
}
