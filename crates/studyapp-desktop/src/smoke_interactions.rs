//! Native regression scenarios for note interactions, display and nested scrolling.
use crate::{
    app::{Page, StudyApp},
    folders_ui::FolderFilter,
    smoke_library::{click_scrolled, create_note, scroll_settings, settled, until},
    worker::Work,
};
use gpui_kit::{component::WindowExt, test::TestWindowExt, *};
use std::{path::Path, time::Duration};
use studyapp_desktop::presentation::display_path;

fn blocking_notice_button(
    window: &Window,
    center: &crate::notifications::NotificationCenter,
    points: &[Point<Pixels>],
) -> Option<(u64, SharedString)> {
    center
        .test_state()
        .iter()
        .enumerate()
        .find_map(|(index, (id, _))| {
            let card = window.try_find(SharedString::from(format!("notice-{id}")))?;
            points
                .iter()
                .any(|point| card.bounds().contains(point))
                .then(|| {
                    (
                        *id,
                        if index == 0 {
                            "dismiss-notice".into()
                        } else {
                            format!("dismiss-notice-{id}").into()
                        },
                    )
                })
        })
}

/// Scenario setup only: use the actual close action for a prior notification
/// covering a gesture endpoint, then wait for its normal fade/occlusion release.
async fn dismiss_notices_over_gesture(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    source: SharedString,
    target: SharedString,
    cx: &mut AsyncApp,
) {
    let started = std::time::Instant::now();
    loop {
        let clear = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let points = [
                    window.find(source.clone()).bounds().center(),
                    window.find(target.clone()).bounds().center(),
                ];
                let center = view.read(cx).notifications.clone();
                let Some((id, button)) = blocking_notice_button(window, center.read(cx), &points)
                else {
                    return true;
                };
                if !center.read(cx).test_is_closing(id) {
                    window.click(button, cx);
                }
                false
            })
            .unwrap();
        if clear {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "blocking notification did not finish its real dismissal before drag"
        );
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
}

fn screen_point(
    native: &serde_json::Value,
    position: Point<Pixels>,
) -> windows::Win32::Foundation::POINT {
    let scale = native["logicalBounds"][4].as_f64().unwrap() / 96.;
    windows::Win32::Foundation::POINT {
        x: native["parentBounds"]["x"].as_i64().unwrap() as i32
            + (f64::from(f32::from(position.x)) * scale).round() as i32,
        y: native["parentBounds"]["y"].as_i64().unwrap() as i32
            + (f64::from(f32::from(position.y)) * scale).round() as i32,
    }
}

pub(crate) fn assert_overlay_hit(
    native: &serde_json::Value,
    position: Point<Pixels>,
    occluded: bool,
) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{IsChild, WindowFromPoint},
    };
    let target = unsafe { WindowFromPoint(screen_point(native, position)) };
    let parent = HWND(native["parentHwnd"].as_u64().unwrap() as usize as *mut _);
    let child = HWND(native["hwnd"].as_u64().unwrap() as usize as *mut _);
    if occluded {
        assert_eq!(
            target, parent,
            "native child covers painted overlay at {position:?}: {native}"
        );
    } else {
        assert!(
            target == child || unsafe { IsChild(child, target).as_bool() },
            "unpainted overlay area cut a native hole at {position:?}: {native}"
        );
    }
}

fn native_pixel(native: &serde_json::Value, position: Point<Pixels>) -> u32 {
    use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    let point = screen_point(native, position);
    unsafe {
        let dc = GetDC(None);
        let color = GetPixel(dc, point.x, point.y).0;
        ReleaseDC(None, dc);
        color
    }
}

fn assert_native_continuity(before: &serde_json::Value, after: &serde_json::Value) {
    for key in [
        "hwnd",
        "parentHwnd",
        "focusHwnd",
        "foregroundHwnd",
        "visible",
        "controllerVisible",
        "visibilityUpdates",
    ] {
        assert_eq!(after[key], before[key], "bubble changed native {key}");
    }
}

// The native raster outline can conservatively extend less than one physical
// pixel per axis beyond the painted logical triangle. Restoration samples must
// be outside that actual outline, not merely outside the original triangle.
pub(crate) fn raster_arrow_contains(native: &serde_json::Value, position: Point<Pixels>) -> bool {
    let screen = screen_point(native, position);
    let sample = (
        f64::from(screen.x) - native["bounds"]["x"].as_f64().unwrap(),
        f64::from(screen.y) - native["bounds"]["y"].as_f64().unwrap(),
    );
    native["occlusionArrowPolygons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|polygon| {
            let points = polygon.as_array().unwrap();
            let sides = (0..points.len())
                .map(|index| {
                    let a = &points[index];
                    let b = &points[(index + 1) % points.len()];
                    (b[0].as_f64().unwrap() - a[0].as_f64().unwrap())
                        * (sample.1 - a[1].as_f64().unwrap())
                        - (b[1].as_f64().unwrap() - a[1].as_f64().unwrap())
                            * (sample.0 - a[0].as_f64().unwrap())
                })
                .collect::<Vec<_>>();
            sides.iter().all(|side| *side >= 0.) || sides.iter().all(|side| *side <= 0.)
        })
}

struct NoteFolderDragProbe {
    source: SharedString,
    target: SharedString,
    note_id: String,
    destination: String,
    path: std::path::PathBuf,
    evidence: std::rc::Rc<std::cell::RefCell<serde_json::Value>>,
    started: std::time::Instant,
}

fn rect_json(bounds: Bounds<Pixels>) -> serde_json::Value {
    serde_json::json!([
        f32::from(bounds.left()),
        f32::from(bounds.top()),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height)
    ])
}

