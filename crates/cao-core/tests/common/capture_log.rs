//! A `log` sink for the Application Log tests, which assert on the lines a
//! run writes.
//!
//! `log::set_logger` succeeds only once per process, so each test binary
//! that installs this sink holds a single test; the sink is shared here
//! rather than copied into each binary.

use std::sync::{Mutex, MutexGuard};

use log::{Level, LevelFilter, Log, Metadata, Record};

/// One captured `log` record: what the Application Log layout prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub level: Level,
    pub target: String,
    pub module_path: Option<String>,
    pub message: String,
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
                target: record.target().to_owned(),
                module_path: record.module_path().map(str::to_owned),
                message: record.args().to_string(),
            });
        }
    }

    fn flush(&self) {
        // Records are held in memory, so there is nothing to flush.
    }
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

/// Installs the capture as the process's logger at the Info level.
///
/// # Panics
///
/// When a logger is already installed: call it once, from the binary's one
/// test, before the run under test logs anything.
pub fn install() {
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(LevelFilter::Info);
}

/// The records captured so far, in the order they were logged. The guard
/// holds the capture's lock, so drop it before logging anything more.
pub fn records() -> MutexGuard<'static, Vec<Captured>> {
    CAPTURE.0.lock().unwrap()
}
