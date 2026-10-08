//! The oracle parser and its mapping tables, pinned against transcripts captured
//! from the C++ oracle (`docs/parity-oracle.md`).
//!
//! The files under `tests/transcripts/` are the oracle's stdout, byte for byte
//! (CRLF included), from runs over small generated mod trees. Their absolute
//! paths are the capture machine's; the parser keeps them verbatim.

mod common;

use cao_parity::HarnessError;
use cao_parity::facts::*;
use cao_parity::oracle;
use common::{CAPTURE, transcript};

fn path(case: &str, rest: &str) -> String {
    format!("{CAPTURE}/{case}/{rest}")
}

fn started(facts: RunFacts) -> StartedRun {
    match facts {
        RunFacts::Started(run) => run,
        RunFacts::StartError(error) => panic!("expected a started run, got {error:?}"),
    }
}

fn phase(sequence: u64, phase: RunPhase, status: PhaseStatus) -> RunEventFact {
    RunEventFact {
        sequence,
        payload: RunEventPayload::Phase { phase, status },
    }
}

fn progress(completed: u64, total: u64, succeeded: u64, failed: u64) -> PhaseStatus {
    PhaseStatus::Progress(Progress {
        completed,
        total,
        succeeded,
        failed,
    })
}

/// Builds a synthetic transcript with the oracle's CRLF terminators, for
/// records no captured run produced.
fn synthetic(records: &[&str]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|record| format!("{record}\r\n").into_bytes())
        .collect()
}

/// A minimal Succeeded run, in the oracle's shape.
const SUCCEEDED: [&str; 4] = [
    "EVENT:|7-7-0|1|Preparing|Indeterminate",
    "EVENT:|7-7-0|2|Safety Cleanup|Indeterminate",
    "EVENT:|7-7-0|3|Outcome|Succeeded|Final Phase|Preparing",
    "Cancellation Observed|no",
];

fn with(records: &[&str], extra: &[&str]) -> Vec<u8> {
    let all: Vec<&str> = records.iter().chain(extra).copied().collect();
    synthetic(&all)
}

#[test]
fn dry_run_transcript_parses_into_its_events_and_terminal_details() {
    let run = started(oracle::parse(transcript("dry_run_textures").as_bytes(), 1).unwrap());
    let case = "dry_run_textures";

    assert_eq!(run.run_id, "4164818643-1268160434-0");
    use PhaseStatus::{Indeterminate, Skipped};
    use RunPhase::*;
    assert_eq!(
        run.events,
        vec![
            phase(1, Preparing, Indeterminate),
            phase(2, DiscoveringArchives, Indeterminate),
            phase(3, ExtractingArchives, Skipped(PhaseSkipReason::DryRun)),
            phase(4, BuildingEffectiveAssetTree, Indeterminate),
            phase(5, ProcessingAssets, progress(0, 4, 0, 0)),
            phase(6, ProcessingAssets, progress(1, 4, 0, 1)),
            phase(7, ProcessingAssets, progress(2, 4, 1, 1)),
            phase(8, ProcessingAssets, progress(3, 4, 2, 1)),
            phase(9, ProcessingAssets, progress(4, 4, 2, 2)),
            phase(10, ArchiveFinalization, Skipped(PhaseSkipReason::DryRun)),
            phase(11, SafetyCleanup, Indeterminate),
        ]
    );
    assert_eq!(
        run.terminal,
        TerminalFacts {
            outcome: RunOutcome::CompletedWithFailures,
            final_phase: ArchiveFinalization,
            cancellation_observed: false,
            mod_roots: vec![path(case, "mods/DryMod")],
            run_failures: vec![],
            cleanup_failures: vec![],
            asset_failures: vec![
                AssetFailure {
                    path: path(case, "mods/DryMod/textures/broken.dds"),
                    operation: "load_texture".into(),
                    message: "Failed to load Texture.".into(),
                    affected_path: path(case, "mods/DryMod/textures/broken.dds"),
                    service_detail: String::new(),
                },
                AssetFailure {
                    path: path(case, "mods/DryMod/meshes/thing.nif"),
                    operation: "load_mesh".into(),
                    message: "Failed to load Mesh.".into(),
                    affected_path: path(case, "mods/DryMod/meshes/thing.nif"),
                    service_detail: String::new(),
                },
            ],
            archive_failures: vec![],
            finalization_failure: None,
            committed_mutations: vec![],
            archive_collisions: vec![],
            skipped_assets: vec![],
        }
    );
}

