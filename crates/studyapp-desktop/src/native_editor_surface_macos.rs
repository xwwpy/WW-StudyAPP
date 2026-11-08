//! An owned AppKit container: drawing masks and hit testing share one region.
use anyhow::{Context as _, Result};
use gpui_kit::{Bounds, Pixels, px};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::NSView;
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CAShapeLayer, CATransaction};
use std::cell::{Cell, RefCell};
use wry::{WebView, WebViewExtMacOS};

#[derive(Default)]
struct SurfaceIvars {
    blocked: Cell<bool>,
    visible: RefCell<Vec<Bounds<Pixels>>>,
}

define_class!(
    // NSView is subclassable; all access stays on the AppKit main thread.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = SurfaceIvars]
    struct EditorSurface;

    impl EditorSurface {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            if self.ivars().blocked.get() { return None; }
            let local = self.convertPoint_fromView(point, self.superview().as_deref());
            let logical = gpui_kit::point(px(local.x as f32), px(local.y as f32));
            if !self.ivars().visible.borrow().iter().any(|rect| rect.contains(&logical)) { return None; }
            // Matching NSView's ABI; AppKit retains the returned descendant.
            unsafe { msg_send![super(self), hitTest: point] }
        }
    }
);

pub struct MacSurface {
    view: Retained<EditorSurface>,
    mask: Retained<CAShapeLayer>,
    applied: RefCell<Option<(Bounds<Pixels>, Vec<Bounds<Pixels>>)>>,
}

impl MacSurface {
    pub fn new(webview: &WebView) -> Result<Self> {
        let mtm = MainThreadMarker::new().context("编辑器需要 AppKit 主线程")?;
        let native = webview.webview();
        let parent = native.superview().context("编辑器缺少父视图")?;
        let frame = native.frame();
        let allocated = EditorSurface::alloc(mtm).set_ivars(SurfaceIvars::default());
        // Initialize our own subclass; never replace a WebKit class or delegate.
        let view: Retained<EditorSurface> =
            unsafe { msg_send![super(allocated), initWithFrame: frame] };
        let mask: Retained<CAShapeLayer> = unsafe { msg_send![CAShapeLayer::class(), new] };
        unsafe {
            view.setWantsLayer(true);
            parent.addSubview(&view);
            native.removeFromSuperview();
            view.addSubview(&native);
        }
        Ok(Self {
            view,
            mask,
            applied: RefCell::new(None),
        })
    }

    pub fn set_bounds(&self, bounds: Bounds<Pixels>, webview: &WebView) -> Result<()> {
        let parent = self.view.superview().context("编辑器缺少父视图")?;
        let y = if parent.isFlipped() {
            f64::from(bounds.top())
        } else {
            parent.bounds().size.height - f64::from(bounds.bottom())
        };
        unsafe {
            self.view.setFrame(NSRect::new(
                NSPoint::new(f64::from(bounds.left()), y),
                NSSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height)),
            ));
        }
        // Wry positions the WKWebView within this top-left-origin container.
        webview.set_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0., 0.).into(),
            size: wry::dpi::LogicalSize::new(
                f64::from(bounds.size.width.max(px(1.))),
                f64::from(bounds.size.height.max(px(1.))),
            )
            .into(),
        })?;
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) {
        self.view.setHidden(!visible);
    }

    pub fn set_blocked(&self, blocked: bool) {
        self.view.ivars().blocked.set(blocked);
    }

    pub fn set_visible_regions(
        &self,
        bounds: Bounds<Pixels>,
        visible: Vec<Bounds<Pixels>>,
    ) -> Result<()> {
        if self.applied.borrow().as_ref() == Some(&(bounds, visible.clone())) {
            return Ok(());
        }
        let local: Vec<_> = visible
            .iter()
            .map(|rect| Bounds::new(rect.origin - bounds.origin, rect.size))
            .collect();
        let layer = self.view.layer().context("编辑器缺少合成图层")?;
        let path = CGMutablePath::new();
        let flipped = layer.isGeometryFlipped();
        for rect in &local {
            let y = if flipped {
                rect.top()
            } else {
                bounds.size.height - rect.bottom()
            };
            unsafe {
                CGMutablePath::add_rect(
                    Some(&path),
                    std::ptr::null(),
                    NSRect::new(
                        NSPoint::new(f64::from(rect.left()), f64::from(y)),
                        NSSize::new(f64::from(rect.size.width), f64::from(rect.size.height)),
                    ),
                );
            }
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.mask.setFrame(layer.bounds());
        self.mask.setPath(Some(&path));
        layer.setMask(Some(&self.mask));
        CATransaction::commit();
        *self.view.ivars().visible.borrow_mut() = local;
        *self.applied.borrow_mut() = Some((bounds, visible));
        Ok(())
    }
}

impl Drop for MacSurface {
    fn drop(&mut self) {
        unsafe {
            self.view.removeFromSuperview();
        }
    }
}
