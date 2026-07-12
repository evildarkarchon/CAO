//! Throwaway executable for comparing Slint interaction-contract directions.

slint::include_modules!();

/// Opens the interaction prototype and runs the native Slint event loop.
fn main() -> Result<(), slint::PlatformError> {
    MainWindow::new()?.run()
}