impl NoteFolderDragProbe {
    fn save(&self) {
        std::fs::write(
            &self.path,
            serde_json::to_string_pretty(&*self.evidence.borrow()).unwrap(),
        )
        .unwrap();
    }
    fn outcome(&self, app: &StudyApp, cx: &App) -> bool {
        let folder = app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.notes.iter().find(|note| note.id == self.note_id))
            .and_then(|note| note.folder_id.as_ref());
        let success = folder == Some(&self.destination);
        self.evidence.borrow_mut()["outcome"] = serde_json::json!({"elapsedMs":self.started.elapsed().as_millis(),"folder":folder,"notice":format!("{:?}",app.notice),"success":success,"activeDrag":cx.has_active_drag()});
        self.save();
        success
    }
    fn frame(&self, phase: &str, window: &Window, app: &StudyApp, cx: &App) {
        let mut clip = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let mut clips = Vec::new();
        for id in ["notes-pane", "note-list-scroll"] {
            if let Some(element) = window.try_find(id) {
                clip = clip.intersect(&element.bounds());
                clips.push(serde_json::json!({"id":id,"bounds":rect_json(element.bounds())}));
            }
        }
        let element = |id: SharedString| {
            window.try_find(id).map(|element| {
            let bounds = element.bounds();
            let center = bounds.center();
            serde_json::json!({"path":format!("{:?}",element.path()),"bounds":rect_json(bounds),"visible":element.visible(),"center":[f32::from(center.x),f32::from(center.y)],"clipIntersection":rect_json(bounds.intersect(&clip)),"centerInsideObservedClips":clip.contains(&center)})
        })
        };
        let snapshot = app.snapshot.as_ref().unwrap();
        let note = snapshot.notes.iter().find(|note| note.id == self.note_id);
        let folder = snapshot
            .folders
            .iter()
            .find(|folder| folder.id == self.destination);
        let record = serde_json::json!({
            "phase":phase,"elapsedMs":self.started.elapsed().as_millis(),
            "sourceId":self.source.as_str(),"targetId":self.target.as_str(),
            "source":element(self.source.clone()),"target":element(self.target.clone()),
            "sourceSnapshotPath":note.map(|note| &note.path),"destinationSnapshot":folder,
            "viewport":[f32::from(window.viewport_size().width),f32::from(window.viewport_size().height)],
            "observedClips":clips,"query":app.search.read(cx).value().as_str(),
            "selectedNote":app.selected_note,"noteFolder":note.and_then(|note| note.folder_id.as_ref()),
            "rootLibrary":app.last_library,"snapshotLibrary":snapshot.data_path,"layout":app.layout,
            "noteMenu":app.active_note_menu,"folderOverlay":app.folders.read(cx).overlay_open(),
            "folderFilter":format!("{:?}",app.folders.read(cx).filter()),"modal":app.has_modal(cx),
            "notices":app.notifications.read(cx).occlusions().iter().copied().map(rect_json).collect::<Vec<_>>(),
            "activeDrag":cx.has_active_drag(),"pointer":[f32::from(window.mouse_position().x),f32::from(window.mouse_position().y)],
            "notePreview":window.try_find("note-drag-preview").map(|preview| rect_json(preview.bounds())),
            "nativeEditor":app.note_content.read(cx).native_state().ok(),"notice":format!("{:?}",app.notice)
        });
        self.evidence.borrow_mut()["frames"]
            .as_array_mut()
            .unwrap()
            .push(record);
        self.save();
    }
}

