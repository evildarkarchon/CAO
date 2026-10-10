//! The `corpus` runner (#500): the case budget, per-case outcomes, the summary
//! that counts not-run cases apart from passing ones, and the Different report
//! whose replay reproduces the Different. The builds are stand-in processes,
//! as in `case_runner.rs`: the oracle replays a captured transcript and the
//! Rust side copies facts, so a "seeded regression" is an edit to those facts.

mod common;

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cao_parity::HarnessError;
use cao_parity::case::{CaseDrivers, CaseFile, CaseLayout, CaseSpec, Side, SideResources};
use cao_parity::cases::{fixtures_dir, seed};
use cao_parity::corpus::{
    ArtifactKind, Budgeted, CaseOutcome, CorpusCase, Harness, Origin, TIME_BUDGET, artifact_kind,
    resolve_case, run_corpus, run_one, within_budget,
};
use cao_parity::generate::{GENERATOR_VERSION, generated_cases};
use cao_parity::materialise::Environment;
use cao_parity::oracle;
use cao_parity::rules::ParityRules;
use common::{CAPTURE, TempDir, shipped_profiles, transcript};

fn corpus_case(id: &str, origin: Origin) -> CorpusCase {
    CorpusCase {
        id: id.into(),
        case: seed("tracer-dry-run-textures").unwrap().unwrap(),
        origin,
    }
}

#[test]
fn generated_cases_are_trimmed_before_seeds() {
    let seeds: Vec<CorpusCase> = (0..8)
        .map(|n| corpus_case(&format!("seed-{n}"), Origin::Seed))
        .collect();
    let generated: Vec<CorpusCase> = (0..5)
        .map(|n| corpus_case(&format!("pw-{n:03}"), Origin::Generated))
        .collect();

    let budgeted = within_budget(seeds, generated, 10);
    let ids: Vec<&str> = budgeted.cases.iter().map(|case| case.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "seed-0", "seed-1", "seed-2", "seed-3", "seed-4", "seed-5", "seed-6", "seed-7",
            "pw-000", "pw-001"
        ]
    );
    assert_eq!((budgeted.trimmed_generated, budgeted.trimmed_seeds), (3, 0));

    // Seeds go only once every generated case has.
    let seeds: Vec<CorpusCase> = (0..4)
        .map(|n| corpus_case(&format!("seed-{n}"), Origin::Seed))
        .collect();
    let generated = vec![corpus_case("pw-000", Origin::Generated)];
    let budgeted = within_budget(seeds, generated, 3);
    assert_eq!(budgeted.cases.len(), 3);
    assert!(
        budgeted
            .cases
            .iter()
            .all(|case| case.origin == Origin::Seed)
    );
    assert_eq!((budgeted.trimmed_generated, budgeted.trimmed_seeds), (1, 1));
}

#[test]
fn artifacts_are_grouped_by_kind() {
    for (path, kind) in [
        ("mods/A/textures/a.DDS", ArtifactKind::Texture),
        ("mods/A/textures/a.tga", ArtifactKind::Texture),
        ("mods/A/meshes/a.nif", ArtifactKind::Mesh),
        ("mods/A/meshes/terrain/a.btr", ArtifactKind::Mesh),
        ("mods/A/meshes/a.hkx", ArtifactKind::Animation),
        ("mods/A/A.bsa", ArtifactKind::Archive),
        ("mods/A/A - Main.ba2", ArtifactKind::Archive),
        ("mods/A/A.esp", ArtifactKind::LoadingPlugin),
        ("mods/A/textures/a.dds.caobad", ArtifactKind::Leftover),
        ("mods/A/A.bsa.bak", ArtifactKind::Leftover),
        (
            "mods/A/.cao-staging/ownership.manifest",
            ArtifactKind::Leftover,
        ),
        ("mods/A/scripts/a.pex", ArtifactKind::Other),
    ] {
        assert_eq!(artifact_kind(path), kind, "{path}");
    }
}

#[test]
fn a_replayed_id_resolves_to_its_seed_or_generated_case() {
    let (id, generated) = generated_cases().into_iter().next().unwrap();
    let resolved = resolve_case(&id, None).unwrap();
    assert_eq!(resolved.case, generated);
    assert_eq!(resolved.warning, None);

    // A case kept by another generator version is rebuilt by this one, with
    // a warning that its bytes may differ.
    let mut other_version = generated.clone();
    other_version.generator_version = Some(GENERATOR_VERSION + 1);
    let resolved = resolve_case(&id, Some(other_version)).unwrap();
    assert_eq!(resolved.case, generated);
    let warning = resolved.warning.unwrap();
    assert!(warning.contains("may differ"), "{warning}");

    let seed_case = seed("tracer-dry-run-textures").unwrap().unwrap();
    assert_eq!(
        resolve_case("tracer-dry-run-textures", None).unwrap().case,
        seed_case
    );

    // Neither a seed nor generated: only a kept case.json can replay it.
    assert!(resolve_case("pw-999-sse-om-apply", None).is_err());
    let kept = resolve_case("pw-999-sse-om-apply", Some(seed_case.clone())).unwrap();
    assert_eq!(kept.case, seed_case);
}