#[test]
fn several_mods_transcript_keeps_diagnostics_and_committed_mutations() {
    let run = started(oracle::parse(transcript("several_mods_apply").as_bytes(), 1).unwrap());
    let case = "several_mods_apply";

    let diagnostics: Vec<_> = run
        .events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::Diagnostic { .. }))
        .cloned()
        .collect();
    assert_eq!(
        diagnostics,
        vec![
            RunEventFact {
                sequence: 2,
                payload: RunEventPayload::Diagnostic {
                    phase: RunPhase::Preparing,
                    detail: "The child Mod Root matches a configured separator marker".into(),
                    path: path(case, "mods/Group_separator"),
                },
            },
            RunEventFact {
                sequence: 3,
                payload: RunEventPayload::Diagnostic {
                    phase: RunPhase::Preparing,
                    detail: "The child Mod Root matches an ignored-mod name".into(),
                    path: path(case, "mods/Nemesis"),
                },
            },
        ]
    );
    assert_eq!(run.events.len(), 16, "every event before the terminal one");
    assert_eq!(
        run.terminal.mod_roots,
        vec![path(case, "mods/Alpha"), path(case, "mods/Beta")]
    );
    let mutation = |root: &str, kind, committed| CommittedMutations {
        mod_root: path(case, root),
        kind,
        committed,
        partial_or_unknown: 0,
    };
    assert_eq!(
        run.terminal.committed_mutations,
        vec![
            mutation("mods/Alpha", MutationKind::AssetProcessing, 2),
            mutation("mods/Alpha", MutationKind::ArchiveFinalization, 2),
            mutation("mods/Beta", MutationKind::AssetProcessing, 2),
            mutation("mods/Beta", MutationKind::ArchiveFinalization, 2),
        ]
    );
}

#[test]
fn failed_run_transcripts_map_their_failure_codes() {
    let run = started(oracle::parse(transcript("unreadable_archive").as_bytes(), 2).unwrap());
    let archive = path("unreadable_archive", "mods/ArcMod/ArcMod.bsa");
    assert_eq!(
        run.events[2].payload,
        RunEventPayload::Failure {
            phase: RunPhase::DiscoveringArchives,
            code: RunFailureCode::ArchiveUnreadable,
            detail: "unsupported version".into(),
            path: archive.clone(),
        }
    );
    assert_eq!(run.terminal.outcome, RunOutcome::Failed);
    assert_eq!(run.terminal.final_phase, RunPhase::DiscoveringArchives);
    assert_eq!(
        run.terminal.run_failures,
        vec![DetailedPath {
            detail: "unsupported version".into(),
            path: archive,
        }]
    );

    let run = started(oracle::parse(transcript("fo4_mesh_conflict").as_bytes(), 2).unwrap());
    assert_eq!(
        run.events[1].payload,
        RunEventPayload::Failure {
            phase: RunPhase::Preparing,
            code: RunFailureCode::PolicyConflict,
            detail: "The loaded profile conflicts with the requested Routing Policy".into(),
            path: String::new(),
        }
    );
    assert!(run.terminal.mod_roots.is_empty());
}

#[test]
fn skip_records_map_through_their_tables() {
    let run = started(oracle::parse(transcript("no_requested_work").as_bytes(), 0).unwrap());
    let skipped: Vec<_> = run
        .events
        .iter()
        .filter_map(|event| match event.payload {
            RunEventPayload::Phase {
                phase,
                status: PhaseStatus::Skipped(reason),
            } => Some((phase, reason)),
            _ => None,
        })
        .collect();
    use RunPhase::*;
    let none = PhaseSkipReason::NoRequestedWork;
    assert_eq!(
        skipped,
        vec![
            (DiscoveringArchives, none),
            (ExtractingArchives, none),
            (BuildingEffectiveAssetTree, none),
            (ProcessingAssets, none),
            (ArchiveFinalization, none),
        ]
    );

    // Meshes were present but not requested: Skip Reason 1.
    let run = started(oracle::parse(transcript("apply_archive_creation").as_bytes(), 0).unwrap());
    assert_eq!(
        run.terminal.skipped_assets,
        vec![SkippedAssets {
            reason: SkipReason::DisabledAssetKind,
            count: 2,
        }]
    );
    assert_eq!(run.terminal.outcome, RunOutcome::Succeeded);
}

#[test]
fn an_exit_code_that_contradicts_the_outcome_is_a_harness_error() {
    let error = oracle::parse(transcript("dry_run_textures").as_bytes(), 0).unwrap_err();
    assert!(
        matches!(
            error,
            HarnessError::ExitCodeMismatch {
                expected: 1,
                actual: 0
            }
        ),
        "{error}"
    );
    assert!(oracle::parse(&synthetic(&["Start Error: 2"]), 1).is_err());
}