/// Exact TestWindowExt::drag_to sequence, with read-only per-phase telemetry.
/// Coordinates are resolved once as in the original helper; no reveal/retry,
/// guard override or outcome/timeout change is introduced by this probe.
fn drag_note_with_probe(
    window: &mut Window,
    view: &Entity<StudyApp>,
    probe: &NoteFolderDragProbe,
    cx: &mut App,
) {
    window.render_frame(cx);
    let source = window.find(probe.source.clone());
    let target = window.find(probe.target.clone());
    assert!(source.visible(), "drag source is not visible");
    assert!(target.visible(), "drag target is not visible");
    let from = source.bounds().center();
    let to = target.bounds().center();
    probe.frame("resolved-endpoints", window, view.read(cx), cx);
    assert!(
        blocking_notice_button(window, view.read(cx).notifications.read(cx), &[from, to]).is_none(),
        "notification still covers a resolved drag endpoint"
    );
    window.render_frame(cx);
    window.dispatch_event(
        MouseMoveEvent {
            position: from,
            pressed_button: None,
            modifiers: Default::default(),
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
    probe.frame("before-down", window, view.read(cx), cx);
    window
        .render_to_image()
        .unwrap()
        .save(
            probe
                .path
                .with_file_name("native-note-folder-drag-before.png"),
        )
        .unwrap();
    window.dispatch_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
    probe.frame("after-down", window, view.read(cx), cx);
    for step in 1..=8 {
        let fraction = step as f32 / 8.;
        let position = point(
            from.x + (to.x - from.x) * fraction,
            from.y + (to.y - from.y) * fraction,
        );
        window.dispatch_event(
            MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                modifiers: Default::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        probe.frame(&format!("move-{step}"), window, view.read(cx), cx);
    }
    window
        .render_to_image()
        .unwrap()
        .save(
            probe
                .path
                .with_file_name("native-note-folder-drag-final-hover.png"),
        )
        .unwrap();
    window.dispatch_event(
        MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            modifiers: Default::default(),
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
    probe.frame("after-up", window, view.read(cx), cx);
}

/// Deterministic geometry fixture, distinct from the real sidebar source hover.
/// The OS pointer is moved, then GPUI receives an explicit move because a native
/// child owns mouse messages inside its client area. This proves shared paint,
/// registry and HWND clipping; it does not claim native hover forwarding.
pub(crate) async fn exercise_pointer_bubble(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    editor: &Entity<crate::markdown_editor::MarkdownEditor>,
    database: &Path,
    label: &str,
    cx: &mut AsyncApp,
) {
    use studyapp_ui::bubble::OverlayShape;
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| app.clear_notices(cx));
            window.render_frame(cx);
        })
        .unwrap();
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    let before_browser =
        crate::smoke_editor::bridge(window, editor, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    let before = crate::smoke_visibility::visible(window, editor, cx).await;
    let (bounds, original_size) = window
        .update(cx, |_, window, _| {
            let logical = before["logicalBounds"].as_array().unwrap();
            (
                Bounds::new(
                    point(
                        px(logical[0].as_f64().unwrap() as f32),
                        px(logical[1].as_f64().unwrap() as f32),
                    ),
                    size(
                        px(logical[2].as_f64().unwrap() as f32),
                        px(logical[3].as_f64().unwrap() as f32),
                    ),
                ),
                window.viewport_size(),
            )
        })
        .unwrap();
    let positions = [
        point(bounds.center().x, bounds.top() + bounds.size.height * 0.25),
        point(bounds.center().x, bounds.bottom() - px(12.)),
        point(
            bounds.right() - px(8.),
            bounds.top() + bounds.size.height * 0.25,
        ),
        point(
            bounds.right() - px(18.),
            bounds.top() + bounds.size.height * 0.25,
        ),
    ];
    let mut records = Vec::new();
    let mut bodies = Vec::new();
    let mut arrows = Vec::new();
    let mut restored_samples = Vec::new();
    for (index, position) in positions.into_iter().enumerate() {
        let physical = screen_point(&before, position);
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetCursorPos(physical.x, physical.y).unwrap();
        }
        window
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position,
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
                if index == 0 {
                    let bubble = studyapp_ui::components::pointer_tooltip(
                        "指针居中气泡：移动、翻转和边缘收敛".into(),
                        window,
                        cx,
                    );
                    view.update(cx, |app, cx| {
                        app.smoke_pointer_bubble = Some(bubble);
                        cx.notify();
                    });
                }
                window.render_frame(cx);
            })
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_millis(80))
            .await;
        let (body, arrow, native) = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let shapes = view.read(cx).bubble_registry.read(cx).shapes();
                let OverlayShape::RoundedRect { bounds: body, .. } = shapes[0] else {
                    panic!("missing body")
                };
                let OverlayShape::Triangle(arrow) = shapes[1] else {
                    panic!("missing arrow")
                };
                assert_eq!(window.find("sidebar-tooltip-content").bounds(), body);
                if index < 2 {
                    assert_eq!(
                        body.left(),
                        window.pixel_snap(position.x - body.size.width * 0.5),
                        "body must use the exact GPUI device-grid centering rule"
                    );
                }
                if index == 1 {
                    assert!(
                        body.bottom() < position.y,
                        "near-bottom bubble did not flip"
                    );
                }
                let native = editor.read(cx).native_state().unwrap();
                assert_native_continuity(&before, &native);
                let body_sample = point(body.center().x, body.bottom() - px(5.));
                let arrow_sample = (arrow[0] + arrow[1] + arrow[2]) / 3.;
                let beside = point(
                    arrow[0].x + px(1.),
                    arrow[1].y + (arrow[0].y - arrow[1].y) * 0.15,
                );
                assert!(
                    bounds.contains(&body_sample)
                        && bounds.contains(&arrow_sample)
                        && bounds.contains(&beside)
                );
                assert_overlay_hit(&native, body_sample, true);
                assert_overlay_hit(&native, arrow_sample, true);
                assert!(
                    !raster_arrow_contains(&native, beside),
                    "adjacent sample entered the bounded physical arrow outline"
                );
                assert_overlay_hit(&native, beside, false);
                for sample in &restored_samples {
                    if !body.contains(sample) && !raster_arrow_contains(&native, *sample) {
                        assert_overlay_hit(&native, *sample, false);
                    }
                }
                restored_samples.extend([body_sample, arrow_sample]);
                (body, arrow, native)
            })
            .unwrap();
        let color = window
            .update(cx, |_, _, _| studyapp_ui::theme::palette().surface_raised)
            .unwrap();
        let expected = ((color >> 16) & 255) | (color & 0xFF00) | ((color & 255) << 16);
        let body_sample = point(body.center().x, body.bottom() - px(5.));
        let arrow_sample = (arrow[0] + arrow[1] + arrow[2]) / 3.;
        let beside = point(
            arrow[0].x + px(1.),
            arrow[1].y + (arrow[0].y - arrow[1].y) * 0.15,
        );
        let pixel_started = std::time::Instant::now();
        let mut sampled = (0, 0);
        let mut samples_taken = 0;
        for attempt in 0..60 {
            sampled = (
                native_pixel(&native, body_sample),
                native_pixel(&native, arrow_sample),
            );
            samples_taken = attempt + 1;
            if sampled == (expected, expected) {
                break;
            }
            if attempt < 59 {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
            }
        }
        let pixel_match = sampled == (expected, expected);
        let sample_record = |position: Point<Pixels>, color: u32| {
            let screen = screen_point(&native, position);
            serde_json::json!({"logical":[f32::from(position.x),f32::from(position.y)],"screen":[screen.x,screen.y],"colorRef":color,"colorRefHex":format!("#{color:06X}")})
        };
        // Persist the current indexed position before the pixel assertion.
        // On failure capture actual desktop pixels (including the HWND), not
        // GPUI-only output. At most four small JSONs and one failure BMP/host.
        let indexed = format!("native-{label}-pointer-bubble-position-{index}");
        std::fs::write(database.with_file_name(format!("{indexed}.json")), serde_json::to_string_pretty(&serde_json::json!({
            "driving":"controlled fixture with OS pointer and explicit GPUI move; actual sidebar hover is separate",
            "index":index,"pointer":[f32::from(position.x),f32::from(position.y)],
            "body":[f32::from(body.left()),f32::from(body.top()),f32::from(body.size.width),f32::from(body.size.height)],
            "arrow":arrow.map(|point| [f32::from(point.x),f32::from(point.y)]),
            "nativeBefore":before,"native":native,
            "sampleFormat":"Windows COLORREF 0x00BBGGRR","expectedColorRef":expected,"expectedColorRefHex":format!("#{expected:06X}"),
            "bodySample":sample_record(body_sample,sampled.0),"arrowSample":sample_record(arrow_sample,sampled.1),
            "besideSample":sample_record(beside,native_pixel(&native,beside)),
            "samplesTaken":samples_taken,"sampleElapsedMs":pixel_started.elapsed().as_millis(),"pixelMatch":pixel_match
        })).unwrap()).unwrap();
        if !pixel_match {
            crate::smoke_visibility::capture_window(
                window,
                editor,
                &database.with_file_name(format!("{indexed}.bmp")),
                cx,
            )
            .await;
        }
        assert!(
            pixel_match,
            "actual screen pixels did not show bubble fill: body={}, arrow={}, expected={expected}; indexed evidence={indexed}",
            sampled.0, sampled.1
        );
        bodies.push(body);
        arrows.push(arrow);
        records.push(serde_json::json!({"position":[f32::from(position.x),f32::from(position.y)],"body":[f32::from(body.left()),f32::from(body.top()),f32::from(body.size.width),f32::from(body.size.height)],"arrow":arrow.map(|p| [f32::from(p.x),f32::from(p.y)]),"rasterArrowPolygons":native["occlusionArrowPolygons"],"native":native}));
    }
    assert_eq!(
        bodies[2], bodies[3],
        "fixture did not exercise a clamped stationary body"
    );
    assert_ne!(arrows[2], arrows[3], "clamped arrow did not follow pointer");
    crate::smoke_visibility::capture_window(
        window,
        editor,
        &database.with_file_name(format!("native-{label}-pointer-bubble.bmp")),
        cx,
    )
    .await;
    let alternate = if original_size.width > px(1000.) {
        size(px(1000.), px(700.))
    } else {
        size(px(1024.), px(720.))
    };
    window
        .update(cx, |_, window, _| {
            crate::smoke_library::resize(window, alternate)
        })
        .unwrap();
    cx.background_executor()
        .timer(Duration::from_millis(150))
        .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let body = window.find("sidebar-tooltip-content").bounds();
            assert!(
                body.right() <= window.viewport_size().width - px(8.)
                    && body.bottom() <= window.viewport_size().height - px(8.)
            );
            assert_native_continuity(&before, &editor.read(cx).native_state().unwrap());
            crate::smoke_library::resize(window, original_size);
            view.update(cx, |app, cx| {
                app.smoke_pointer_bubble = None;
                cx.notify();
            });
        })
        .unwrap();
    for _ in 0..5 {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        window
            .update(cx, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    let after = window
        .update(cx, |_, _, cx| {
            assert!(view.read(cx).bubble_registry.read(cx).shapes().is_empty());
            let native = editor.read(cx).native_state().unwrap();
            assert_native_continuity(&before, &native);
            assert_eq!(
                native["occlusionArrowPolygons"],
                before["occlusionArrowPolygons"]
            );
            assert_eq!(native["occlusions"], before["occlusions"]);
            for sample in restored_samples {
                assert_overlay_hit(&native, sample, false);
            }
            native
        })
        .unwrap();
    let after_browser =
        crate::smoke_editor::bridge(window, editor, serde_json::json!({"type":"snapshot"}), cx)
            .await;
    for key in ["text", "generation", "selection"] {
        assert_eq!(
            before_browser[key], after_browser[key],
            "bubble changed browser {key}"
        );
    }
    std::fs::write(database.with_file_name(format!("native-{label}-pointer-bubble.json")), serde_json::to_string_pretty(&serde_json::json!({"driving":"controlled fixture with OS pointer and explicit GPUI move; real sidebar hover is separate","before":before,"steps":records,"after":after,"browserBefore":before_browser,"browserAfter":after_browser})).unwrap()).unwrap();
}

pub async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    database: &Path,
    cx: &mut AsyncApp,
) {
    settled(window, view, cx).await;
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::resize(window, size(px(1280.), px(900.)));
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.page = Page::Notes;
                app.history = false;
                app.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                app.folders
                    .update(cx, |panel, cx| panel.set_filter(FolderFilter::All, cx));
                cx.notify();
            });
        })
        .unwrap();
    cx.background_executor()
        .timer(Duration::from_millis(150))
        .await;
    let editor = window
        .update(cx, |_, _, cx| view.read(cx).note_content.clone())
        .unwrap();
    crate::smoke_visibility::capture(
        window,
        &editor,
        &database.with_file_name("native-after-history.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            crate::smoke_library::library_add(window, true, view, cx);
            window.click("folder-name-input", cx);
            window.input("拖动目标目录", cx);
            window.press("enter", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "Enter creates exactly one folder",
        |app, _| {
            app.snapshot
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .filter(|f| f.name == "拖动目标目录")
                .count()
                == 1
        },
        cx,
    )
    .await;
    let destination = window
        .update(cx, |_, _, cx| {
            view.read(cx)
                .snapshot
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .find(|f| f.name == "拖动目标目录")
                .unwrap()
                .id
                .clone()
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.click("folders-all", cx);
            crate::smoke_library::reveal_scrolled(
                window,
                format!("folder-row-{destination}").into(),
                "folder-tree-scroll",
                cx,
            );
            window.right_click(SharedString::from(format!("folder-row-{destination}")), cx);
        })
        .unwrap();
    until(
        window,
        view,
        "folder row context menu opened",
        |app, cx| app.folders.read(cx).overlay_open() && !app.folders.read(cx).modal_open(),
        cx,
    )
    .await;
    let rename_input = window
        .update(cx, |_, window, cx| {
            assert_eq!(
                view.read(cx).folders.read(cx).filter(),
                &FolderFilter::All,
                "right-click must keep the current note filter"
            );
            assert_eq!(
                editor.read(cx).native_state().unwrap()["controllerVisible"],
                true,
                "folder context menu must keep the document visible, clipping only its own region"
            );
            for old_button in ["folder-rename", "folder-move", "folder-delete"] {
                assert!(
                    window.try_find(SharedString::from(old_button)).is_none(),
                    "folder row actions must live in its context menu"
                );
            }
            window.press("down", cx);
            window.press("down", cx);
            window.press("enter", cx); // Rename the clicked row, while All remains selected.
            window.render_frame(cx);
            assert!(
                view.read(cx).folders.read(cx).modal_open(),
                "menu dismissal must not close the newly opened action dialog"
            );
            window.click("folder-name-input", cx);
            window.press("ctrl-a", cx);
            window
                .focused_input(cx)
                .expect("rename input must retain focus")
                .as_input()
                .expect("folder name uses a single-line input")
                .clone()
        })
        .unwrap();
    // Configured native shortcuts defer dispatch to avoid reentering StudyApp.
    // Real key messages leave the event loop between Ctrl+A and text input.
    cx.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    until(
        window,
        view,
        "deferred Ctrl+A selected the rename input",
        |_, cx| {
            let input = rename_input.read(cx);
            input.selected_range() == (0..input.text().len())
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            assert_eq!(
                rename_input.read(cx).value().as_ref(),
                "拖动目标目录",
                "rename must start from the clicked folder's original name"
            );
            window.input("右键重命名拖动目录", cx);
            window.press("enter", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "context rename targets clicked folder",
        |app, cx| {
            !app.folders.read(cx).overlay_open()
                && app
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .folders
                    .iter()
                    .any(|folder| folder.id == destination && folder.name == "右键重命名拖动目录")
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.click(SharedString::from(format!("folder-row-{destination}")), cx);
        })
        .unwrap();
    until(
        window,
        view,
        "empty folder selection delivered",
        |app, cx| {
            app.folders.read(cx).filter() == &FolderFilter::Folder(destination.clone())
                && app.selected_note.is_none()
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(view.read(cx).selected_note.is_none());
            let timer_snapshot = view.read(cx).snapshot.as_ref().unwrap().clone();
            view.update(cx, |app, cx| {
                app.accept_snapshot(timer_snapshot, false, window, cx)
            });
            assert!(
                view.read(cx).selected_note.is_none(),
                "timer refresh must keep an empty folder's document empty"
            );
            for old_button in ["folder-rename", "folder-move", "folder-delete"] {
                assert!(
                    window.try_find(SharedString::from(old_button)).is_none(),
                    "selected folder retained its obsolete bottom action buttons"
                );
            }
            window.click("folders-all", cx);
            // The compact row menu gives keyboard users the same four actions.
            crate::smoke_library::reveal_scrolled(
                window,
                format!("folder-menu-{destination}").into(),
                "folder-tree-scroll",
                cx,
            );
            window.click(SharedString::from(format!("folder-menu-{destination}")), cx);
        })
        .unwrap();
    until(
        window,
        view,
        "folder row keyboard menu opened",
        |app, cx| app.folders.read(cx).overlay_open() && !app.folders.read(cx).modal_open(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.press("down", cx);
            window.press("enter", cx); // New child is the first menu item.
            window.render_frame(cx);
            assert!(view.read(cx).folders.read(cx).modal_open());
            window.input("键盘菜单子目录", cx);
            window.press("enter", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "keyboard row menu creates child",
        |app, cx| {
            !app.folders.read(cx).overlay_open()
                && app.snapshot.as_ref().unwrap().folders.iter().any(|folder| {
                    folder.name == "键盘菜单子目录"
                        && folder.parent_id.as_ref() == Some(&destination)
                })
        },
        cx,
    )
    .await;
    let note = create_note(window, view, "拖动与右键测试", None, cx).await;
    let bytes = std::fs::read(&note.path).unwrap();
    let note_element: SharedString = format!("note-{}", note.id).into();
    let drag_probe = NoteFolderDragProbe {
        source: note_element.clone(),
        target: format!("folder-drop-{destination}").into(),
        note_id: note.id.clone(),
        destination: destination.clone(),
        path: database.with_file_name("native-note-folder-drag.json"),
        evidence: std::rc::Rc::new(std::cell::RefCell::new(serde_json::json!({
            "driving":"GPUI synthetic drag inside the native application; same two resolved centers, eight pressed moves, render after each event", "frames":[], "events":[]
        }))),
        started: std::time::Instant::now(),
    };
    let folder_move_probe = window.update(cx, |_, _, cx| {
        let folders = view.read(cx).folders.clone();
        let evidence = drag_probe.evidence.clone();
        let path = drag_probe.path.clone();
        let expected_note = note.id.clone();
        let expected_destination = destination.clone();
        let started = drag_probe.started;
        cx.subscribe(&folders, move |_, event: &crate::folders_ui::FolderEvent, cx| {
            if let crate::folders_ui::FolderEvent::Command(command) = event
                && let studyapp_core::Command::MoveNote { note_id, folder_id } = command.as_ref()
                && note_id == &expected_note && folder_id.as_ref() == Some(&expected_destination)
            {
                evidence.borrow_mut()["events"].as_array_mut().unwrap().push(serde_json::json!({"kind":"FolderEvent::Command::MoveNote","elapsedMs":started.elapsed().as_millis(),"noteId":note_id,"folderId":folder_id,"activeDrag":cx.has_active_drag()}));
                std::fs::write(&path, serde_json::to_string_pretty(&*evidence.borrow()).unwrap()).unwrap();
            }
        })
    }).unwrap();
    window
        .update(cx, |_, window, cx| {
            drag_probe.frame("before-selection", window, view.read(cx), cx);
            view.update(cx, |app, cx| {
                app.search.update(cx, |input, cx| {
                    input.set_value("拖动与右键测试", window, cx)
                });
                app.selected_note = Some(note.id.clone());
                app.selected_version = None;
                cx.notify();
            });
            window.render_frame(cx);
            drag_probe.frame("after-selection-layout", window, view.read(cx), cx);
        })
        .unwrap();
    // The previous long-URI scenario can still publish its missing-image
    // warning after a later scenario starts. Preserve notification hit testing
    // and dismiss only a card that actually covers this gesture's endpoints.
    dismiss_notices_over_gesture(
        window,
        view,
        note_element.clone(),
        drag_probe.target.clone(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.click(note_element.clone(), cx);
            assert!(
                window.try_find("note-file-path").is_none(),
                "paths must be hidden by default"
            );
            assert!(
                window
                    .try_find(SharedString::from(format!("note-row-path-{}", note.id)))
                    .is_none()
            );
            drag_probe.frame("after-selection-click", window, view.read(cx), cx);
            drag_probe.frame("before-drag-helper", window, view.read(cx), cx);
            drag_note_with_probe(window, view, &drag_probe, cx);
        })
        .unwrap();
    until(
        window,
        view,
        "native drag moves into target",
        |app, cx| {
            drag_probe.outcome(app, cx);
            app.snapshot
                .as_ref()
                .unwrap()
                .notes
                .iter()
                .any(|n| n.id == note.id && n.folder_id.as_ref() == Some(&destination))
        },
        cx,
    )
    .await;
    drop(folder_move_probe);
    assert_eq!(std::fs::read(&note.path).unwrap(), bytes);
    window
        .update(cx, |_, window, cx| {
            assert_eq!(
                view.read(cx).selected_note.as_ref(),
                Some(&note.id),
                "moving the selected note must preserve its document"
            );
            assert_eq!(
                view.read(cx).folders.read(cx).filter(),
                &FolderFilter::Folder(destination.clone()),
                "new-note destination follows the moved selected note"
            );
            crate::smoke_library::reveal_scrolled(
                window,
                note_element.clone(),
                "note-list-scroll",
                cx,
            );
        })
        .unwrap();
    crate::smoke_visibility::hover(window, &editor, note_element.clone(), cx).await;
    let hover_native = window
        .update(cx, |_, window, cx| {
            let native = editor.read(cx).native_state().unwrap();
            let row = window.find(note_element.clone()).bounds();
            let position = point(row.right() - px(12.), row.center().y);
            let physical = screen_point(&native, position);
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetCursorPos(physical.x, physical.y)
                    .unwrap();
            }
            native
        })
        .unwrap();
    for attempt in 0..100 {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        let shown = window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("sidebar-tooltip-content").is_some()
            })
            .unwrap();
        if shown {
            break;
        }
        assert!(
            attempt < 99,
            "native pointer hover did not reveal schedule tooltip"
        );
    }
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let tip = window.find("sidebar-tooltip-content").bounds();
            let document = window.find("continuous-markdown-editor").bounds();
            assert_eq!(
                tip.left(),
                window.pixel_snap(window.mouse_position().x - tip.size.width * 0.5),
                "real sidebar tooltip is not centered on the OS pointer"
            );
            let overlap = tip.intersect(&document);
            assert!(
                overlap.size.width > px(4.) && overlap.size.height > px(4.),
                "sidebar fixture did not overlap native editor"
            );
            let native = editor.read(cx).native_state().unwrap();
            assert_overlay_hit(&native, overlap.center(), true);
            assert_native_continuity(&hover_native, &native);
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("sidebar-hover.png"))
                .unwrap();
        })
        .unwrap();
    crate::smoke_visibility::capture_window(
        window,
        &editor,
        &database.with_file_name("sidebar-hover-native.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            let row = window.find(note_element.clone()).bounds();
            let position = point(row.right() - px(24.), row.center().y + px(2.));
            let physical = screen_point(&hover_native, position);
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetCursorPos(physical.x, physical.y)
                    .unwrap();
            }
            window.render_frame(cx);
        })
        .unwrap();
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("sidebar-tooltip-content").bounds().left(),
                window.pixel_snap(
                    window.mouse_position().x
                        - window.find("sidebar-tooltip-content").bounds().size.width * 0.5
                ),
                "already-open tooltip did not follow actual OS pointer"
            );
            let position = window.find("continuous-markdown-editor").bounds().center();
            let physical = screen_point(&hover_native, position);
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetCursorPos(physical.x, physical.y)
                    .unwrap();
            }
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
    for _ in 0..4 {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        window
            .update(cx, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    window
        .update(cx, |_, window, _| {
            assert!(
                window.try_find("sidebar-tooltip-content").is_none(),
                "sidebar tooltip did not dismiss off source"
            )
        })
        .unwrap();
    exercise_pointer_bubble(window, view, &editor, database, "note", cx).await;
    window
        .update(cx, |_, window, cx| {
            window.right_click(note_element.clone(), cx);
        })
        .unwrap();
    until(
        window,
        view,
        "deferred note context menu opened",
        |app, _| app.active_note_menu.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            assert_eq!(
                view.read(cx).note_content.read(cx).native_state().unwrap()["controllerVisible"],
                true,
                "opening a note menu must keep the document visible"
            );
            window.press("down", cx);
            window.press("right", cx);
            window.press("enter", cx); // Unclassified is first for a classified note.
        })
        .unwrap();
    until(
        window,
        view,
        "context menu moves to Unclassified and closes",
        |app, _| {
            app.active_note_menu.is_none()
                && app
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .notes
                    .iter()
                    .any(|n| n.id == note.id && n.folder_id.is_none())
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, _, cx| {
            assert_eq!(
                view.read(cx).selected_note.as_ref(),
                Some(&note.id),
                "context move must preserve the selected document"
            );
            assert_eq!(view.read(cx).folders.read(cx).filter(), &FolderFilter::Root);
        })
        .unwrap();
    let editor = window
        .update(cx, |_, _, cx| view.read(cx).note_content.clone())
        .unwrap();
    crate::smoke_visibility::capture(
        window,
        &editor,
        &database.with_file_name("native-after-menu.bmp"),
        cx,
    )
    .await;
    let previous_clipboard = window
        .update(cx, |_, _, cx| cx.read_from_clipboard())
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.right_click(note_element.clone(), cx)
        })
        .unwrap();
    until(
        window,
        view,
        "deferred copy-path menu opened",
        |app, _| app.active_note_menu.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.press("down", cx);
            window.press("down", cx);
            window.press("down", cx);
            window.press("enter", cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(display_path(&note.path))
            );
            cx.write_to_clipboard(
                previous_clipboard.unwrap_or_else(|| ClipboardItem::new_string(String::new())),
            );
            window.click(("nav", 4_usize), cx);
            window.click("settings-section-appearance", cx);
            window.click("appearance-workspace", cx);
            click_scrolled(window, "show-file-paths".into(), "page-scroll", cx);
            assert!(
                view.read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .show_file_paths
            );
            assert!(
                !view
                    .read(cx)
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .settings
                    .show_file_paths,
                "checkbox saved before explicit save"
            );
            assert!(view.read(cx).settings_dirty(cx));
            window.click(("nav", 1_usize), cx);
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).page,
                Page::Settings,
                "dirty settings left without confirmation"
            );
            assert!(view.read(cx).preferences_ui.leave.is_some());
            window.click("settings-continue-editing", cx);
            assert_eq!(view.read(cx).page, Page::Settings);
            assert!(view.read(cx).preferences_ui.leave.is_none());
            assert!(
                view.read(cx).settings_dirty(cx),
                "continue editing discarded the draft"
            );
            let library = view.read(cx).last_library.clone();
            view.update(cx, |app, cx| {
                app.storage_busy = true;
                app.accept_picker_result(
                    library.clone(),
                    Work::SwitchLibrary(database.with_file_name("unused-picker-result")),
                    cx,
                );
                assert!(app.preferences_ui.leave.is_none());
                assert!(!app.preferences_ui.replacing);
                let (queued_library, work) = app
                    .picker_results
                    .try_recv()
                    .expect("picker result must wait while migration owns storage locks");
                assert_eq!(queued_library, library);
                app.storage_busy = false;
                app.accept_picker_result(queued_library, work, cx);
            });
            assert!(
                matches!(
                    view.read(cx).preferences_ui.leave,
                    Some(crate::settings_ui::SettingsContinuation::PickedWork(..))
                ),
                "async picker result bypassed dirty settings guard"
            );
            assert_eq!(view.read(cx).last_library, library);
            assert!(
                !view.read(cx).storage_busy,
                "guarded picker result started switching libraries"
            );
            window.render_frame(cx);
            window.click("settings-continue-editing", cx);
            view.update(cx, |app, cx| {
                app.close_requested = true;
                app.finish_draft_persist(crate::editor_lifecycle::NoteIntent::Close, window, cx);
            });
            assert!(
                matches!(
                    view.read(cx).preferences_ui.leave,
                    Some(crate::settings_ui::SettingsContinuation::Close)
                ),
                "async note close bypassed dirty settings guard"
            );
            assert!(!view.read(cx).close_allowed && !view.read(cx).close_requested);
            assert_eq!(view.read(cx).page, Page::Settings);
            window.render_frame(cx);
            window.click("settings-continue-editing", cx);
            assert!(view.read(cx).preferences_ui.leave.is_none());
            assert!(view.read(cx).settings_dirty(cx));
            window.click(("nav", 1_usize), cx);
            window.render_frame(cx);
            window.click("settings-discard", cx);
            assert_eq!(view.read(cx).page, Page::Notes);
            assert!(
                !view
                    .read(cx)
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .settings
                    .show_file_paths
            );
            assert!(
                !view.read(cx).settings_dirty(cx),
                "discard retained dirty settings"
            );
            window.click(("nav", 4_usize), cx);
            window.click("settings-section-appearance", cx);
            window.click("appearance-workspace", cx);
            click_scrolled(window, "show-file-paths".into(), "page-scroll", cx);
            window.click(("nav", 1_usize), cx);
            window.render_frame(cx);
            assert!(view.read(cx).preferences_ui.leave.is_some());
            window.click("settings-save-and-continue", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "path setting saved before navigation continues",
        |app, cx| {
            app.page == Page::Notes
                && !app.preferences_ui.busy
                && app.preferences_ui.leave.is_none()
                && !app.settings_dirty(cx)
                && app.snapshot.as_ref().unwrap().settings.show_file_paths
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.click(("nav", 1_usize), cx);
            window.render_frame(cx);
            assert!(window.try_find("note-file-path").is_some());
            assert!(
                window
                    .try_find(SharedString::from(format!("note-row-path-{}", note.id)))
                    .is_none(),
                "compact library rows keep paths in their hover tooltip"
            );
            assert_eq!(
                window.find(note_element.clone()).bounds().size.height,
                px(32.)
            );
            window.click("edit-note", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "editor path enabled",
        |app, _| app.note_editor.is_some(),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("editor-file-path").is_some());
            crate::smoke_library::editor_action(window, view, true, cx);
            window.click("confirm-note-action", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "discard unchanged path test draft",
        |app, _| app.note_editor.is_none() && app.draft_discard.is_none(),
        cx,
    )
    .await;
    for i in 0..12 {
        create_note(window, view, &format!("空间滚动测试 {i:02}"), None, cx).await;
    }
    window
        .update(cx, |_, window, cx| {
            window.click(("nav", 4_usize), cx);
            window.click("settings-section-storage", cx);
            window.click("storage-copies", cx);
            view.update(cx, |app, cx| {
                app.clear_notices(cx);
                app.storage_work(Work::RefreshStorage, cx);
            });
        })
        .unwrap();
    until(
        window,
        view,
        "storage report with multiple copies",
        |app, _| {
            !app.storage_busy
                && app
                    .storage_report
                    .as_ref()
                    .is_some_and(|r| r.copies.len() >= 12)
        },
        cx,
    )
    .await;
    // The dedicated copies page is intentionally shorter than the previous
    // all-in-one Storage page. Use a real compact-height window to exercise
    // outer overflow while keeping the complete 340px inner list reachable.
    // Respect the production window's 700px minimum; Windows clamps smaller
    // requests, so waiting for a 640px client area can never succeed.
    let storage_size = window
        .update(cx, |_, window, _| {
            let original = window.viewport_size();
            crate::smoke_library::resize(window, size(px(1280.), px(700.)));
            original
        })
        .unwrap();
    for attempt in 0..100 {
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
        if window
            .update(cx, |_, window, _| window.viewport_size().height <= px(720.))
            .unwrap()
        {
            break;
        }
        assert!(attempt < 99, "storage scroll fixture resize did not settle");
    }
    window
        .update(cx, |_, window, cx| {
            // Position the complete bounded list within the viewport.
            for _ in 0..35 {
                window.render_frame(cx);
                let bounds = window.find("storage-copy-list").bounds();
                let visible = window.find("settings-section-scroll").bounds().intersect(
                    &Bounds::new(point(px(0.), px(0.)), window.viewport_size()),
                );
                if bounds.origin.y >= visible.top() + px(8.)
                    && bounds.bottom() <= visible.bottom() - px(8.)
                {
                    break;
                }
                let delta = if bounds.origin.y < visible.top() + px(8.) {
                    80.
                } else {
                    -80.
                };
                scroll_settings(window, delta, cx);
            }
            let initial = window.find("storage-copy-list").bounds();
            let visible = window.find("settings-section-scroll").bounds().intersect(
                &Bounds::new(point(px(0.), px(0.)), window.viewport_size()),
            );
            assert!(initial.top() >= visible.top() && initial.bottom() <= visible.bottom(),
                "scrollbar fixture must fit its real ancestor clip: list={initial:?}; visible={visible:?}");
            assert!(view.read(cx).storage_ui.scroll.max_offset_for_scrollbar().y > px(0.));
            for delta in [-80., -10000., -80., 10000., 80.] {
                window.scroll(
                    "storage-copy-list",
                    ScrollDelta::Pixels(point(px(0.), px(delta))),
                    cx,
                );
                assert_eq!(
                    window.find("storage-copy-list").bounds().origin,
                    initial.origin,
                    "nested wheel moved Settings"
                );
            }
            assert_eq!(view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar().y, px(0.));
            eprintln!("storage thumb before: list={initial:?}; outer={:?}; offset={:?}; max={:?}", window.find("settings-section-scroll").bounds(), view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar(), view.read(cx).storage_ui.scroll.max_offset_for_scrollbar());
            // Base Scrollbar is a custom Element; its vertical track is at the
            // viewport's right edge (the element itself spans the whole list).
            window.drag(
                point(initial.right() - px(8.), initial.origin.y + px(8.)),
                point(initial.right() - px(8.), initial.bottom() - px(8.)),
                cx,
            );
            eprintln!("storage thumb after: offset={:?}; max={:?}; logical_top={:?}", view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar(), view.read(cx).storage_ui.scroll.max_offset_for_scrollbar(), view.read(cx).storage_ui.scroll.logical_scroll_top());
            window
                .render_to_image()
                .unwrap()
                .save(database.with_file_name("storage-contained-scroll.png"))
                .unwrap();
            assert!(
                view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar().y
                    == -view.read(cx).storage_ui.scroll.max_offset_for_scrollbar().y,
                "scrollbar thumb drag must reach the final file"
            );
            let old_top = window.find("storage-copy-list").bounds().origin.y;
            // A small window can leave the outer section at its bottom limit.
            // First move up through its padding, then move down to the original
            // position. Neither wheel may leak into the nested file list.
            let page = window.find("settings-section-scroll").bounds();
            let outside = point(page.origin.x + px(5.), page.center().y);
            let inner_offset = view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar();
            // The outer div does not expose an app-owned ScrollHandle. Measure
            // its actual range with one stable child's position at both ends,
            // then use the bottom as a baseline that permits an upward move.
            scroll_settings(window, 10000., cx);
            let outer_top_anchor = window.find("storage-copy-list").bounds().origin.y;
            scroll_settings(window, -10000., cx);
            let outer_bottom_anchor = window.find("storage-copy-list").bounds().origin.y;
            let outer_max = outer_top_anchor - outer_bottom_anchor;
            let original_outer_offset = old_top - outer_top_anchor;
            assert!(outer_max > px(0.), "outer-scroll fixture needs a nonzero actual range");
            assert_eq!(view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar(), inner_offset);
            eprintln!("outer wheel fixture: original_offset={original_outer_offset:?}; max={outer_max:?}; at_bottom={}; section={page:?}; list_before={old_top:?}; inner_offset={inner_offset:?}; inner_max={:?}", original_outer_offset == -outer_max, view.read(cx).storage_ui.scroll.max_offset_for_scrollbar());
            window.dispatch_event(
                ScrollWheelEvent {
                    position: outside,
                    delta: ScrollDelta::Pixels(point(px(0.), px(60.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            let raised_top = window.find("storage-copy-list").bounds().origin.y;
            eprintln!("outer wheel up: list_top={raised_top:?}; offset={:?}", raised_top - outer_top_anchor);
            assert!(
                raised_top > outer_bottom_anchor,
                "Settings must scroll up outside the file list"
            );
            assert_eq!(view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar(), inner_offset);
            window.dispatch_event(
                ScrollWheelEvent {
                    position: outside,
                    delta: ScrollDelta::Pixels(point(px(0.), -(raised_top - outer_bottom_anchor))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            eprintln!("outer wheel down: list_top={:?}; offset={:?}", window.find("storage-copy-list").bounds().origin.y, window.find("storage-copy-list").bounds().origin.y - outer_top_anchor);
            assert!(
                window.find("storage-copy-list").bounds().origin.y < raised_top,
                "Settings must scroll outside the file list"
            );
            assert_eq!(
                window.find("storage-copy-list").bounds().origin.y,
                outer_bottom_anchor,
                "outer wheel roundtrip did not restore the section position"
            );
            assert_eq!(view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar(), inner_offset);
            scroll_settings(window, f32::from(old_top - outer_bottom_anchor), cx);
            assert_eq!(window.find("storage-copy-list").bounds().origin.y, old_top, "outer boundary probe did not restore the original offset");
            assert_eq!(view.read(cx).storage_ui.scroll.scroll_px_offset_for_scrollbar(), inner_offset);
            window.click("settings-section-appearance", cx);
            window.click("appearance-workspace", cx);
            click_scrolled(window, "show-file-paths".into(), "page-scroll", cx);
            crate::smoke_library::resize(window, storage_size);
            assert!(
                !view
                    .read(cx)
                    .preferences_ui
                    .draft
                    .as_ref()
                    .unwrap()
                    .show_file_paths
            );
            assert!(
                view.read(cx)
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .settings
                    .show_file_paths
            );
            click_scrolled(window, "save-settings".into(), "page-scroll", cx);
        })
        .unwrap();
    until(
        window,
        view,
        "path setting disabled again",
        |app, cx| {
            !app.preferences_ui.busy
                && !app.settings_dirty(cx)
                && !app.snapshot.as_ref().unwrap().settings.show_file_paths
        },
        cx,
    )
    .await;
    window
        .update(cx, |_, window, cx| {
            window.click(("nav", 1_usize), cx);
            window.render_frame(cx);
            assert!(window.try_find("note-file-path").is_none());
            view.update(cx, |app, cx| {
                app.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            });
        })
        .unwrap();
    crate::smoke_visibility::capture(
        window,
        &editor,
        &database.with_file_name("native-after-settings.bmp"),
        cx,
    )
    .await;
    window
        .update(cx, |_, window, _| {
            crate::smoke_library::resize(window, size(px(1000.), px(750.)))
        })
        .unwrap();
    crate::smoke_visibility::capture(
        window,
        &editor,
        &database.with_file_name("native-after-resize.bmp"),
        cx,
    )
    .await;
    assert_eq!(std::fs::read(&note.path).unwrap(), bytes);
    std::fs::write(database.with_file_name("interactions-smoke-result.txt"),
        "PASS: Enter directory creation; unselected folder right-click rename and row-menu child creation; folder actions use menus and modal survives menu dismissal; actual pointer drag; right-click Move to Unclassified; explicit path copy; show-path checkbox stages until save; dirty-settings continue/discard/save-and-continue; nested wheel at both boundaries, actual scrollbar drag and outer scrolling outside the list; source bytes unchanged.\n").unwrap();
}

#[cfg(test)]
mod notice_drag_tests {
    use super::blocking_notice_button;
    use gpui_kit::{
        AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
        SharedString, StatefulInteractiveElement, Styled, TestAppContext, TestSupportExt, Window,
        div, px, size, test::TestWindowExt,
    };
    use std::{cell::Cell, rc::Rc, time::Duration};
    use studyapp_ui::notifications::NotificationCenter;

    struct Preview;
    impl Render for Preview {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size(px(20.))
        }
    }
    struct Fixture {
        center: Entity<NotificationCenter>,
        starts: Rc<Cell<usize>>,
        drops: usize,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let starts = self.starts.clone();
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .id("probe-source")
                        .test_support()
                        .absolute()
                        .left(px(187.))
                        .top(px(242.))
                        .w(px(255.))
                        .h(px(32.))
                        .on_drag("note".to_owned(), move |_, _, _, cx| {
                            starts.set(starts.get() + 1);
                            cx.new(|_| Preview)
                        }),
                )
                .child(
                    div()
                        .id("probe-target")
                        .test_support()
                        .absolute()
                        .left(px(187.))
                        .top(px(338.))
                        .w(px(255.))
                        .h(px(32.))
                        .on_drop(cx.listener(|this, _: &String, _, cx| {
                            this.drops += 1;
                            cx.notify();
                        })),
                )
                .child(self.center.clone())
        }
    }

    #[gpui_kit::test]
    fn real_notice_dismissal_unblocks_the_original_drag_gesture(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            let center = cx.new(|_| NotificationCenter::new());
            center.update(cx, |center, cx| {
                center.freeze_time();
                center.push(Ok("笔记已创建".into()), cx);
                center.push(
                    Err(format!(
                        "部分图片无法显示（1 项）：studyapp-media:{}",
                        "abcdef0123456789".repeat(100)
                    )),
                    cx,
                );
                center.advance_time(Duration::from_millis(240), cx);
            });
            Fixture {
                center,
                starts: Default::default(),
                drops: 0,
            }
        });
        cx.simulate_resize(size(px(1028.), px(780.)));
        cx.update(|window, cx| window.render_frame(cx));
        let center = fixture.read_with(cx, |fixture, _| fixture.center.clone());
        let button = cx.update(|window, cx| {
            let points = [
                window.find("probe-source").bounds().center(),
                window.find("probe-target").bounds().center(),
            ];
            let (_, button) = blocking_notice_button(window, center.read(cx), &points)
                .expect("fixture warning must cover the real drag down point");
            assert_ne!(
                button,
                SharedString::from("dismiss-notice"),
                "nonblocking success card should remain"
            );
            window.drag_to("probe-source", "probe-target", cx);
            button
        });
        fixture.read_with(cx, |fixture, _| {
            assert_eq!(
                fixture.starts.get(),
                0,
                "occluding notice should prevent drag start"
            );
            assert_eq!(fixture.drops, 0);
        });
        cx.update(|window, cx| window.click(button, cx));
        center.update(cx, |center, cx| {
            center.advance_time(Duration::from_millis(240), cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                center.read(cx).visible_count(),
                1,
                "dismissal must preserve the nonblocking success notice"
            );
            let points = [
                window.find("probe-source").bounds().center(),
                window.find("probe-target").bounds().center(),
            ];
            assert!(blocking_notice_button(window, center.read(cx), &points).is_none());
            window.drag_to("probe-source", "probe-target", cx);
        });
        fixture.read_with(cx, |fixture, _| {
            assert_eq!(fixture.starts.get(), 1);
            assert_eq!(fixture.drops, 1);
        });
    }
}
