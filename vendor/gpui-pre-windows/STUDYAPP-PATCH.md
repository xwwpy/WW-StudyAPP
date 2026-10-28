# Native child window composition patch

This is the source of `gpui-pre-windows` 0.3.9 from crates.io, distributed
under Apache-2.0. The original `LICENSE-APACHE` is included.

- Archive: https://static.crates.io/crates/gpui-pre-windows/gpui-pre-windows-0.3.9.crate
- Archive SHA-256: `513d4f0fc7d596d3bdef2a73a524ffb83dabc95bf484f2855a698e7f431290a9`
- Upstream repository: https://github.com/zed-industries/zed
- Upstream snapshot recorded in package metadata:
  `93d745668805029476e6d7fbb19652116fed51de`
- Original normalized `Cargo.toml`, `Cargo.toml.orig`, build script and source
  files are retained. The registry marker and package-local lockfile are not
  needed for the workspace dependency.

## Change

In `src/directx_renderer.rs`, create the DirectComposition target with
`topmost = false`.

Microsoft documents that `topmost = true` places the visual tree above the
window's native children. GPUI's opaque card background therefore hid the
embedded WebView2 editor despite its HWND/controller both being visible,
valid bounds, correct hit testing, and successful browser IPC. With `false`,
native child windows clip the GPUI visual tree.

Reference: https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-createtargetforhwnd

The regression test captures actual desktop pixels, verifies the first note's
visible text, clicks the native editor, and enters Unicode through Windows
keyboard input. Browser DOM state and CDP screenshots do not test this issue.

Re-evaluate this patch when upgrading GPUI. Do not remove it merely because
browser-only tests pass.

In `src/events.rs`, immediately update the actual Win32 cursor when its
handle changes, including `Some(hand)` to `Some(resize)`. Upstream only
compares whether a handle exists, so a stationary pointer can retain the
previous shape until another `WM_SETCURSOR` arrives. This was reproduced by
the native CI hand-to-divider check. Since the message is broadcast, only
the window currently owning client-area pointer input or mouse capture may
call `SetCursor`; native child windows, non-client resizing and cursor
visibility retain their own behavior.

The regression moves once per target and reads the actual UI-thread
`GetCursor` handle, requiring a non-null expected hand/resize/arrow. A
non-null global `GetCursorInfo` handle must also agree; suppressed CI
desktop cursors may have a null global handle.

## Window movement diagnostics (test-support only)

With the `test-support` feature, `WW_STUDY_WINDOW_DRAG_TRACE` may name a JSONL
file for one aggregate record per Win32 move/resize modal loop. It reports move
callbacks, queued work, draw requests, actual rendered scenes and elapsed time.
The file is written only after leaving the loop. These diagnostics are absent
from ordinary builds; they do not change the timer or callback behavior.

The separate `vendor/gpui-pre` patch avoids rebuilding an unchanged Windows
client scene on pure desktop movement while retaining every bounds observer.
