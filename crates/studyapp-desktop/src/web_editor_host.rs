//! The native child-window host for the continuous Markdown editor.

#[cfg(windows)]
#[path = "native_surface_geometry.rs"]
mod surface_geometry;

#[cfg(windows)]
const EDITOR_PAGE: &str = "https://studyapp.editor/index.html";
#[cfg(not(windows))]
const EDITOR_PAGE: &str = "studyapp://editor/index.html";

// JSON may expand each raw byte to six bytes (\u0000); allow the full 10 MiB
// document plus bounded transaction metadata. The document itself is checked
// separately by the native protocol and by the editor transaction filter.
const MAX_IPC_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorAsset {
    Html,
    Script,
    Stylesheet,
}

fn editor_asset(url: &str) -> Option<EditorAsset> {
    match url {
        "studyapp://editor/index.html" => Some(EditorAsset::Html),
        "studyapp://editor/editor.js" => Some(EditorAsset::Script),
        "studyapp://editor/editor.css" => Some(EditorAsset::Stylesheet),
        _ => None,
    }
}

fn editor_page_allowed(url: &str) -> bool {
    url == EDITOR_PAGE
        || url
            .strip_prefix(EDITOR_PAGE)
            .is_some_and(|suffix| suffix.starts_with('#'))
}

fn ipc_allowed(url: &str, bytes: usize) -> bool {
    editor_page_allowed(url) && bytes <= MAX_IPC_BYTES
}

#[cfg(windows)]
pub use windows_host::{HostedEditor, ParentWindow};

#[cfg(not(windows))]
#[path = "web_editor_host_unix.rs"]
mod unix_host;
#[cfg(not(windows))]
pub use unix_host::{HostedEditor, ParentWindow};

#[cfg(windows)]
mod windows_host {
    use super::{
        editor_page_allowed, embedded_response, ipc_allowed,
        surface_geometry::{
            DragGate, PhysicalRect, PhysicalShape, clipped_occlusions, physical_overlay_shapes,
        },
    };
    use anyhow::{Context as _, Result, ensure};
    use gpui_kit::{Bounds, Pixels, Window};
    use std::{
        cell::{Cell, RefCell},
        marker::PhantomData,
        path::PathBuf,
        rc::Rc,
        sync::mpsc::Sender,
    };
    use studyapp_ui::bubble::OverlayShape;
    use wry::{
        DragDropEvent, NewWindowResponse, PermissionResponse, WebContext, WebView, WebViewBuilder,
        WebViewBuilderExtWindows, WebViewExtWindows,
        dpi::{LogicalPosition, LogicalSize},
        raw_window_handle::{
            HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
        },
    };

    /// An owned copy of the HWND, without a borrow of the GPUI window or App.
    ///
    /// This deliberately remains !Send and !Sync. The caller must keep its GPUI
    /// window alive while creating/using the child, and create it on the same UI
    /// thread. It is not an owner of the actual Windows window.
    #[derive(Clone, Debug)]
    pub struct ParentWindow {
        raw: Win32WindowHandle,
        _ui_thread: PhantomData<Rc<()>>,
    }

    impl ParentWindow {
        pub fn capture(window: &Window) -> Result<Self> {
            // Window also has an inherent window_handle() returning a GPUI
            // AnyWindowHandle. Use the trait explicitly to obtain the HWND.
            let handle =
                HasWindowHandle::window_handle(window).context("无法取得编辑器所属窗口句柄")?;
            let RawWindowHandle::Win32(raw) = handle.as_raw() else {
                anyhow::bail!("Markdown 编辑器需要 Windows 原生窗口");
            };
            Ok(Self {
                raw,
                _ui_thread: PhantomData,
            })
        }
    }

