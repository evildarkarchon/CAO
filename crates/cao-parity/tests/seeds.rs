//! Seed transcription (#507): every static C++ scenario is a seed naming its
//! origin, or is listed in `seeds/UNTRANSCRIBED.md` with the reason it could
//! not be one, and the fault seeds are provoked purely by their tree.
//!
//! The C++ suites under `<workspace>/tests/*.cpp` were the inventory these
//! tests checked against: that every test slot was a seed or on the list,
//! and that every origin named a real slot and row. Those inventory checks
//! passed for the last time before the C++ tree was deleted (#515) and went
//! with it. The seeds and the list stay, so the checks below hold them to
//! their shape without the C++ sources: each scenario reference is well
//! formed and names one of the suites that existed, each entry gives its
//! reason, and a seed sits in its origin's suite folder.

use cao_parity::cases::{FAULT_FOLDER, Seed, seed_files, seeds_dir};

/// The folder of the hand-written seeds that predate transcription.
const HAND_WRITTEN: &str = "hand-written";

/// The 21 C++ QtTest suites, as the C++ tree held them when it was deleted.
/// A scenario reference must name one of them; the slots and rows they held
/// can no longer be checked.
const CPP_SUITES: [&str; 21] = [
    "ApplicationLoggingTests",
    "ApplicationRunSetupTests",
    "ArchiveFinalizationTests",
    "ArchiveFirstAssetDiscoveryTests",
    "AssetExecutionTests",
    "AssetRoutingTests",
    "AssetRunTests",
    "CliConsoleInterruptTests",
    "CliExecutionTests",
    "CliRunTests",
    "DurableStagingTests",
    "GuiRunDispatchTests",
    "GuiRunTests",
    "MainOptimizerTests",
    "MeshReferenceMaintenanceTests",
    "OptimizationRunServiceTests",
    "OptionsCAOTests",
    "RunEventDeliveryTests",
    "RunEvidenceTests",
    "RunExecutorTests",
    "RunSetupTests",
];

/// A scenario reference, `Suite::slot` or `Suite::slot [row]`, split into its
/// suite, slot and row.
fn parse_scenario(text: &str) -> Option<(&str, &str, Option<&str>)> {
    let (suite, rest) = text.split_once("::")?;
    let (slot, row) = match rest.split_once(" [") {
        Some((slot, row)) => (slot, Some(row.strip_suffix(']')?)),
        None => (rest, None),
    };
    Some((suite, slot, row))
}

/// Panics unless `text` is a well-formed scenario reference whose suite is
/// one of [`CPP_SUITES`], with a slot that is a C++ identifier and, when it
/// names a row, a non-empty row name.
fn check_scenario(text: &str, context: &str) {
    let (suite, slot, row) =
        parse_scenario(text).unwrap_or_else(|| panic!("{context}: `{text}` is not a scenario"));
    assert!(
        CPP_SUITES.contains(&suite),
        "{context}: no C++ suite `{suite}`"
    );
    assert!(
        !slot.is_empty() && slot.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "{context}: `{slot}` is not a test slot name"
    );
    if let Some(row) = row {
        assert!(!row.is_empty(), "{context}: `{text}` names an empty row");
    }
}

/// One entry of `seeds/UNTRANSCRIBED.md`: the scenario and its reason.
struct Untranscribed {
    scenario: String,
    reason: String,
}

/// The entries of `seeds/UNTRANSCRIBED.md`: every line of the form
/// `` - `Suite::slot`: reason `` or `` - `Suite::slot [row]`: reason ``.
fn untranscribed() -> Vec<Untranscribed> {
    let path = seeds_dir().join("UNTRANSCRIBED.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .map(|rest| {
            let (scenario, reason) = rest
                .split_once('`')
                .unwrap_or_else(|| panic!("UNTRANSCRIBED.md: unclosed entry `{rest}`"));
            let reason = reason
                .strip_prefix(": ")
                .unwrap_or_else(|| panic!("UNTRANSCRIBED.md: `{scenario}` gives no reason"));
            Untranscribed {
                scenario: scenario.to_owned(),
                reason: reason.trim().to_owned(),
            }
        })
        .collect()
}

fn seeds() -> Vec<Seed> {
    seed_files().unwrap()
}

/// A misspelt suite in a reference is caught even without the C++ sources.
#[test]
#[should_panic(expected = "no C++ suite `RunExecutorTest`")]
fn a_scenario_naming_no_cpp_suite_is_rejected() {
    check_scenario("RunExecutorTest::cancellation", "probe");
}

#[test]
fn every_untranscribed_entry_names_a_scenario_and_gives_its_reason() {
    let listed = untranscribed();
    assert!(!listed.is_empty());
    let mut seen = Vec::new();
    for entry in &listed {
        check_scenario(&entry.scenario, "UNTRANSCRIBED.md");
        assert!(
            entry.reason.len() > 10,
            "UNTRANSCRIBED.md: `{}` needs a real reason, not `{}`",
            entry.scenario,
            entry.reason
        );
        assert!(
            !seen.contains(&entry.scenario),
            "UNTRANSCRIBED.md lists `{}` twice",
            entry.scenario
        );
        seen.push(entry.scenario.clone());
    }
}

