//! A Dry Run writes a "would extract" Application Log line for each Archive
//! extraction it plans, in the style of the Loose Asset optimizers' "would
//! be optimized" lines (#641).
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

/// One captured `log` record: its level and message.
#[derive(Debug, PartialEq, Eq)]
struct Captured {
    level: Level,
    message: String,
}

/// A `log` sink keeping every info-or-more-severe record.
struct Capture(Mutex<Vec<Captured>>);

impl Log for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0.lock().unwrap().push(Captured {
                level: record.level(),
                message: record.args().to_string(),
            });
        }
    }

    fn flush(&self) {
        // Records are held in memory, so there is nothing to flush.
    }
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

/// Each planned extraction is logged once, naming the Archive, its Mod Root
/// and the files it would add: the entries it wins, so a shadowed entry is
/// not counted. Nothing is logged as extracted.
#[test]
fn a_dry_run_logs_each_planned_extraction() {
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(LevelFilter::Info);
    let root = canonical(&scratch_dir("planned-extraction-log"));
    let first = root.join("a.bsa");
    let second = root.join("b.bsa");
    write_archive(
        &first,
        &[
            entry("meshes/first.nif", b"first"),
            entry("meshes/shared.nif", b"first"),
        ],
    );
    write_archive(&second, &[entry("meshes/shared.nif", b"second")]);
    let toolset = Arc::new(ScriptedToolset::default());

    let result = execute_run(
        &request(
            ExecutionMode::DryRun,
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
    let records = CAPTURE.0.lock().unwrap();
    // Deterministic Archive Precedence puts `a.bsa` first, so `b.bsa` loses
    // its only entry and would add nothing.
    for (archive, files) in [(&first, 2), (&second, 0)] {
        let expected = Captured {
            level: Level::Info,
            message: format!(
                "would extract {} into {} ({files} files)",
                archive.display(),
                root.display()
            ),
        };
        assert!(records.contains(&expected), "{records:?}");
    }
    assert!(
        !records
            .iter()
            .any(|record| record.message.starts_with("BSA successfully extracted")),
        "{records:?}"
    );
}