    impl HasWindowHandle for ParentWindow {
        fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
            // SAFETY: capture obtains this handle from a live GPUI window.
            // The caller contract keeps that parent alive for the child lifetime.
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(self.raw)) })
        }
    }

    /// A single, persistent WebView for the continuous document surface.
    ///
    /// Keep this on the foreground thread. In particular, new() pumps Windows
    /// messages internally: call it from the body of a foreground async task,
    /// outside any GPUI App/entity/window update or render borrow.
    type EditorGeometry = (f64, f64, f64, f64, u32);
    pub struct HostedEditor {
        // Fields drop in declaration order: release the child before its context.
        webview: WebView,
        _context: WebContext,
        _parent: ParentWindow,
        bounds: Cell<Option<EditorGeometry>>,
        visible: Cell<bool>,
        occlusions: RefCell<Vec<OverlayShape>>,
        applied_occlusions: RefCell<Vec<PhysicalRect>>,
        applied_shapes: RefCell<Vec<PhysicalShape>>,
        applied_region_size: Cell<Option<(i32, i32)>>,
        #[cfg(feature = "ui-test")]
        occlusion_updates: Cell<u64>,
        #[cfg(feature = "ui-test")]
        bounds_updates: Cell<u64>,
        #[cfg(feature = "ui-test")]
        visibility_updates: Cell<u64>,
    }

    impl HostedEditor {
        pub fn new(parent: ParentWindow, sender: Sender<String>, profile: PathBuf) -> Result<Self> {
            std::fs::create_dir_all(&profile).context("无法创建本地编辑器运行目录")?;
            let mut context = WebContext::new(Some(profile));
            let drag_sender = sender.clone();
            let drag_gate = RefCell::new(DragGate::default());
            let builder = WebViewBuilder::new_with_web_context(&mut context)
                .with_custom_protocol("studyapp".to_owned(), |_, request| {
                    embedded_response(request)
                })
                .with_https_scheme(true)
                // Wry maps this to https://studyapp.editor/index.html on Windows.
                .with_url("studyapp://editor/index.html")
                .with_visible(false)
                .with_focused(false)
                .with_bounds(wry::Rect {
                    position: LogicalPosition::new(0.0, 0.0).into(),
                    size: LogicalSize::new(1.0, 1.0).into(),
                })
                .with_background_color((255, 255, 255, 255))
                .with_devtools(false)
                .with_browser_accelerator_keys(false)
                .with_default_context_menus(false)
                .with_general_autofill_enabled(false)
                .with_hotkeys_zoom(false)
                // Keyboard cut/copy/paste remain native editing operations.
                // Do not grant page JavaScript ambient clipboard-read permission.
                .with_clipboard(false)
                .with_permission_handler(|_| PermissionResponse::Deny)
                .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                .with_download_started_handler(|_, _| false)
                .with_navigation_handler(|url| editor_page_allowed(&url))
                .with_drag_drop_handler(move |event| {
                    // Only OS-provided paths enter this trusted host channel.
                    // Over carries position only: suppress it to avoid invalidating
                    // the shell on every native pointer movement.
                    let change = match event {
                        DragDropEvent::Enter { paths, .. } => drag_gate
                            .borrow_mut()
                            .enter(paths.clone())
                            .then_some(("enter", paths)),
                        DragDropEvent::Drop { paths, .. } => drag_gate
                            .borrow_mut()
                            .drop_paths(paths.clone())
                            .then_some(("drop", paths)),
                        DragDropEvent::Leave => drag_gate
                            .borrow_mut()
                            .leave()
                            .then_some(("leave", Vec::new())),
                        _ => None,
                    };
                    if let Some((phase, paths)) = change {
                        let _ = drag_sender.send(
                            serde_json::json!({
                                "type": "externalDrag", "phase": phase, "paths": paths
                            })
                            .to_string(),
                        );
                    }
                    true
                })
                .with_ipc_handler(move |request| {
                    if ipc_allowed(&request.uri().to_string(), request.body().len()) {
                        #[derive(serde::Deserialize)]
                        struct MessageKind<'a> {
                            #[serde(rename = "type", borrow)]
                            kind: &'a str,
                        }
                        // The page cannot forge a native file drag event.
                        if !serde_json::from_str::<MessageKind<'_>>(request.body())
                            .is_ok_and(|message| message.kind != "externalDrag")
                        {
                            return;
                        }
                        // Callbacks never enter GPUI: the editor drains this queue
                        // on its foreground task and validates the JSON protocol.
                        let _ = sender.send(request.into_body());
                    }
                });
            #[cfg(feature = "ui-test")]
            let builder =
                builder.with_initialization_script("window.__STUDYAPP_EDITOR_TEST__ = true;");
            let webview = builder
                .build_as_child(&parent)
                .context("无法启动连续 Markdown 编辑器，请检查 WebView2 运行时")?;
            Ok(Self {
                webview,
                _context: context,
                _parent: parent,
                bounds: Cell::new(None),
                visible: Cell::new(false),
                occlusions: RefCell::new(Vec::new()),
                applied_occlusions: RefCell::new(Vec::new()),
                applied_shapes: RefCell::new(Vec::new()),
                applied_region_size: Cell::new(None),
                #[cfg(feature = "ui-test")]
                occlusion_updates: Cell::new(0),
                #[cfg(feature = "ui-test")]
                bounds_updates: Cell::new(0),
                #[cfg(feature = "ui-test")]
                visibility_updates: Cell::new(0),
            })
        }

        /// Queues script execution. A successful return does not acknowledge its
        /// result; saves must wait for the matching frontend IPC flush response.
        pub fn evaluate(&self, script: &str) -> Result<()> {
            self.webview
                .evaluate_script(script)
                .context("无法向 Markdown 编辑器发送操作")
        }

        /// GPUI Pixels are logical units; Wry applies the window DPI conversion.
        /// Overlays are clipped separately, without changing this document rectangle.
        pub fn set_bounds(&self, bounds: Bounds<Pixels>) -> Result<()> {
            let x = f64::from(f32::from(bounds.origin.x));
            let y = f64::from(f32::from(bounds.origin.y));
            let width = f64::from(f32::from(bounds.size.width));
            let height = f64::from(f32::from(bounds.size.height));
            ensure!(
                [x, y, width, height].iter().all(|v| v.is_finite())
                    && width >= 0.0
                    && height >= 0.0,
                "Markdown 编辑器布局尺寸无效"
            );
            // A window can move to another monitor without changing logical
            // bounds. Include DPI so the child still receives new physical bounds.
            let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.webview.hwnd()) };
            ensure!(dpi != 0, "Markdown 编辑器窗口已失效");
            let geometry = (x, y, width, height, dpi);
            if self.bounds.get() == Some(geometry) {
                return Ok(());
            }
            self.webview
                .set_bounds(wry::Rect {
                    position: LogicalPosition::new(x, y).into(),
                    size: LogicalSize::new(width, height).into(),
                })
                .context("无法更新 Markdown 编辑器布局")?;
            self.bounds.set(Some(geometry));
            #[cfg(feature = "ui-test")]
            self.bounds_updates.set(self.bounds_updates.get() + 1);
            self.apply_occlusions()?;
            Ok(())
        }

        /// Parent-client bounds of opaque cards. Never cut a shadow-sized
        /// rectangle from Chromium: its exposed corners would become solid.
        pub fn set_occlusions(&self, shapes: Vec<OverlayShape>) -> Result<()> {
            *self.occlusions.borrow_mut() = shapes;
            self.apply_occlusions()
        }

        fn apply_occlusions(&self) -> Result<()> {
            use windows::Win32::Graphics::Gdi::{
                CombineRgn, CreatePolygonRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject,
                GDI_REGION_TYPE, RGN_DIFF, SetWindowRgn, WINDING,
            };
            let Some((x, y, width, height, dpi)) = self.bounds.get() else {
                return Ok(());
            };
            let shapes = self.occlusions.borrow();
            let cards: Vec<_> = shapes
                .iter()
                .filter_map(|shape| match shape {
                    OverlayShape::RoundedRect { bounds, .. } => Some((
                        f64::from(f32::from(bounds.left())),
                        f64::from(f32::from(bounds.top())),
                        f64::from(f32::from(bounds.size.width)),
                        f64::from(f32::from(bounds.size.height)),
                    )),
                    OverlayShape::Triangle(_) => None,
                })
                .collect();
            let clips = clipped_occlusions((x, y, width, height), dpi, &cards);
            let physical_shapes = physical_overlay_shapes((x, y, width, height), dpi, &shapes);
            let scale = f64::from(dpi) / 96.;
            let size = (
                (width * scale).ceil() as i32,
                (height * scale).ceil() as i32,
            );
            if *self.applied_occlusions.borrow() == clips
                && *self.applied_shapes.borrow() == physical_shapes
                && (physical_shapes.is_empty() || self.applied_region_size.get() == Some(size))
            {
                return Ok(());
            }
            // SAFETY: the child HWND belongs to this UI-thread host. Windows
            // owns a successful SetWindowRgn region; all other GDI objects are
            // released locally, including every failure path.
            unsafe {
                if physical_shapes.is_empty() {
                    ensure!(
                        SetWindowRgn(self.webview.hwnd(), None, true) != 0,
                        "无法恢复编辑器可见区域"
                    );
                } else {
                    let region = CreateRectRgn(0, 0, size.0, size.1);
                    ensure!(!region.0.is_null(), "无法创建编辑器可见区域");
                    for shape in &physical_shapes {
                        let hole = match shape {
                            PhysicalShape::RoundedRect {
                                rect: (left, top, right, bottom),
                                diameter,
                            } => CreateRoundRectRgn(
                                *left,
                                *top,
                                right + 1,
                                bottom + 1,
                                *diameter,
                                *diameter,
                            ),
                            PhysicalShape::ArrowPolygon(points) => {
                                let vertices = points
                                    .iter()
                                    .map(|&(x, y)| windows::Win32::Foundation::POINT { x, y })
                                    .collect::<Vec<_>>();
                                CreatePolygonRgn(&vertices, WINDING)
                            }
                        };
                        if hole.0.is_null() {
                            let _ = DeleteObject(region.into());
                            anyhow::bail!("无法创建通知遮挡区域");
                        }
                        let combined = CombineRgn(Some(region), Some(region), Some(hole), RGN_DIFF);
                        let _ = DeleteObject(hole.into());
                        if combined == GDI_REGION_TYPE(0) {
                            let _ = DeleteObject(region.into());
                            anyhow::bail!("无法更新通知遮挡区域");
                        }
                    }
                    if SetWindowRgn(self.webview.hwnd(), Some(region), true) == 0 {
                        let _ = DeleteObject(region.into());
                        anyhow::bail!("无法应用编辑器可见区域");
                    }
                }
            }
            *self.applied_occlusions.borrow_mut() = clips;
            *self.applied_shapes.borrow_mut() = physical_shapes;
            self.applied_region_size.set(Some(size));
            #[cfg(feature = "ui-test")]
            self.occlusion_updates.set(self.occlusion_updates.get() + 1);
            Ok(())
        }

        pub fn set_visible(&self, visible: bool) -> Result<()> {
            if self.visible.get() == visible {
                return Ok(());
            }
            self.webview
                .set_visible(visible)
                .context("无法更新 Markdown 编辑器显示状态")?;
            self.visible.set(visible);
            #[cfg(feature = "ui-test")]
            self.visibility_updates
                .set(self.visibility_updates.get() + 1);
            Ok(())
        }

        /// Inspect the HWND and controller, rather than the DOM: script execution
        /// and CDP screenshots also succeed while the native surface is hidden.
        #[cfg(feature = "ui-test")]
        pub fn native_state(&self) -> Result<serde_json::Value> {
            use windows::{
                Win32::{
                    Foundation::{HWND, POINT, RECT},
                    Graphics::Gdi::ClientToScreen,
                    UI::{
                        Input::KeyboardAndMouse::GetFocus,
                        WindowsAndMessaging::{
                            GA_ROOT, GWL_STYLE, GetAncestor, GetClassNameW, GetClientRect,
                            GetForegroundWindow, GetWindowLongW, GetWindowRect, IsChild,
                            IsWindowVisible, WS_VISIBLE, WindowFromPoint,
                        },
                    },
                },
                core::BOOL,
            };
            let child = self.webview.hwnd();
            let parent = HWND(self._parent.raw.hwnd.get() as *mut _);
            let mut rect = RECT::default();
            let mut parent_rect = RECT::default();
            let mut origin = POINT::default();
            let mut controller_visible = BOOL::default();
            // SAFETY: inspection runs on the creating thread and the host keeps
            // both native handles alive throughout the test.
            let (
                visible,
                own_visible,
                parent_visible,
                focus,
                focused,
                hit,
                hit_is_editor,
                hit_root,
                hit_class,
                foreground,
            ) = unsafe {
                GetWindowRect(child, &mut rect)?;
                GetClientRect(parent, &mut parent_rect)?;
                ensure!(
                    ClientToScreen(parent, &mut origin).as_bool(),
                    "无法定位父窗口"
                );
                self.webview
                    .controller()
                    .IsVisible(&mut controller_visible)?;
                let focus = GetFocus();
                let hit = WindowFromPoint(POINT {
                    x: (rect.left + rect.right) / 2,
                    y: (rect.top + rect.bottom) / 2,
                });
                let mut class = [0_u16; 256];
                let class_length = GetClassNameW(hit, &mut class) as usize;
                (
                    IsWindowVisible(child).as_bool(),
                    GetWindowLongW(child, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0,
                    IsWindowVisible(parent).as_bool(),
                    focus.0 as usize,
                    focus == child || IsChild(child, focus).as_bool(),
                    hit.0 as usize,
                    hit == child || IsChild(child, hit).as_bool(),
                    GetAncestor(hit, GA_ROOT).0 as usize,
                    String::from_utf16_lossy(&class[..class_length]),
                    GetForegroundWindow().0 as usize,
                )
            };
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let inside_parent = width > 0
                && height > 0
                && rect.left >= origin.x
                && rect.top >= origin.y
                && rect.right <= origin.x + parent_rect.right
                && rect.bottom <= origin.y + parent_rect.bottom;
            Ok(serde_json::json!({
                "hwnd": child.0 as usize,
                "parentHwnd": parent.0 as usize,
                "visible": visible,
                "ownVisible": own_visible,
                "parentVisible": parent_visible,
                "controllerVisible": controller_visible.as_bool(),
                "bounds": {"x":rect.left, "y":rect.top, "width":width, "height":height},
                "parentBounds": {"x":origin.x, "y":origin.y, "width":parent_rect.right, "height":parent_rect.bottom},
                "insideParent": inside_parent,
                "focusHwnd": focus,
                "focused": focused,
                "hitHwnd": hit,
                "hitIsEditor": hit_is_editor,
                "hitRootHwnd": hit_root,
                "hitClass": hit_class,
                "foregroundHwnd": foreground,
                "logicalBounds": self.bounds.get(),
                "boundsUpdates": self.bounds_updates.get(),
                "visibilityUpdates": self.visibility_updates.get(),
                "occlusionUpdates": self.occlusion_updates.get(),
                "occlusions": *self.applied_occlusions.borrow(),
                "occlusionArrowPolygons": self.applied_shapes.borrow().iter().filter_map(|shape| match shape { PhysicalShape::ArrowPolygon(points) => Some(points.clone()), _ => None }).collect::<Vec<_>>(),
                // Historical diagnostic alias retained; entries now contain the
                // complete conservative polygon, not three rounded vertices.
                "occlusionTriangles": self.applied_shapes.borrow().iter().filter_map(|shape| match shape { PhysicalShape::ArrowPolygon(points) => Some(points.clone()), _ => None }).collect::<Vec<_>>(),
            }))
        }

        /// Captures the actual native child WebView, which GPUI render_to_image
        /// cannot include. Poll/await the receiver while the message loop runs.
        #[cfg(feature = "ui-test")]
        pub fn insert_text(
            &self,
            text: &str,
        ) -> Result<std::sync::mpsc::Receiver<std::result::Result<(), String>>> {
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use windows::core::{HSTRING, w};
            self.evaluate("document.querySelector('.cm-content').focus();")?;
            let (sender, receiver) = std::sync::mpsc::channel();
            let callback =
                CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, _| {
                    let _ = sender.send(status.map_err(|error| error.to_string()));
                    Ok(())
                }));
            let parameters = HSTRING::from(serde_json::json!({"text": text}).to_string());
            // SAFETY: called on the creating STA; WebView retains the callback.
            unsafe {
                self.webview.webview().CallDevToolsProtocolMethod(
                    w!("Input.insertText"),
                    &parameters,
                    &callback,
                )
            }
            .context("无法发送原生浏览器输入")?;
            Ok(receiver)
        }

        #[cfg(feature = "ui-test")]
        pub fn dispatch_key(
            &self,
            key: &str,
            code: &str,
            modifiers: u32,
            virtual_key: i32,
        ) -> Result<std::sync::mpsc::Receiver<std::result::Result<(), String>>> {
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use windows::core::{HSTRING, w};
            let parameters = |kind| {
                HSTRING::from(
                    serde_json::json!({
                        "type": kind, "key": key, "code": code,
                        "modifiers": modifiers, "windowsVirtualKeyCode": virtual_key
                    })
                    .to_string(),
                )
            };
            let down = parameters("rawKeyDown");
            let up = parameters("keyUp");
            let webview = self.webview.webview();
            let (sender, receiver) = std::sync::mpsc::channel();
            let callback =
                CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, _| {
                    if let Err(error) = status {
                        let _ = sender.send(Err(error.to_string()));
                        return Ok(());
                    }
                    let complete_sender = sender.clone();
                    let complete = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                        move |status, _| {
                            let _ = complete_sender.send(status.map_err(|error| error.to_string()));
                            Ok(())
                        },
                    ));
                    // Send key-up only after Chromium processed key-down.
                    if let Err(error) = unsafe {
                        webview.CallDevToolsProtocolMethod(
                            w!("Input.dispatchKeyEvent"),
                            &up,
                            &complete,
                        )
                    } {
                        let _ = sender.send(Err(error.to_string()));
                    }
                    Ok(())
                }));
            unsafe {
                self.webview.webview().CallDevToolsProtocolMethod(
                    w!("Input.dispatchKeyEvent"),
                    &down,
                    &callback,
                )
            }
            .context("无法发送浏览器快捷键")?;
            Ok(receiver)
        }

        #[cfg(feature = "ui-test")]
        pub fn capture_png(
            &self,
            path: PathBuf,
        ) -> Result<std::sync::mpsc::Receiver<std::result::Result<(), String>>> {
            use base64::Engine as _;
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use windows::core::w;

            let (sender, receiver) = std::sync::mpsc::channel();
            let callback = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                move |status, response| {
                    // Encoding and disk I/O do not hold the WebView callback.
                    std::thread::spawn(move || {
                        let result = (|| -> Result<()> {
                            status.context("WebView2 截图请求失败")?;
                            let response: serde_json::Value = serde_json::from_str(&response)
                                .context("无法读取 WebView2 截图响应")?;
                            let encoded = response
                                .get("data")
                                .and_then(serde_json::Value::as_str)
                                .context("WebView2 截图响应未包含图片")?;
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .context("WebView2 截图编码无效")?;
                            ensure!(
                                bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                                "WebView2 未返回 PNG 图片"
                            );
                            std::fs::write(&path, bytes).context("无法保存编辑器截图")?;
                            Ok(())
                        })();
                        let _ = sender.send(result.map_err(|error| format!("{error:#}")));
                    });
                    Ok(())
                },
            ));
            // SAFETY: WebView and callback stay on their creating STA thread.
            // WebView2 retains the callback until the asynchronous operation ends.
            unsafe {
                self.webview.webview().CallDevToolsProtocolMethod(
                    w!("Page.captureScreenshot"),
                    w!(r#"{"format":"png","captureBeyondViewport":false,"fromSurface":true}"#),
                    &callback,
                )
            }
            .context("无法请求编辑器原生截图")?;
            Ok(receiver)
        }
    }
}

