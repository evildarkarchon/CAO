//! The shared normaliser and the fact comparator, driven by captured oracle
//! transcripts. The "Rust side" of each comparison is the same transcript as
//! another build would report it: its own case root, its own Run ID, and
//! whatever variation the test is about.

mod common;

use std::path::Path;

use cao_parity::HarnessError;
use cao_parity::compare::{FactRule, Verdict, compare_facts};
use cao_parity::facts::RunFacts;
use cao_parity::normalise::{NormalisedFacts, normalise};
use cao_parity::oracle;
use common::{CAPTURE, transcript};

/// Where the "Rust side" of each comparison pretends its case tree lives.
const OTHER_ROOT: &str = "D:/parity/work/case-0001/rust";

/// Normalises a transcript captured under `CAPTURE/<case>`.
fn oracle_side(case: &str, text: &str, exit_code: i32) -> NormalisedFacts {
    let facts = oracle::parse(text.as_bytes(), exit_code).unwrap();
    normalise(&facts, Path::new(&format!("{CAPTURE}/{case}"))).unwrap()
}

/// The same transcript as reported from `OTHER_ROOT` under a different Run ID.
fn other_side(case: &str, text: &str, exit_code: i32) -> NormalisedFacts {
    let moved = relocate(case, text);
    let facts = oracle::parse(moved.as_bytes(), exit_code).unwrap();
    normalise(&facts, Path::new(OTHER_ROOT)).unwrap()
}

/// Moves a transcript to `OTHER_ROOT` and gives it another Run ID.
fn relocate(case: &str, text: &str) -> String {
    let run_id = text.split('|').nth(1).unwrap().to_owned();
    text.replace(&format!("{CAPTURE}/{case}"), OTHER_ROOT)
        .replace(&run_id, "99-99-1")
}

fn differences(
    verdict: Verdict<cao_parity::compare::FactDifference>,
) -> Vec<cao_parity::compare::FactDifference> {
    match verdict {
        Verdict::Different(differences) => differences,
        other => panic!("expected Different, got {other:?}"),
    }
}

#[test]
fn the_same_run_from_another_case_root_and_run_id_is_identical() {
    for (case, exit_code) in [
        ("dry_run_textures", 1),
        ("several_mods_apply", 1),
        ("unreadable_archive", 2),
        ("fo4_mesh_conflict", 2),
        ("no_requested_work", 0),
        ("apply_archive_creation", 0),
        ("animations_without_hkxcmd", 1),
    ] {
        // apply_archive_creation was captured under its first name.
        let root = if case == "apply_archive_creation" {
            "dry_run_apply_archives"
        } else {
            case
        };
        let text = transcript(case);
        let oracle = oracle_side(root, &text, exit_code);
        let rust = other_side(root, &text, exit_code);
        assert_eq!(compare_facts(&oracle, &rust), Verdict::Identical, "{case}");
    }
}

#[test]
fn normalised_paths_are_relative_to_the_case_root() {
    let text = transcript("dry_run_textures");
    let NormalisedFacts::Started(run) = oracle_side("dry_run_textures", &text, 1) else {
        panic!("expected a started run");
    };
    assert_eq!(run.mod_roots, vec!["mods/DryMod".to_owned()]);
    assert_eq!(
        run.asset_failures[0].path,
        "mods/DryMod/textures/broken.dds"
    );
    assert_eq!(
        run.asset_failures[0].affected_path,
        "mods/DryMod/textures/broken.dds"
    );
}

#[test]
fn message_text_differences_are_equivalent_not_different() {
    let text = transcript("several_mods_apply");
    let reworded = text
        .replace("Failed to load Texture.", "The texture could not be read")
        .replace("matches a configured separator marker", "is a separator");
    let oracle = oracle_side("several_mods_apply", &text, 1);
    let rust = other_side("several_mods_apply", &reworded, 1);
    assert_eq!(compare_facts(&oracle, &rust), Verdict::Equivalent);
}