/// Stand-in builds for any case: the oracle replays the `dry_run_textures`
/// transcript as if run from the case's `oracle/`, and the Rust side writes
/// the same facts from `rust/`, edited by `regress` for the case ids it names.
struct StandInDrivers {
    scratch: PathBuf,
    regress: &'static [&'static str],
}

/// The transcript as the oracle would print it from `root`.
fn transcript_at(root: &Path) -> String {
    transcript("dry_run_textures").replace(
        &format!("{CAPTURE}/dry_run_textures"),
        &root.to_str().unwrap().replace('\\', "/"),
    )
}

impl CaseDrivers for StandInDrivers {
    fn oracle(&self, layout: &CaseLayout, _spec: &CaseSpec) -> Result<Command, HarnessError> {
        let path = self.scratch.join(format!("{}.oracle.txt", layout.id()));
        std::fs::write(&path, transcript_at(&layout.side(Side::Oracle))).unwrap();
        let mut command = Command::new("cmd");
        command
            .raw_arg(format!("/c type \"{}\" & exit /b 1", path.display()))
            .current_dir(layout.side(Side::Oracle));
        Ok(command)
    }

    fn rust(&self, layout: &CaseLayout) -> Result<Command, HarnessError> {
        let mut text = transcript_at(&layout.side(Side::Rust));
        if self.regress.contains(&layout.id()) {
            // The seeded regression: the Rust side claims a cancellation.
            text = text.replace("Cancellation Observed|no", "Cancellation Observed|yes");
        }
        let facts = oracle::parse(text.as_bytes(), 1).unwrap();
        let path = self.scratch.join(format!("{}.rust.json", layout.id()));
        std::fs::write(&path, serde_json::to_vec(&facts).unwrap()).unwrap();
        let mut command = Command::new("cmd");
        command
            .raw_arg(format!(
                "/c echo rust side & copy /y \"{}\" \"{}\" >nul",
                path.display(),
                layout.rust_facts().display()
            ))
            .current_dir(layout.side(Side::Rust));
        Ok(command)
    }
}

/// A small case: one Texture and one text file in a One Mod Root.
fn small_case() -> CaseFile {
    seed("tracer-dry-run-textures").unwrap().unwrap()
}

fn replay(id: &str) -> String {
    format!("cao-parity case {id} --work \"C:/parity\" --oracle \"C:/oracle.exe\"")
}