#[test]
fn a_start_error_is_its_own_fact() {
    assert_eq!(
        oracle::parse(&synthetic(&["Start Error: 2"]), 2).unwrap(),
        RunFacts::StartError(StartError::ActiveRun)
    );
}

/// Every name and code position the tables read, each replaced by a value the
/// oracle never prints.
#[test]
fn an_unknown_enum_name_or_code_is_a_harness_error_not_a_verdict() {
    let cases: [(&[&str], &str); 7] = [
        (&["EVENT:|7-7-0|1|Unknown Phase|Indeterminate"], "Run Phase"),
        (
            &["EVENT:|7-7-0|1|Preparing|Skipped|Because"],
            "Phase Skip Reason",
        ),
        (
            &["EVENT:|7-7-0|1|Outcome|Partly|Final Phase|Preparing"],
            "Run Outcome",
        ),
        (
            &[
                "EVENT:|7-7-0|1|Outcome|Succeeded|Final Phase|Preparing",
                "Cancellation Observed|no",
                "Committed Mutations Retained|C:/m|Unknown Mutation|1|partial-or-unknown=0",
            ],
            "Mutation Kind",
        ),
        (
            &[
                "EVENT:|7-7-0|1|Outcome|Succeeded|Final Phase|Preparing",
                "Cancellation Observed|no",
                "Surprise Detail|x",
            ],
            "terminal detail",
        ),
        (
            &["EVENT:|7-7-0|1|Failure|Preparing|19|detail|"],
            "Run Failure Code",
        ),
        (
            &[
                "EVENT:|7-7-0|1|Outcome|Succeeded|Final Phase|Preparing",
                "Cancellation Observed|no",
                "Skipped Assets|3|1",
            ],
            "Skip Reason",
        ),
    ];
    for (records, table) in cases {
        let error = oracle::parse(&synthetic(records), 0).unwrap_err();
        let found = match &error {
            HarnessError::UnknownName { table, .. } | HarnessError::UnknownCode { table, .. } => {
                *table
            }
            other => panic!("{records:?}: expected an unknown-value error, got {other}"),
        };
        assert_eq!(found, table, "{records:?}");
    }
    let error = oracle::parse(&synthetic(&["Start Error: 3"]), 2).unwrap_err();
    assert!(matches!(
        error,
        HarnessError::UnknownCode {
            table: "Start Error",
            code: 3,
            ..
        }
    ));
}

#[test]
fn escaped_text_fields_are_unescaped_after_splitting() {
    let facts = oracle::parse(
        &with(&SUCCEEDED, &[r"Run Failure|a\pb\\c\r\nd|C:/m/x\py"]),
        0,
    )
    .unwrap();
    assert_eq!(
        started(facts).terminal.run_failures,
        vec![DetailedPath {
            detail: "a|b\\c\r\nd".into(),
            path: "C:/m/x|y".into(),
        }]
    );

    for bad in [r"Run Failure|a\qb|", r"Run Failure|trailing\|"] {
        let error = oracle::parse(&with(&SUCCEEDED, &[bad]), 0).unwrap_err();
        assert!(
            matches!(error, HarnessError::Transcript { line: 5, .. }),
            "{bad}: {error}"
        );
    }
}

#[test]
fn late_diagnostics_after_the_terminal_event_are_kept() {
    let run = started(
        oracle::parse(
            &with(
                &SUCCEEDED,
                &["EVENT:|7-7-0|4|Diagnostic|Safety Cleanup|The dispatcher failed|"],
            ),
            0,
        )
        .unwrap(),
    );
    assert_eq!(
        run.events.last().unwrap(),
        &RunEventFact {
            sequence: 4,
            payload: RunEventPayload::Diagnostic {
                phase: RunPhase::SafetyCleanup,
                detail: "The dispatcher failed".into(),
                path: String::new(),
            },
        }
    );

    // Only diagnostics may be late, and terminal details cannot resume after one.
    for late in [
        &["EVENT:|7-7-0|4|Preparing|Indeterminate"][..],
        &["EVENT:|7-7-0|4|Diagnostic|Preparing|d|", "Mod Root|C:/m"][..],
    ] {
        assert!(
            oracle::parse(&with(&SUCCEEDED, late), 0).is_err(),
            "{late:?}"
        );
    }
}

