//! The `unsafe` calls `cao-gui` cannot avoid, each behind a small safe wrapper:
//! the Slint-drawn menu bar's environment switch, and a monitor's work area.
//!
//! `cao-gui` keeps the workspace's `unsafe_code = "forbid"`; this crate is the
//! GUI's only exception (#508). Before adding a wrapper here, look for a safe
//! API: the dialog owner window, for example, needs none, because winit's
//! `WindowAttributesExtWindows::with_owner_window` is safe and Slint's
//! `BackendSelector::with_winit_window_attributes_hook` reaches it.

/// A monitor's work area, in physical pixels of the virtual screen: the
/// monitor less the taskbar and any docked app bars. `right` and `bottom` are
/// exclusive, as in a Win32 `RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The work area of the monitor showing most of the window `hwnd`, or of the
/// nearest monitor when the window is on none (GUI-6). `None` when Windows
/// cannot say, such as for a handle that names no window.
///
/// winit reports a monitor's whole size only, and a dialog placed by it alone
/// can open under the taskbar.
pub fn monitor_work_area(hwnd: isize) -> Option<WorkArea> {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };

    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: both calls take plain values and validate the handles they are
    // given: a stale or invalid `hwnd` makes `MonitorFromWindow` return the
    // nearest monitor or null, and `GetMonitorInfoW` fails on a null monitor
    // rather than reading through it. `info` is a live, writable
    // `MONITORINFO` whose `cbSize` names its own size, as the call requires.
    let found = unsafe {
        let monitor = MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST);
        !monitor.is_null() && GetMonitorInfoW(monitor, &mut info) != 0
    };
    found.then_some(WorkArea {
        left: info.rcWork.left,
        top: info.rcWork.top,
        right: info.rcWork.right,
        bottom: info.rcWork.bottom,
    })
}

/// Makes Slint draw the window's menu bar itself instead of using a native
/// `muda` menu, so the menu bar follows CAO's dark/light palette (#462, #466).
///
/// Slint 1.18 has no API or Cargo feature for this: `i-slint-backend-selector`
/// always enables the winit backend's `muda` feature, and the backend only
/// checks `std::env::var_os("SLINT_NO_MUDA").is_some()` when a window is
/// created. Call this before the first window exists; windows created earlier
/// keep the native menu.
pub fn use_slint_drawn_menu_bar() {
    // SAFETY: `set_var` is `unsafe` because a concurrent `getenv` on POSIX can
    // read freed memory. The workspace is Windows-only (spec #476), and the
    // standard library documents `set_var` as "always safe to call on Windows,
    // in single-threaded and multi-threaded programs": Windows guards the
    // process environment block with its own lock, and `std` reads it with
    // `GetEnvironmentVariableW`, never through a cached pointer.
    unsafe { std::env::set_var("SLINT_NO_MUDA", "1") };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The winit backend only checks that the variable exists
    /// (`i-slint-backend-winit` 1.18.1, `muda.rs`), so presence is the contract.
    #[test]
    fn the_slint_drawn_menu_bar_is_requested_through_the_environment() {
        use_slint_drawn_menu_bar();
        assert!(std::env::var_os("SLINT_NO_MUDA").is_some());
    }

    /// A handle that names no window is no fault: Windows answers with the
    /// nearest monitor or with none, and an area it gives is a real one.
    #[test]
    fn a_handle_that_names_no_window_gives_a_well_formed_area_or_none() {
        if let Some(area) = monitor_work_area(0) {
            assert!(area.left < area.right && area.top < area.bottom, "{area:?}");
        }
    }
}
