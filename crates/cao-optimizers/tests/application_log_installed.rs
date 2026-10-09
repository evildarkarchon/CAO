//! The Application Log installed as the process's `log` sink, as `main`
//! bootstraps it, then redirected the way each run start does.
//!
//! This is its own test binary with one test, because `log::set_logger`
//! succeeds only once per process.

mod common;

use cao_optimizers::application_log::ApplicationLog;
use common::scratch_dir;
use log::LevelFilter;

#[test]
fn the_installed_sink_follows_each_runs_redirect_through_the_log_macros() {
    let dir = scratch_dir("log-installed");
    let startup = dir.join("logs").join("SSE").join("startup.html");
    let run = dir.join("logs").join("SSE").join("run.html");

    let sink = ApplicationLog::open(&startup, false)
        .unwrap()
        .install()
        .unwrap();
    assert_eq!(log::max_level(), LevelFilter::Info);
    log::info!("bootstrapped");
    log::debug!("not at the info level");

    // Deviation 23: turning debug logging on for a run keeps the log going.
    sink.redirect(&run, true).unwrap();
    assert_eq!(log::max_level(), LevelFilter::Trace);
    log::debug!("debug for this run");
    log::trace!("verbose for this run");

    let startup_text = std::fs::read_to_string(&startup).unwrap();
    assert!(
        startup_text.contains("[INFO] bootstrapped"),
        "{startup_text}"
    );
    assert!(
        !startup_text.contains("not at the info level"),
        "{startup_text}"
    );

    let run_text = std::fs::read_to_string(&run).unwrap();
    let module = module_path!();
    assert!(
        run_text.contains(&format!(" DEBUG{{{module}@"))
            && run_text.contains("} debug for this run"),
        "{run_text}"
    );
    assert!(
        run_text.contains(&format!(" VERB {{{module}@"))
            && run_text.contains("} verbose for this run"),
        "{run_text}"
    );

    sink.redirect(&run, false).unwrap();
    assert_eq!(log::max_level(), LevelFilter::Info);
}
