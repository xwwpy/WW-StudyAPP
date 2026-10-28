# Menu and scrollbar integration patches

This is the runtime source of `gpui-component` 0.7.3 from crates.io, distributed
under Apache-2.0. The original `LICENSE-APACHE` is included.

- Archive: https://static.crates.io/crates/gpui-component/gpui-component-0.7.3.crate
- Archive SHA-256: `465a363235cd4062ef5f0953fc096fc764ed6f53dfbd9f409f669f93ea3b1ff9`
- Upstream repository: https://github.com/longbridge/gpui-kit
- Upstream snapshot recorded in package metadata:
  `67265a36e260ffe03c5aaebdcd8d654dbf892eab`, path `crates/component`.
- `src/`, `locales/`, the build script, README and original license are retained.
  The normalized manifest retains all runtime dependencies and features.
  Upstream integration tests and their seven `[[test]]` manifest declarations,
  the package-local lockfile, registry markers and redundant original manifest
  are omitted. No runtime module or locale was omitted.

## Change

In `src/menu/menu_item.rs`, `MenuItemElement::render` applies an arrow cursor
after instance styling, then replaces it with a pointing hand when the row is
enabled.

`PopupMenuLayout` additionally reports the actual popup and active submenu
rectangles. The native WebView host clips only those rounded surfaces instead
of hiding the complete document whenever a menu opens. Layout reports are
coalesced, inactive submenu regions are removed, and dismissal resets reports
so reopening a retained menu publishes its regions again. Existing activation,
keyboard navigation and dismissal semantics remain unchanged.

The cursor covers the complete row, including its padding, for ordinary
actions, custom items and submenu triggers. Disabled rows, labels and
separators retain an arrow. Activation, hover selection, focus, keyboard
navigation, dismissal and layout remain the upstream implementation.

An application ancestor cannot provide this policy: each popup and submenu
occludes lower hitboxes, and submenus are deferred outside the parent menu's
layout bounds. Patch the shared row itself so nested menus follow the same
policy without reconstructing the menu implementation in application code.

The application retains native Windows cursor checks in its UI verification.
GPUI's public test window snapshots do not expose the resolved cursor, so a
style-only surrogate would not verify pointer hit testing on submenu rows.

Re-evaluate this patch when upgrading GPUI Component.

`src/scroll/scrollable.rs` gives the scrollbar decoration a zero-size layout
footprint and resolves its viewport from the tracked scroll handle. GPUI counts
absolute children when measuring scroll content: a full-size overlay adds the
container padding to that measurement, producing scrollbars for fitting content
and excessive extents for long content. Its layout bounds also follow scroll
offsets. The tracked viewport keeps both thumb painting and hit testing at the
visible container. Application regression tests cover fitting content, padding,
horizontal/vertical overflow and shrinking content.

Virtual lists must be prepainted before this decoration so their tracked
viewport reflects the current frame. StudyApp appends a shared list scrollbar
after the list and extends only its right edge when a caller reserves a gutter;
the actual list height, offsets and drag hooks remain authoritative. The padded
viewport regression in this vendor module asserts painted thumb placement and
drag hit geometry instead of the decoration's deliberately empty layout bounds.
