//! A Dry Run reports its planned Archive creation in the Application Log as
//! the Loose Asset optimizers report theirs: a "would create" line for each
//! planned output Archive and for each Dummy Plugin it would need (#643).
//!
//! This is its own test binary with one test, because `log::set_logger`
//! succeeds only once per process.

mod common;

use std::sync::Arc;

use cao_core::routing::ExecutionMode;
use cao_core::run::{CancellationToken, RunOutcome};
use cao_core::test_support::{ArchiveRunRequest, FakeArchivePacker, ScriptedToolset};
use common::capture_log;
use common::{canonical, scratch_dir};
use log::Level;

/// A Dry Run that would split its meshes into two Archives and pack its
/// Textures into a third logs a "would create" line for each Archive, in
/// plan order, each followed by the Dummy Plugin it is the first to need.
#[test]
fn a_dry_run_logs_each_archive_and_dummy_plugin_it_would_create() {
    capture_log::install();
    let root = canonical(&scratch_dir("creation-plan-log")).join("mod");
    for (name, contents) in [
        ("meshes/a.nif", &b"twelve bytes"[..]),
        ("meshes/b.nif", b"twelve bytes"),
        ("textures/t.dds", b"texture"),
    ] {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }
    // Room for one mesh per Archive, so the meshes split. The Textures
    // Archive falls back to the same Dummy Plugin as the first mesh Archive,
    // which is reported once.
    let packer = FakeArchivePacker {
        archive_limit: 20,
        ..FakeArchivePacker::default()
    };
    let run = ArchiveRunRequest::new(&root).with_execution_mode(ExecutionMode::DryRun);
    let configuration = run.configuration(ScriptedToolset {
        packer: Arc::new(packer),
        ..ScriptedToolset::default()
    });

    let result = run.execute(&configuration, None, &CancellationToken::new());

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let shown = |name: &str| root.join(name).display().to_string();
    let expected = [
        format!("would create {} (1 files, 12 bytes)", shown("mod.bsa")),
        format!("would create Dummy Plugin {}", shown("mod.esp")),
        format!("would create {} (1 files, 12 bytes)", shown("mod0.bsa")),
        format!("would create Dummy Plugin {}", shown("mod0.esp")),
        format!(
            "would create {} (1 files, 7 bytes)",
            shown("mod - Textures.bsa")
        ),
    ];
    let records = capture_log::records();
    let would_create: Vec<_> = records
        .iter()
        .filter(|record| record.message.starts_with("would create"))
        .map(|record| (record.level, record.message.clone()))
        .collect();
    assert_eq!(
        would_create,
        expected.map(|message| (Level::Info, message)),
        "{records:?}"
    );
}
