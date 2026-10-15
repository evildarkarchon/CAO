//! The `unsafe` calls `cao-gui` cannot avoid, each behind a small safe wrapper.
//!
//! `cao-gui` keeps the workspace's `unsafe_code = "forbid"`; this crate is the
//! GUI's only exception (#508). Before adding a wrapper here, look for a safe
//! API: the dialog owner window, for example, needs none, because winit's
//! `WindowAttributesExtWindows::with_owner_window` is safe and Slint's
//! `BackendSelector::with_winit_window_attributes_hook` reaches it.

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
}