#[test]
fn cancellation_and_collisions_parse_from_terminal_details() {
    let run = started(
        oracle::parse(
            &synthetic(&[
                "EVENT:|a-b|1|Preparing|Indeterminate",
                "EVENT:|a-b|2|Outcome|Cancelled|Final Phase|Processing Assets",
                "Cancellation Observed|yes",
                "Cleanup Failure|denied|C:/m/.cao-staging/run-a-b-0123/x",
                "Finalization Failure|plan failed",
                "Archive Failure|C:/m/m.bsa|write failed",
                "Archive Collision|C:/m|textures/a.dds|winner=C:/m/b.bsa|loose-asset-wins=no|shadowed=C:/m/a.bsa|shadowed=C:/m/c.bsa",
            ]),
            130,
        )
        .unwrap(),
    );
    let terminal = run.terminal;
    assert!(terminal.cancellation_observed);
    assert_eq!(terminal.outcome, RunOutcome::Cancelled);
    assert_eq!(
        terminal.archive_collisions,
        vec![ArchiveCollision {
            mod_root: "C:/m".into(),
            game_path: "textures/a.dds".into(),
            winning_archive: "C:/m/b.bsa".into(),
            loose_asset_wins: false,
            shadowed_archives: vec!["C:/m/a.bsa".into(), "C:/m/c.bsa".into()],
        }]
    );
    assert_eq!(
        terminal.cleanup_failures[0].path,
        "C:/m/.cao-staging/run-a-b-0123/x"
    );
    assert_eq!(
        terminal.finalization_failure.as_deref(),
        Some("plan failed")
    );
    assert_eq!(terminal.archive_failures[0].archive_path, "C:/m/m.bsa");
}

#[test]
fn malformed_streams_are_harness_errors() {
    let malformed: [(&str, Vec<u8>); 8] = [
        ("empty stdout", Vec::new()),
        (
            "cut-off final record",
            b"EVENT:|7-7-0|1|Preparing|Indeterminate".to_vec(),
        ),
        ("no terminal event", synthetic(&SUCCEEDED[..2])),
        ("missing Cancellation Observed", synthetic(&SUCCEEDED[..3])),
        (
            "sequence gap",
            synthetic(&[
                "EVENT:|7-7-0|1|Preparing|Indeterminate",
                "EVENT:|7-7-0|3|Outcome|Succeeded|Final Phase|Preparing",
                "Cancellation Observed|no",
            ]),
        ),
        (
            "second Run ID",
            synthetic(&[
                "EVENT:|7-7-0|1|Preparing|Indeterminate",
                "EVENT:|8-8-0|2|Outcome|Succeeded|Final Phase|Preparing",
                "Cancellation Observed|no",
            ]),
        ),
        (
            "detail before the terminal event",
            synthetic(&["Mod Root|C:/m"]),
        ),
        ("extra field", with(&SUCCEEDED, &["Mod Root|C:/m|extra"])),
    ];
    for (what, stdout) in malformed {
        let error = oracle::parse(&stdout, 0).unwrap_err();
        assert!(
            matches!(error, HarnessError::Transcript { .. }),
            "{what}: {error}"
        );
    }
}

#[test]
fn terminal_details_must_keep_their_rendering_order() {
    // C++ prints Cancellation Observed first, then each category in a fixed order.
    let out_of_order: [&[&str]; 3] = [
        &["Mod Root|C:/m", "Cancellation Observed|no"],
        &[
            "Cancellation Observed|no",
            "Skipped Assets|1|2",
            "Mod Root|C:/m",
        ],
        &[
            "Cancellation Observed|no",
            "Asset Failure|C:/m/a|op|msg|C:/m/a|",
            "Run Failure|d|",
        ],
    ];
    for details in out_of_order {
        let mut records = vec!["EVENT:|7-7-0|1|Outcome|Succeeded|Final Phase|Preparing"];
        records.extend_from_slice(details);
        let error = oracle::parse(&synthetic(&records), 0).unwrap_err();
        assert!(
            matches!(error, HarnessError::Transcript { .. }),
            "{details:?}: {error}"
        );
    }

    // Extraction failures, the Finalization Failure and finalization's own
    // Archive Failures interleave as C++ prints them.
    let in_order = with(
        &SUCCEEDED,
        &[
            "Archive Failure|C:/m/a.bsa|extract",
            "Finalization Failure|plan",
            "Archive Failure|C:/m/b.bsa|write",
        ],
    );
    assert_eq!(
        started(oracle::parse(&in_order, 0).unwrap())
            .terminal
            .archive_failures
            .len(),
        2
    );
}
