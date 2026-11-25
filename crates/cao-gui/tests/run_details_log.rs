//! A run's detail rows reach the Application Log at its end (GUI-12): they are
//! written to the log file, where they can be copied and outlive the next run,
//! and the Log tab shows them as log rows rather than pinned detail rows.
//!
//! Run events and the Log tab's rows reach the window through the event loop,
//! so this file starts the event-loop test backend, which a process may do
//! once: it holds one test.

mod common;

use cao_gui::ui::{LogSeverity, LogTab};
use cao_optimizers::application_log::ApplicationLog;
use common::*;
use slint::{ComponentHandle, Model};

/// The texts of the Log tab's rows that came from the log, not pinned there.
fn log_rows(ui: &cao_gui::ui::MainWindow) -> Vec<(LogSeverity, String)> {
    ui.global::<LogTab>()
        .get_rows()
        .iter()
        .filter(|row| row.severity != LogSeverity::Detail)
        .map(|row| (row.severity, row.text.to_string()))
        .collect()
}

#[test]
fn an_ended_runs_detail_rows_are_written_to_the_application_log() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let app_dir = app_dir("run_details_log");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let mods = app_dir.join("mod");
    std::fs::create_dir_all(&mods).unwrap();
    let (app, _desktop) = open(&app_dir);
    // Attached without being installed as the `log` facade's sink: the rows
    // must reach it all the same.
    let log_path = app_dir.join("run.html");
    let log = ApplicationLog::open(&log_path, false).unwrap();
    app.attach_log(Box::leak(Box::new(log)));
    let ui = app.window();
    ui.set_user_path(mods.display().to_string().into());
    ui.set_dry_run(true);

    click(ui, "Run");
    run_event_loop_until(|| !ui.get_running());
    // The feed's drain is queued behind the run's end.
    run_event_loop_until(|| {
        log_rows(ui)
            .iter()
            .any(|(_, text)| text.ends_with("Final Phase: Archive Finalization"))
    });

    let file = String::from_utf8_lossy(&std::fs::read(&log_path).unwrap()).into_owned();
    assert!(
        file.contains("Final Phase: Archive Finalization"),
        "the file holds the run's evidence: {file}"
    );
    assert!(file.contains("Cancellation Observed: no"), "{file}");
    // Evidence is info; only failures and warnings are marked as such.
    let final_phase = log_rows(ui)
        .into_iter()
        .find(|(_, text)| text.ends_with("Final Phase: Archive Finalization"))
        .unwrap();
    assert_eq!(final_phase.0, LogSeverity::Info);
    assert!(
        detail_rows(ui).is_empty(),
        "the rows are log rows now, not pinned twice: {:?}",
        detail_rows(ui)
    );

    // The next run starts without dropping them from the tab.
    click(ui, "Run");
    assert!(
        log_rows(ui)
            .iter()
            .any(|(_, text)| text.ends_with("Final Phase: Archive Finalization"))
    );
    run_event_loop_until(|| !ui.get_running());
}