#[test]
fn a_transcribed_seed_sits_in_its_suites_folder_and_names_its_origin() {
    let mut transcribed = 0;
    for seed in seeds() {
        let context = format!("seed `{}`", seed.id);
        if seed.folder == HAND_WRITTEN || seed.folder == FAULT_FOLDER {
            for origin in &seed.case.origin {
                check_scenario(origin, &context);
            }
            continue;
        }
        assert!(
            CPP_SUITES.contains(&seed.folder.as_str()),
            "{context} sits in `{}`, which is neither a C++ suite, `{HAND_WRITTEN}` nor \
             `{FAULT_FOLDER}`",
            seed.folder
        );
        assert!(!seed.case.origin.is_empty(), "{context} names no origin");
        for origin in &seed.case.origin {
            check_scenario(origin, &context);
        }
        let (suite, _, _) = parse_scenario(&seed.case.origin[0]).unwrap();
        assert_eq!(
            suite, seed.folder,
            "{context}: its first origin is in another suite than its folder"
        );
        transcribed += 1;
    }
    assert!(transcribed > 0, "no C++ scenario is transcribed yet");
}

/// The reason that marks the untranscribed remainder of a seeded scenario.
const PARTLY: &str = "Partly transcribed";

/// A scenario is either a seed or untranscribed, unless its entry says it is
/// only partly transcribed: then a seed stands for it, and the entry names
/// the part that needs a Rust test. Conversely, a "partly transcribed" entry
/// must have the seed it qualifies.
#[test]
fn a_scenario_is_never_both_a_seed_and_untranscribed_unless_partly_transcribed() {
    let seeds = seeds();
    let seeded = |scenario: &str| {
        seeds.iter().find(|seed| {
            seed.case.origin.iter().any(|origin| {
                origin == scenario
                    || parse_scenario(origin)
                        .is_some_and(|(s, t, _)| format!("{s}::{t}") == scenario)
            })
        })
    };
    for entry in untranscribed() {
        let seed = seeded(&entry.scenario);
        if entry.reason.starts_with(PARTLY) {
            assert!(
                seed.is_some(),
                "`{}` is partly transcribed, yet no seed transcribes it",
                entry.scenario
            );
        } else {
            // One row may be untranscribed while a sibling row is a seed, so
            // only an exact match is a contradiction here.
            let exact = seeds
                .iter()
                .find(|seed| seed.case.origin.contains(&entry.scenario));
            assert!(
                exact.is_none(),
                "`{}` is untranscribed, yet seed `{}` transcribes it",
                entry.scenario,
                exact.map(|seed| seed.id.as_str()).unwrap_or_default()
            );
        }
    }
}

/// The budget trims generated cases before seeds (#473), so every seed must
/// fit in it with room to spare for the generated cases.
#[test]
fn every_seed_fits_the_corpus_budget() {
    let budgeted = cao_parity::corpus::corpus_cases().unwrap();
    let seeds = seeds().len();
    assert_eq!(budgeted.trimmed_seeds, 0);
    assert!(
        seeds < cao_parity::corpus::MAX_CASES,
        "{seeds} seeds leave no room for generated cases"
    );
}

/// The fault seed families (#473), by the id prefix their seeds share.
const FAULT_FAMILIES: [&str; 7] = [
    "fault-preparing-",
    "fault-staging-residue-",
    "fault-archive-preflight-",
    "fault-collision-",
    "fault-extraction-",
    "fault-loose-",
    "fault-finalization-",
];

#[test]
fn every_fault_family_has_seeds_provoked_purely_by_their_tree() {
    let faults: Vec<Seed> = seeds()
        .into_iter()
        .filter(|seed| seed.folder == FAULT_FOLDER)
        .collect();
    for family in FAULT_FAMILIES {
        assert!(
            faults.iter().any(|seed| seed.id.starts_with(family)),
            "no fault seed in the `{family}` family"
        );
    }
    for seed in &faults {
        assert!(
            FAULT_FAMILIES
                .iter()
                .any(|family| seed.id.starts_with(family)),
            "fault seed `{}` belongs to no family",
            seed.id
        );
        // A fault seed's only lever is its tree: no profile value differs from
        // the shipped profile's, and it stands for no C++ scenario, whose
        // faults were injected through seams the oracle does not have.
        assert!(
            seed.case.profile_overrides.is_empty(),
            "fault seed `{}` overrides the profile",
            seed.id
        );
        assert!(
            seed.case.origin.is_empty(),
            "fault seed `{}` names a C++ origin",
            seed.id
        );
        assert!(
            !seed.case.tree.content.is_empty() || !seed.case.tree.fs_shape.is_empty(),
            "fault seed `{}` has no tree to provoke its fault",
            seed.id
        );
    }
}