use std::borrow::Cow;
use wry::http::{Method, Request, Response, StatusCode};

const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; \
    style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; \
    frame-src 'none'; frame-ancestors 'none'; object-src 'none'; \
    media-src 'none'; font-src 'none'; form-action 'none'; base-uri 'none'";

fn embedded_response(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let asset = if request.method() == Method::GET {
        editor_asset(&request.uri().to_string())
    } else {
        None
    };
    let (status, content_type, body): (_, _, &'static [u8]) = match asset {
        Some(EditorAsset::Html) => (
            StatusCode::OK,
            "text/html; charset=utf-8",
            include_bytes!("../editor/dist/index.html"),
        ),
        Some(EditorAsset::Script) => (
            StatusCode::OK,
            "text/javascript; charset=utf-8",
            include_bytes!("../editor/dist/editor.js"),
        ),
        Some(EditorAsset::Stylesheet) => (
            StatusCode::OK,
            "text/css; charset=utf-8",
            include_bytes!("../editor/dist/editor.css"),
        ),
        None => (
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            b"Editor resource not found",
        ),
    };
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Content-Security-Policy", CONTENT_SECURITY_POLICY)
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .header("Cache-Control", "no-store")
        .body(Cow::Borrowed(body))
        .expect("static editor response headers are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_the_three_embedded_assets() {
        assert_eq!(
            editor_asset("studyapp://editor/index.html"),
            Some(EditorAsset::Html)
        );
        assert_eq!(
            editor_asset("studyapp://editor/editor.js"),
            Some(EditorAsset::Script)
        );
        assert_eq!(
            editor_asset("studyapp://editor/editor.css"),
            Some(EditorAsset::Stylesheet)
        );
        for url in [
            "studyapp://editor/",
            "studyapp://editor/index.html?file=secret",
            "studyapp://editor/../Cargo.toml",
            "studyapp://editor/%2e%2e/Cargo.toml",
            "studyapp://editor/editor.js.map",
            "studyapp://editor.evil/editor.js",
            "studyapp://editor:443/editor.js",
            "file:///C:/Users/PC/private.md",
            "https://example.com/editor.js",
        ] {
            assert_eq!(editor_asset(url), None, "{url}");
        }
    }

    #[test]
    fn ipc_accepts_only_the_loaded_editor_document() {
        assert!(editor_page_allowed(EDITOR_PAGE));
        assert!(editor_page_allowed(&format!("{EDITOR_PAGE}#heading")));
        let foreign_page = if cfg!(windows) {
            "studyapp://editor/index.html"
        } else {
            "https://studyapp.editor/index.html"
        };
        assert!(!editor_page_allowed(foreign_page));
        for url in [
            "http://studyapp.editor/index.html",
            "https://studyapp.editor.evil/index.html",
            "https://evil@studyapp.editor/index.html",
            "https://studyapp.editor:443/index.html",
            "https://studyapp.editor/editor.js",
            "https://studyapp.editor/index.html?remote=yes",
            "about:blank",
            "data:text/html,hello",
        ] {
            assert!(!editor_page_allowed(url), "{url}");
        }
    }

    #[test]
    fn oversized_ipc_cannot_enter_the_host_queue() {
        assert!(ipc_allowed(EDITOR_PAGE, MAX_IPC_BYTES));
        assert!(!ipc_allowed(EDITOR_PAGE, MAX_IPC_BYTES + 1));
        assert!(!ipc_allowed("https://example.com/", 1));
    }
}
