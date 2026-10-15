//! Seed transcription (#507): every static C++ scenario is a seed naming its
//! origin, or is listed in `seeds/UNTRANSCRIBED.md` with the reason it could
//! not be one, and the fault seeds are provoked purely by their tree.
//!
//! The C++ suites under `<workspace>/tests/*.cpp` are the inventory these
//! tests check against. When the C++ tree is deleted (the spec's last slice),
//! the inventory checks go with it; the seeds and the list stay.

use std::collections::BTreeMap;
use std::path::Path;

use cao_parity::cases::{FAULT_FOLDER, Seed, seed_files, seeds_dir};

/// The folder of the hand-written seeds that predate transcription.
const HAND_WRITTEN: &str = "hand-written";

/// One C++ QtTest suite: its slots and its source text.
struct Suite {
    slots: Vec<String>,
    source: String,
}

/// Every C++ QtTest suite, by name, with its test slots: the functions under
/// `private slots:`, without `initTestCase`, `cleanupTestCase`, `init`,
/// `cleanup` and the `_data` row tables.
fn cpp_suites() -> BTreeMap<String, Suite> {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests");
    let mut suites = BTreeMap::new();
    for entry in std::fs::read_dir(&tests).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "cpp") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let slots = test_slots(&source);
        assert!(
            !slots.is_empty(),
            "{} declares no test slots",
            path.display()
        );
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        suites.insert(name, Suite { slots, source });
    }
    assert!(suites.len() >= 21, "found only {} suites", suites.len());
    suites
}

/// The test slots a QtTest class declares under `private slots:`.
fn test_slots(source: &str) -> Vec<String> {
    const FIXTURE_SLOTS: [&str; 4] = ["initTestCase", "cleanupTestCase", "init", "cleanup"];
    let mut slots = Vec::new();
    let mut inside = false;
    for raw in source.lines() {
        let line = raw.trim();
        if line == "private slots:" || line == "private Q_SLOTS:" {
            inside = true;
            continue;
        }
        // The class ends at a `};` in column 0; a slot defined inline may
        // hold indented `};` of its own.
        if inside && (raw.trim_end() == "};" || line == "private:" || line == "public:") {
            break;
        }
        // Declared (`void name();`) or defined inline (`void name() {`).
        let Some(name) = inside
            .then(|| line.strip_prefix("void "))
            .flatten()
            .and_then(|rest| rest.split_once("()"))
            .map(|(name, _)| name.trim())
            .filter(|name| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        else {
            continue;
        };
        if !FIXTURE_SLOTS.contains(&name) && !name.ends_with("_data") {
            slots.push(name.to_owned());
        }
    }
    slots
}

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

/// Panics unless `text` names a slot of a real suite and, when it names a
/// row, that row's name appears as a string literal in the suite's source.
fn check_scenario(suites: &BTreeMap<String, Suite>, text: &str, context: &str) {
    let (suite, slot, row) =
        parse_scenario(text).unwrap_or_else(|| panic!("{context}: `{text}` is not a scenario"));
    let known = suites
        .get(suite)
        .unwrap_or_else(|| panic!("{context}: no C++ suite `{suite}`"));
    assert!(
        known.slots.iter().any(|known| known == slot),
        "{context}: `{suite}` has no test slot `{slot}`"
    );
    if let Some(row) = row {
        assert!(
            row_names(&known.source, suite, slot)
                .iter()
                .any(|name| row_matches(name, row)),
            "{context}: `{suite}::{slot}_data` has no row named \"{row}\""
        );
    }
}

/// The string literals of a slot's `_data` function: its row names among
/// them, or templates such as `format-%1-contained` for rows a loop
/// generates. Rows named from a list (`for (problem : {"invalid", ...})`)
/// are found the same way.
fn row_names<'s>(source: &'s str, suite: &str, slot: &str) -> Vec<&'s str> {
    let header = format!("void {suite}::{slot}_data()");
    let Some(start) = source.find(&header) else {
        return Vec::new();
    };
    // The definition ends at the first `}` in column 0.
    let body = &source[start..];
    let end = body.find("\n}").unwrap_or(body.len());
    body[..end].split('"').skip(1).step_by(2).collect()
}

/// Whether `row` is the row name `name`, or fills its one `%1` with digits.
fn row_matches(name: &str, row: &str) -> bool {
    match name.split_once("%1") {
        None => name == row,
        Some((head, tail)) => row
            .strip_prefix(head)
            .and_then(|rest| rest.strip_suffix(tail))
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())),
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

/// The inventory parser finds every slot: 411 across the 21 suites at the
/// time of transcription, counted independently of this parser. A miscount
/// would let a scenario escape the coverage check below.
#[test]
fn the_inventory_finds_every_cpp_test_slot() {
    let suites = cpp_suites();
    let total: usize = suites.values().map(|suite| suite.slots.len()).sum();
    assert_eq!((suites.len(), total), (21, 411));
    assert_eq!(
        suites["CliConsoleInterruptTests"].slots,
        ["consoleInterruptsRemainCooperative"]
    );
}

#[test]
fn every_cpp_scenario_is_a_seed_or_on_the_untranscribed_list() {
    let suites = cpp_suites();
    let seeds = seeds();
    let listed = untranscribed();
    let mut missing = Vec::new();
    for (suite, known) in &suites {
        for slot in &known.slots {
            let names =
                |text: &str| parse_scenario(text).is_some_and(|(s, t, _)| s == suite && t == slot);
            let seeded = seeds
                .iter()
                .any(|seed| seed.case.origin.iter().any(|origin| names(origin)));
            let untranscribed = listed.iter().any(|entry| names(&entry.scenario));
            if !seeded && !untranscribed {
                missing.push(format!("{suite}::{slot}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} C++ scenarios are neither a seed nor untranscribed:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn every_untranscribed_entry_names_a_real_scenario_and_gives_its_reason() {
    let suites = cpp_suites();
    let listed = untranscribed();
    assert!(!listed.is_empty());
    let mut seen = Vec::new();
    for entry in &listed {
        check_scenario(&suites, &entry.scenario, "UNTRANSCRIBED.md");
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
    let suites = cpp_suites();
    let mut transcribed = 0;
    for seed in seeds() {
        let context = format!("seed `{}`", seed.id);
        if seed.folder == HAND_WRITTEN || seed.folder == FAULT_FOLDER {
            for origin in &seed.case.origin {
                check_scenario(&suites, origin, &context);
            }
            continue;
        }
        assert!(
            suites.contains_key(&seed.folder),
            "{context} sits in `{}`, which is neither a C++ suite, `{HAND_WRITTEN}` nor \
             `{FAULT_FOLDER}`",
            seed.folder
        );
        assert!(!seed.case.origin.is_empty(), "{context} names no origin");
        for origin in &seed.case.origin {
            check_scenario(&suites, origin, &context);
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