#[test]
fn multiset_order_differences_are_equivalent_not_different() {
    let text = transcript("several_mods_apply");
    // Swap the two Mod Roots, the two Asset Failures and the two diagnostics.
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    let first_two = |lines: &[&str], prefix: &str| {
        let found: Vec<usize> = (0..lines.len())
            .filter(|&index| lines[index].starts_with(prefix))
            .collect();
        assert!(found.len() >= 2, "{prefix}");
        (found[0], found[1])
    };
    for prefix in [
        "Mod Root|",
        "Asset Failure|",
        "Committed Mutations Retained|",
    ] {
        let (a, b) = first_two(&lines, prefix);
        lines.swap(a, b);
    }
    // Events must keep their sequence numbers, so swap the diagnostics' payloads.
    let find = |needle: &str| lines.iter().position(|line| line.contains(needle)).unwrap();
    let (a, b) = (find("|2|Diagnostic|"), find("|3|Diagnostic|"));
    let payload = |line: &str| line.splitn(4, '|').nth(3).unwrap().to_owned();
    let prefix = |line: &str| line.len() - payload(line).len();
    let (line_a, line_b) = (lines[a].to_owned(), lines[b].to_owned());
    let swapped_a = format!("{}{}", &line_a[..prefix(&line_a)], payload(&line_b));
    let swapped_b = format!("{}{}", &line_b[..prefix(&line_b)], payload(&line_a));
    lines[a] = &swapped_a;
    lines[b] = &swapped_b;
    let reordered = lines.concat();

    let oracle = oracle_side("several_mods_apply", &text, 1);
    let rust = other_side("several_mods_apply", &reordered, 1);
    assert_eq!(compare_facts(&oracle, &rust), Verdict::Equivalent);
}

#[test]
fn a_different_verdict_names_the_rule_and_carries_the_fact_diff() {
    let text = transcript("dry_run_textures");
    // The other build loaded the broken texture and reported success for it.
    let changed = text
        .replace("|4|4|succeeded=2|failed=2", "|4|4|succeeded=3|failed=1")
        .lines()
        .filter(|line| !line.contains("textures/broken.dds"))
        .map(|line| format!("{line}\r\n"))
        .collect::<String>();
    let oracle = oracle_side("dry_run_textures", &text, 1);
    let rust = other_side("dry_run_textures", &changed, 1);

    let differences = differences(compare_facts(&oracle, &rust));
    let rules: Vec<FactRule> = differences
        .iter()
        .map(|difference| difference.rule)
        .collect();
    assert_eq!(
        rules,
        vec![FactRule::FinalProgress, FactRule::AssetFailures]
    );

    let assets = &differences[1];
    assert_eq!(assets.rule.to_string(), "Asset Failures");
    assert_eq!(
        assets.oracle,
        vec![
            "path=mods/DryMod/textures/broken.dds operation=load_texture affected=mods/DryMod/textures/broken.dds"
                .to_owned()
        ]
    );
    assert!(assets.rust.is_empty(), "nothing is only on the Rust side");
    let report = assets.to_string();
    assert!(report.contains("Asset Failures"), "{report}");
    assert!(report.contains("textures/broken.dds"), "{report}");
}

#[test]
fn ordered_facts_are_compared_in_order() {
    let text = transcript("dry_run_textures");
    let oracle = oracle_side("dry_run_textures", &text, 1);

    let outcome = text.replace("|Completed With Failures|", "|Failed|");
    let rust = other_side("dry_run_textures", &outcome, 2);
    let differences = differences(compare_facts(&oracle, &rust));
    assert_eq!(differences[0].rule, FactRule::RunOutcome);
    assert_eq!(
        differences[0].oracle,
        vec!["CompletedWithFailures".to_owned()]
    );
    assert_eq!(differences[0].rust, vec!["Failed".to_owned()]);

    // Extracting Archives executed instead of being skipped by Dry Run.
    let phases = text.replace(
        "|3|Extracting Archives|Skipped|Dry Run",
        "|3|PROGRESS:|Extracting Archives|0|0|succeeded=0|failed=0",
    );
    let rust = other_side("dry_run_textures", &phases, 1);
    let rules: Vec<_> = differences_of(&oracle, &rust);
    assert_eq!(
        rules,
        vec![FactRule::PhaseSequence, FactRule::FinalProgress]
    );

    let cancelled = text.replace("Cancellation Observed|no", "Cancellation Observed|yes");
    let rust = other_side("dry_run_textures", &cancelled, 1);
    assert_eq!(
        differences_of(&oracle, &rust),
        vec![FactRule::CancellationObserved]
    );
}

fn differences_of(oracle: &NormalisedFacts, rust: &NormalisedFacts) -> Vec<FactRule> {
    differences(compare_facts(oracle, rust))
        .into_iter()
        .map(|difference| difference.rule)
        .collect()
}

#[test]
fn start_errors_compare_by_code() {
    let refused = |code: &str| {
        let facts = oracle::parse(format!("Start Error: {code}\r\n").as_bytes(), 2).unwrap();
        normalise(&facts, Path::new(OTHER_ROOT)).unwrap()
    };
    assert_eq!(
        compare_facts(&refused("2"), &refused("2")),
        Verdict::Identical
    );
    let different = differences(compare_facts(&refused("2"), &refused("1")));
    assert_eq!(different[0].rule, FactRule::StartError);

    let ran = oracle_side("no_requested_work", &transcript("no_requested_work"), 0);
    let different = differences(compare_facts(&refused("2"), &ran));
    assert_eq!(different[0].rule, FactRule::StartError);
}