#[test]
fn a_corpus_counts_passing_different_and_not_run_cases_apart() {
    let temp = TempDir::new("corpus-run");
    let work = temp.path().join("work");
    let scratch = temp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let profiles = shipped_profiles();
    let drivers = StandInDrivers {
        scratch,
        regress: &["regressed"],
    };
    let harness = Harness {
        work: &work,
        environment: Environment {
            resources: SideResources {
                profiles: &profiles,
                hkxcmd: None,
            },
            fixtures: &fixtures_dir(),
            symlink_rights: false,
        },
        drivers: &drivers,
        rules: &ParityRules,
        timeout: Duration::from_secs(60),
        time_budget: TIME_BUDGET,
        replay: &replay,
    };
    let mut animations = small_case();
    animations.spec.profile = "SSE".into();
    animations.spec.animations = true;
    let cases = vec![
        CorpusCase {
            id: "passing".into(),
            case: small_case(),
            origin: Origin::Seed,
        },
        CorpusCase {
            id: "regressed".into(),
            case: small_case(),
            origin: Origin::Generated,
        },
        CorpusCase {
            id: "needs-hkxcmd".into(),
            case: animations,
            origin: Origin::Generated,
        },
    ];
    let budgeted = Budgeted {
        cases: cases.clone(),
        trimmed_generated: 4,
        trimmed_seeds: 0,
    };

    let mut seen = Vec::new();
    let summary = run_corpus(&harness, &budgeted, &mut |id, outcome| {
        seen.push((id.to_owned(), outcome.name()));
    });

    assert_eq!(
        seen,
        [
            ("passing".to_owned(), "passed"),
            ("regressed".to_owned(), "Different"),
            ("needs-hkxcmd".to_owned(), "not run"),
        ]
    );
    assert_eq!(summary.passed(), 1);
    assert_eq!(summary.different(), 1);
    assert_eq!(summary.not_run(), 1);
    assert_eq!(summary.harness_errors(), 0);
    assert_eq!(summary.exit_code(), 1, "a Different fails the corpus");

    let text = summary.render();
    assert!(
        text.contains(&format!("generator version {GENERATOR_VERSION}")),
        "{text}"
    );
    assert!(text.contains("Passed: 1"), "{text}");
    assert!(
        text.contains("250-case budget: 4 generated, 0 seeds"),
        "{text}"
    );
    assert!(text.contains("Not run: 1"), "{text}");
    assert!(
        text.contains("needs-hkxcmd") && text.contains("no hkxcmd.exe was found"),
        "the reason a case did not run: {text}"
    );
    // Identical, Equivalent and Different per kind. The tracer's nine
    // Textures are untouched on both sides of the two cases that ran.
    let row = |label: &str| -> Vec<String> {
        let line = text
            .lines()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no `{label}` row: {text}"));
        line.trim_start()[label.len()..]
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(row("Texture"), ["18", "0", "0"], "{text}");
    assert_eq!(row("Mesh"), ["2", "0", "0"], "{text}");
    assert_eq!(row("Run facts"), ["1", "0", "1"], "{text}");

    // The passing case is deleted, the Different one kept with its report.
    assert!(!work.join("passing").exists());
    let report = std::fs::read_to_string(work.join("regressed/report.md")).unwrap();
    assert!(
        report.contains(&format!("Generator version: {GENERATOR_VERSION}")),
        "{report}"
    );
    assert!(
        report.contains("Cancellation Observed"),
        "the fact diff and its rule: {report}"
    );
    assert!(
        report.contains("rust side"),
        "the Rust capture is embedded: {report}"
    );
    assert!(
        report.contains("EVENT:|"),
        "the oracle capture is embedded: {report}"
    );
    assert!(report.contains("## Logs"), "{report}");
    assert!(
        report.contains(&replay("regressed")),
        "the exact replay command: {report}"
    );

    // Replaying the case as `cao-parity case regressed` does reproduces the
    // Different, with the same diff: the id is neither a seed nor generated,
    // so it resolves to the `case.json` the run kept.
    let layout = CaseLayout::new(&work, "regressed").unwrap();
    let resolved = resolve_case("regressed", Some(layout.read_case().unwrap())).unwrap();
    assert_eq!(resolved.case, cases[1].case);
    let replayed = run_one(&harness, "regressed", &resolved.case);
    let CaseOutcome::Different(result) = &replayed else {
        panic!(
            "the replay did not reproduce the Different: {}",
            replayed.name()
        );
    };
    assert!(!result.facts.passed());
    let replayed_report = std::fs::read_to_string(work.join("regressed/report.md")).unwrap();
    let diff = |report: &str| {
        report
            .split("## Run fact differences")
            .nth(1)
            .and_then(|rest| rest.split("## Captures").next())
            .unwrap()
            .to_owned()
    };
    assert_eq!(diff(&replayed_report), diff(&report));
}

#[test]
fn a_spent_time_budget_trims_generated_cases_but_never_seeds() {
    let temp = TempDir::new("corpus-time");
    let work = temp.path().join("work");
    let profiles = shipped_profiles();
    let drivers = StandInDrivers {
        scratch: temp.path().to_path_buf(),
        regress: &[],
    };
    let harness = Harness {
        work: &work,
        environment: Environment {
            resources: SideResources {
                profiles: &profiles,
                hkxcmd: None,
            },
            fixtures: &fixtures_dir(),
            symlink_rights: false,
        },
        drivers: &drivers,
        rules: &ParityRules,
        timeout: Duration::from_secs(60),
        // Spent before the first case starts.
        time_budget: Duration::ZERO,
        replay: &replay,
    };
    let budgeted = Budgeted {
        cases: vec![
            corpus_case("seeded", Origin::Seed),
            corpus_case("late", Origin::Generated),
        ],
        trimmed_generated: 0,
        trimmed_seeds: 0,
    };

    let mut seen = Vec::new();
    let summary = run_corpus(&harness, &budgeted, &mut |id, outcome| {
        seen.push((id.to_owned(), outcome.name()));
    });

    assert_eq!(seen, [("seeded".to_owned(), "passed")]);
    assert_eq!(summary.trimmed_for_time, 1);
    let text = summary.render();
    assert!(text.contains("Trimmed for time: 1 generated"), "{text}");
    assert!(text.contains("150-case floor"), "{text}");
}

#[test]
fn a_case_the_guard_rejects_is_a_harness_error_not_a_verdict() {
    let temp = TempDir::new("corpus-guard");
    let work = temp.path().join("work");
    let profiles = shipped_profiles();
    let drivers = StandInDrivers {
        scratch: temp.path().to_path_buf(),
        regress: &[],
    };
    let harness = Harness {
        work: &work,
        environment: Environment {
            resources: SideResources {
                profiles: &profiles,
                hkxcmd: None,
            },
            fixtures: &fixtures_dir(),
            symlink_rights: false,
        },
        drivers: &drivers,
        rules: &ParityRules,
        timeout: Duration::from_secs(60),
        time_budget: TIME_BUDGET,
        replay: &replay,
    };
    let mut triggering = small_case();
    // Odd resize targets with resizing by size off: deviation 18.
    triggering.spec.textures.resize_by_size = false;
    triggering.spec.textures.target_width = 255;
    let outcome = run_one(&harness, "triggering", &triggering);
    let CaseOutcome::HarnessError(message) = &outcome else {
        panic!("expected a harness error, got {}", outcome.name());
    };
    assert!(message.contains("deviation 18"), "{message}");
}
