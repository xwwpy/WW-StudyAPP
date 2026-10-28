# Windows position-only invalidation patch

This directory contains the published `gpui-pre` 0.3.9 source under Apache-2.0.
The original `LICENSE-APACHE`, normalized manifest, original manifest, build
script, resources, examples, tests and documentation are retained.

- Archive: https://static.crates.io/crates/gpui-pre/gpui-pre-0.3.9.crate
- Archive SHA-256: `34f61c1cfd36fc9c436af09308c963822ae27a45a84feb21a77ba5c9e051f1c0`
- Upstream: https://github.com/zed-industries/zed
- Packaged upstream snapshot: `93d745668805029476e6d7fbb19652116fed51de`

## Change

Windows `on_moved` keeps immediately refreshing the platform-derived viewport,
scale factor and client mouse position and notifying every bounds observer.
It invalidates the client scene only when one of those client values changes.
Moving the already-rendered client on the desktop is handled by Windows/DWM.
Observers remain free to invalidate their own content or call `refresh()`.

The resize callback still uses the original unconditional `bounds_changed`
path, including same-size restore notifications. Other platforms retain their
original `on_moved` behavior. The Windows timer, runnable dispatch, native child
composition, DPI handlers and observer ordering remain unchanged.

`src/platform/test/window.rs` adds `simulate_move`; `src/app/test_context.rs`
exposes scoped movement and frame-request simulation for the focused public-API
regression in `crates/studyapp-desktop/src/smoke_window_drag.rs`. It counts real
view renders and verifies that pure moves deliver every observer notification
without rebuilding the client scene, observer-requested refresh remains effective,
and changed client mouse position, DPI and same-size resize still invalidate.
This regression runs with the normal workspace `--all-features` tests.
StudyApp's `window-drag` native smoke scenario also drives an actual caption
drag on Today, Notes and Settings and checks WebView bounds/visibility/regions.

## References

- https://github.com/zed-industries/zed/blob/main/crates/gpui/src/window.rs
- https://github.com/zed-industries/zed/blob/main/crates/gpui_windows/src/events.rs
- https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-entersizemove

Re-evaluate this narrow patch when upgrading GPUI. Do not replace it with
skipping or delaying the native moved callback: anchored overlays and other
bounds observers require prompt updates.

`ListState::set_item_focus_handle` updates an existing row's focus registration
without replacing its measured size or estimated height. Settings uses this for
offscreen inline shortcut recording and appearance input groups. The original
`splice_focusable` API intentionally invalidates replaced row contents and remains
unchanged. A headless regression reproduces the previous loss of 140 px of scroll
extent when ending an offscreen recording, then verifies stable extent, anchors,
and keyboard dispatch with the new API.

## Variable-height list estimates

`src/elements/list.rs` preserves each item's previous height as an unmeasured
size hint when the viewport width changes, including its first layout. The
visible rows still run layout at the new width. Previously this invalidation
erased `with_uniform_item_height` hints for every offscreen row, collapsing the
scrollbar extent to only the rows already visited. StudyApp's headless settings
regression checks initial layout, width changes and inline row-height changes
without constructing the entire list.

Top-aligned lists also anchor an explicit scrollbar drag to the last item when
the pointer reaches the end of its track. The drag keeps its original estimated
height until release, but rows measured along the way can increase the live
height. Keeping the old pixel offset would then leave the last rows below the
viewport. This end anchor is a single positioning action: normal lists do not
begin following subsequently appended items, and intermediate drags retain their
chosen position. Bottom-aligned lists and the existing Tail follow behavior are
unchanged. Storage's headless regression exercises actual thumb drags with both
increasing and decreasing row-height estimates and checks the final row's bounds.
