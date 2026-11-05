//! EWMH requests on the widget's actual GPUI XCB connection. No global display,
//! process lookup, window search, event draining or ownership of the connection.
use anyhow::{Context, Result, ensure};
use std::{ffi::c_void, ptr::NonNull};
use x11rb::{
    connection::Connection,
    protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, MapState, PropMode,
    },
    wrapper::ConnectionExt as _,
    xcb_ffi::XCBConnection,
};

fn atoms(connection: &XCBConnection, window: u32, property: u32) -> Result<Vec<u32>> {
    let reply = connection
        .get_property(false, window, property, AtomEnum::ATOM, 0, 4096)?
        .reply()?;
    ensure!(reply.bytes_after == 0, "窗口状态属性超过安全读取范围");
    if reply.type_ == u32::from(AtomEnum::NONE) {
        return Ok(vec![]);
    }
    ensure!(
        reply.type_ == u32::from(AtomEnum::ATOM) && reply.format == 32,
        "窗口状态属性格式无效"
    );
    Ok(reply.value32().context("无法读取窗口状态属性")?.collect())
}

/// # Safety
/// `raw` is the live, borrowed GPUI connection and `window` is this widget's
/// own live XID. The caller keeps both native handle borrows until return.
pub(super) unsafe fn apply(
    raw: NonNull<c_void>,
    window: u32,
    pinned: bool,
    request: bool,
) -> Result<bool> {
    // SAFETY: the caller supplies a live borrowed connection; false prevents
    // xcb_disconnect on drop. Only our replies are read, never GPUI events.
    let connection = unsafe { XCBConnection::from_raw_xcb_connection(raw.as_ptr(), false)? };
    let state = connection
        .intern_atom(false, b"_NET_WM_STATE")?
        .reply()?
        .atom;
    let above = connection
        .intern_atom(false, b"_NET_WM_STATE_ABOVE")?
        .reply()?
        .atom;
    let mut states = atoms(&connection, window, state)?;
    let mapped = connection.get_window_attributes(window)?.reply()?.map_state != MapState::UNMAPPED;
    if states.contains(&above) == pinned {
        // Before mapping this is only a client hint, not a WM acknowledgement.
        return Ok(mapped);
    }
    if !request {
        return Ok(false);
    }
    let root = connection.get_geometry(window)?.reply()?.root;
    let supported = connection
        .intern_atom(false, b"_NET_SUPPORTED")?
        .reply()?
        .atom;
    let supported = atoms(&connection, root, supported)?;
    ensure!(
        supported.contains(&state) && supported.contains(&above),
        "当前窗口管理器不支持 EWMH 置顶"
    );
    if !mapped {
        // Initial hints before mapping belong to the client. Preserve every
        // unrelated state, including fullscreen/sticky and taskbar policy.
        states.retain(|atom| *atom != above);
        if pinned {
            states.push(above);
        }
        connection
            .change_property32(PropMode::REPLACE, window, state, AtomEnum::ATOM, &states)?
            .check()?;
        connection.flush()?;
        return Ok(false);
    }
    // A mapped client's property belongs to the WM. Request add/remove, never
    // toggle or overwrite it, and wait for the WM's property acknowledgement.
    connection
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, window, state, [u32::from(pinned), above, 0, 1, 0]),
        )?
        .check()?;
    connection.flush()?;
    Ok(false)
}

/// Read native state for the explicitly enabled runner fixture, never apply it.
/// # Safety
/// The same live-connection/window ownership requirements as `apply` hold.
#[cfg(feature = "ui-test")]
pub(super) unsafe fn observe(raw: NonNull<c_void>, window: u32) -> Result<serde_json::Value> {
    let connection = unsafe { XCBConnection::from_raw_xcb_connection(raw.as_ptr(), false)? };
    let state = connection
        .intern_atom(true, b"_NET_WM_STATE")?
        .reply()?
        .atom;
    let above = connection
        .intern_atom(true, b"_NET_WM_STATE_ABOVE")?
        .reply()?
        .atom;
    let geometry = connection.get_geometry(window)?.reply()?;
    let position = connection
        .translate_coordinates(window, geometry.root, 0, 0)?
        .reply()?;
    let attributes = connection.get_window_attributes(window)?.reply()?;
    let focus = connection.get_input_focus()?.reply()?.focus;
    Ok(serde_json::json!({
        "pinned": atoms(&connection, window, state)?.contains(&above),
        "focus": focus,
        "bounds": [position.dst_x, position.dst_y, geometry.width, geometry.height],
        "managed": !attributes.override_redirect,
    }))
}
