//! The Cathedral Assets Optimizer exe: finds the app directory, sets up Slint
//! and the Application Log, and opens the main window (#508, #511).

// A GUI exe opens no console, as CMake's `add_executable(... WIN32 ...)` did (#471).
#![windows_subsystem = "windows"]

use std::process::ExitCode;
use std::rc::Rc;

use anyhow::Context;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        // There is no window, or it failed; a standalone box is all that is
        // left, and it blocks so the exit waits for the user (exit code 1).
        Err(error) => cao_gui::fail_startup(&error, |title, text| {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title(title)
                .set_description(text)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }),
    }
}

/// Sets up the platform and the Application Log, then runs the window until it
/// closes.
fn run() -> anyhow::Result<()> {
    // Before any window exists: the winit backend reads it at window creation.
    cao_gui_unsafe::use_slint_drawn_menu_bar();
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .with_winit_window_attributes_hook(cao_gui::owned_window_attributes)
        .select()
        .context("the winit/femtovg backend could not be started")?;

    // Deviation 1: the app directory is the exe's, never the working directory,
    // and only here is it derived; everything else is handed it.
    let app_dir = cao_winfs::exe_directory().context("the exe's directory could not be found")?;
    // Before the window, as C++ `configureLogging` was, so the Log tab replays
    // everything logged from here on (deviation 24).
    let log =
        cao_gui::bootstrap_log(&app_dir).context("the Application Log could not be started")?;
    let app = cao_gui::App::new(&app_dir, Rc::new(cao_gui::NativeDesktop))?;
    app.attach_log(log);
    app.run()?;
    Ok(())
}