#[test]
fn staging_names_compare_after_run_id_and_nonce_placeholders() {
    let cleanup = |run_id: &str, nonce: &str, root: &str| {
        format!(
            "EVENT:|{run_id}|1|Safety Cleanup|Indeterminate\r\n\
             EVENT:|{run_id}|2|Outcome|Completed With Failures|Final Phase|Safety Cleanup\r\n\
             Cancellation Observed|no\r\n\
             Mod Root|{root}/mods/M\r\n\
             Cleanup Failure|denied|{root}/mods/M/.cao-staging/run-{run_id}-{nonce}/archive-entry-{nonce}\r\n\
             Cleanup Failure|denied|{root}/mods/M/textures/.cao-staging-texture-{run_id}-{nonce}.dds\r\n"
        )
    };
    let side = |run_id: &str, nonce: &str, root: &str| {
        let facts = oracle::parse(cleanup(run_id, nonce, root).as_bytes(), 1).unwrap();
        normalise(&facts, Path::new(root)).unwrap()
    };
    let oracle = side(
        "12-34-0",
        "0123456789abcdef0123456789abcdef",
        "C:/w/c/oracle",
    );
    let rust = side(
        "rust-run-7",
        "fedcba9876543210fedcba9876543210",
        "C:/w/c/rust",
    );
    assert_eq!(compare_facts(&oracle, &rust), Verdict::Identical);

    let NormalisedFacts::Started(run) = oracle else {
        panic!("expected a started run")
    };
    let paths: Vec<_> = run
        .cleanup_failures
        .iter()
        .map(|failure| failure.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "mods/M/.cao-staging/run-{run-id}-{nonce}/archive-entry-{nonce}",
            "mods/M/textures/.cao-staging-texture-{run-id}-{nonce}.dds",
        ]
    );
}

#[test]
fn case_root_matching_ignores_ascii_case_and_separators_but_keeps_the_rest() {
    let text = transcript("no_requested_work");
    let facts = oracle::parse(text.as_bytes(), 0).unwrap();
    let root = format!("{CAPTURE}/no_requested_work")
        .replace('/', "\\")
        .to_lowercase();
    let NormalisedFacts::Started(run) = normalise(&facts, Path::new(&root)).unwrap() else {
        panic!("expected a started run");
    };
    assert_eq!(run.mod_roots, vec!["mods/IdleMod".to_owned()]);

    let verbatim = format!(r"\\?\{}", root);
    let NormalisedFacts::Started(run) = normalise(&facts, Path::new(&verbatim)).unwrap() else {
        panic!("expected a started run");
    };
    assert_eq!(run.mod_roots, vec!["mods/IdleMod".to_owned()]);
}

#[test]
fn a_path_outside_the_case_root_is_a_harness_error() {
    let text = transcript("no_requested_work");
    let facts = oracle::parse(text.as_bytes(), 0).unwrap();
    let error = normalise(&facts, Path::new("C:/elsewhere")).unwrap_err();
    assert!(
        matches!(error, HarnessError::PathOutsideCaseRoot { .. }),
        "{error}"
    );
    // A sibling whose name merely starts with the root's is outside it too.
    let error = normalise(&facts, Path::new(&format!("{CAPTURE}/no_requested"))).unwrap_err();
    assert!(
        matches!(error, HarnessError::PathOutsideCaseRoot { .. }),
        "{error}"
    );
}

#[test]
fn published_failures_must_match_the_terminal_failures() {
    let text = transcript("fo4_mesh_conflict");
    let dropped: String = text
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("Run Failure|"))
        .collect();
    let facts = oracle::parse(dropped.as_bytes(), 2).unwrap();
    let error = normalise(&facts, Path::new(&format!("{CAPTURE}/fo4_mesh_conflict"))).unwrap_err();
    assert!(
        matches!(error, HarnessError::InconsistentFacts(_)),
        "{error}"
    );
}

#[test]
fn rust_side_facts_round_trip_through_json() {
    let facts = oracle::parse(transcript("several_mods_apply").as_bytes(), 1).unwrap();
    let json = serde_json::to_string(&facts).unwrap();
    let back: RunFacts = serde_json::from_str(&json).unwrap();
    assert_eq!(back, facts);
}
