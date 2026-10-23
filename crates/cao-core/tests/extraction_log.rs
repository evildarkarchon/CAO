//! Core's Archive extraction writes the "BSA successfully extracted"
//! Application Log line for each Archive it extracts, wherever extraction
//! runs (#584).
//!
//! This is its own test binary with one test, because `log::set_logger`
//! succeeds only once per process.

mod common;

use std::sync::Arc;

use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{CancellationToken, RunOutcome};
use cao_core::test_support::{
    CountingCleanup, RecordingSink, ScriptedToolset, ToolsetConfiguration, entry, execute_run,
    write_archive,
};
use common::capture_log::{self, Captured};
use common::{canonical, request, scratch_dir};
use log::Level;

#[test]
fn a_run_logs_each_successful_extraction() {
    capture_log::install();
    let root = canonical(&scratch_dir("extraction-log"));
    let archive = root.join("content.bsa");
    write_archive(&archive, &[entry("meshes/extracted.nif", b"extracted")]);
    let toolset = Arc::new(ScriptedToolset::default());

    let result = execute_run(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveExtraction],
        ),
        &mut CountingCleanup::default(),
        Some(&RecordingSink::default()),
        Some(&ToolsetConfiguration::sse(toolset)),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    // The debug Application Log layout prints the module path, so the line
    // keeps the `cao_core::run::toolset` path it had before core assembled
    // Archive work (#581).
    let expected = Captured {
        level: Level::Info,
        target: "cao_core::run::toolset".to_owned(),
        module_path: Some("cao_core::run::toolset".to_owned()),
        message: format!("BSA successfully extracted: {}", archive.display()),
    };
    let records = capture_log::records();
    assert!(records.contains(&expected), "{records:?}");
}
