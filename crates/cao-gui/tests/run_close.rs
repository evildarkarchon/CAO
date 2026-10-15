//! Closing the window during a run (#510): the close cancels the run, the
//! window stays open through Safety Cleanup until the run reports its outcome,
//! and then closes with the controls still locked. This carries the intent of
//! the dropped C++ `GuiRunDispatchTests::closeWaitsForCleanupAndTerminalDelivery`.
//!
//! Run events reach the window through the event loop, so this file starts the
//! event-loop test backend, which a process may do once: it holds one test.

mod common;

use common::*;
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model};

#[test]
fn closing_mid_run_cancels_it_and_closes_once_safety_cleanup_has_reported() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let app_dir = app_dir("run_close");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let mods = app_dir.join("mod");
    std::fs::create_dir_all(&mods).unwrap();
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    ui.set_user_path(mods.display().to_string().into());
    ui.set_dry_run(true);
    click(ui, "Run");
    assert!(ui.get_running());

    // The title-bar close, while the run's events are still undelivered.
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(ui.window().is_visible(), "the window waits for the run");
    assert!(
        ui.get_running() && ui.get_cancelling(),
        "the run is cancelled"
    );
    assert!(!enabled(ui, "Cancel"));
    assert!(
        ui.get_status_text().starts_with("Cancelling - "),
        "{}",
        ui.get_status_text()
    );

    run_event_loop_until(|| !ui.window().is_visible());
    // The terminal result, published only after Safety Cleanup, was delivered
    // before the window closed.
    let details: Vec<String> = ui.get_run_details().iter().map(Into::into).collect();
    assert!(
        details.iter().any(|row| row.starts_with("Final Phase: ")),
        "{details:?}"
    );
    let status = ui.get_status_text();
    assert!(status == "Done" || status == "Cancelled", "{status}");
    // The controls stay locked until the window is gone, and no outcome box
    // pops up over a closing window.
    assert!(ui.get_running());
    assert!(desktop.message_titles().is_empty());
    // Safety Cleanup left no staging behind.
    let leftovers: Vec<_> = std::fs::read_dir(&mods)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
