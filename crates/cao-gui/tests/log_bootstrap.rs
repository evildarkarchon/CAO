//! Bootstrapping the Application Log at startup (#511): a log that cannot be
//! opened stops CAO before the window, with a critical box and exit code 1, as
//! C++ `main`'s `displayError` did.
//!
//! The successful bootstrap installs the process's `log` sink, which can happen
//! once per process, so it is exercised by `log_tab.rs` instead.

mod common;

use std::cell::RefCell;
use std::process::ExitCode;

use common::*;

#[test]
fn a_log_that_cannot_be_opened_stops_startup_with_a_critical_box_and_exit_code_1() {
    let app = app_dir("log_bootstrap_failure");
    // A file named `logs` leaves no room for the `logs/<profile>/` folder.
    std::fs::write(app.join("logs"), "not a folder").unwrap();

    let error = match cao_gui::bootstrap_log(&app) {
        Ok(_) => panic!("the Application Log opened under a file named `logs`"),
        Err(error) => error,
    };
    assert!(
        matches!(error, cao_gui::BootstrapError::Log(_)),
        "unexpected error: {error}"
    );

    let shown = RefCell::new(Vec::new());
    let code = cao_gui::fail_startup(&error, |title, text| {
        shown.borrow_mut().push((title.to_owned(), text.to_owned()));
    });

    assert_eq!(code, ExitCode::from(1));
    let shown = shown.into_inner();
    assert_eq!(shown.len(), 1, "expected one critical box, got {shown:?}");
    let (title, text) = &shown[0];
    assert_eq!(title, "Cathedral Assets Optimizer");
    // The box names the file that could not be opened, under the startup
    // profile `common.ini` remembers.
    assert!(
        text.contains(&app.join("logs").join("TES5").display().to_string()),
        "{text}"
    );
}
