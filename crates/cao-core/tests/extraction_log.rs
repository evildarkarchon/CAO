//! Core's Archive extraction writes the "BSA successfully extracted"
//! Application Log line for each Archive it extracts, wherever extraction
//! runs (#584).
//!
//! This is its own test binary with one test, because `log::set_logger`
//! succeeds only once per process.

mod common;

use std::sync::{Arc, Mutex};

use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{CancellationToken, RunOutcome};
use cao_core::test_support::{
    CountingCleanup, RecordingSink, ScriptedToolset, ToolsetConfiguration, entry, execute_run,
    write_archive,
};
use common::{canonical, request, scratch_dir};
use log::{Level, LevelFilter, Log, Metadata, Record};

/// A `log` sink keeping every info record's message.
struct Capture(Mutex<Vec<String>>);

impl Log for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
    }

    fn flush(&self) {
        // Records are held in memory, so there is nothing to flush.
    }
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

#[test]
fn a_run_using_core_archive_work_logs_each_successful_extraction() {
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(LevelFilter::Info);
    let root = canonical(&scratch_dir("extraction-log"));
    let archive = root.join("content.bsa");
    write_archive(&archive, &[entry("meshes/extracted.nif", b"extracted")]);
    let toolset = Arc::new(ScriptedToolset::default().with_core_archive_work());

    let result = execute_run(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveExtraction],
        ),
        &mut CountingCleanup::default(),
        Some(&RecordingSink::default()),
        Some(&ToolsetConfiguration::sse(Some(toolset))),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let expected = format!("BSA successfully extracted: {}", archive.display());
    let messages = CAPTURE.0.lock().unwrap();
    assert!(messages.contains(&expected), "{messages:?}");
}
