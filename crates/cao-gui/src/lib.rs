//! Cathedral Assets Optimizer's main window over the composition root (#508).
//!
//! - [`App`] is the window and its behaviour: startup, profiles, saving and
//!   loading settings, the menu, and the modal dialogs.
//! - [`Desktop`] is the seam for native dialogs and shell calls;
//!   [`NativeDesktop`] is the real one.
//! - [`owned_window_attributes`] is the winit hook `main` installs so modal
//!   dialogs are created owned by the main window.
//! - [`RunView`] is what the window shows of an Optimization Run: its
//!   progress, outcome and run-detail rows (#510).
//! - [`formats`] maps the unwanted-format names the window shows to
//!   `DXGI_FORMAT` numbers.
//!
//! The `.slint` components themselves are compiled in `cao-gui-ui` and
//! re-exported here as [`ui`].

mod app;
mod desktop;
mod form;
pub mod formats;
mod modal;
mod run_view;

pub use app::{App, StartupError};
pub use cao_gui_ui as ui;
pub use desktop::{Desktop, MessageLevel, NativeDesktop};
pub use modal::{depth as modal_depth, owned_window_attributes};
pub use run_view::{RunView, RunViewState};
