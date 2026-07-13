//! Throwaway native specimen for choosing the fork's product name and visual identity.

slint::include_modules!();

/// Opens the branding specimen and runs the native Slint event loop.
fn main() -> Result<(), slint::PlatformError> {
    MainWindow::new()?.run()
}
