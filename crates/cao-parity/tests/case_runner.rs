//! The per-case layout, the `CaseSpec`, oracle argv rendering and the
//! sequential case runner. The runner is driven by stand-in processes (`cmd`
//! replaying a transcript, `cmd` copying a facts file), so ordering, timeouts
//! and cleanup are exercised with real child processes but no real builds.

mod common;

use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use cao_parity::HarnessError;
use cao_parity::case::{
    ArchiveOptions, CaseDrivers, CaseFile, CaseLayout, CaseSpec, DRY_RUN_UNCHANGED, MeshOptions,
    ModSelection, Side, SideResources, TextureOptions, oracle_arguments, run_case,
};
use cao_parity::compare::Verdict;
use cao_parity::oracle;
use cao_parity::tree::DefaultRules;
use common::{CAPTURE, TempDir, transcript, write};

fn spec() -> CaseSpec {
    CaseSpec {
        profile: "SSE".into(),
        mod_selection: ModSelection::OneMod {
            folder: "mods/DryMod".into(),
        },
        dry_run: true,
        textures: TextureOptions {
            necessary: true,
            compress: true,
            mipmaps: false,
            resize_by_ratio: false,
            ratio_width: 1,
            ratio_height: 1,
            resize_by_size: true,
            target_width: 2048,
            target_height: 1024,
        },
        meshes: MeshOptions {
            level: 2,
            headparts: true,
            resave: false,
        },
        animations: false,
        archives: ArchiveOptions {
            extract: true,
            create: false,
            delete_backup: false,
            compress: true,
            create_dummies: false,
            merge_incompressible: true,
            merge_textures: false,
            delete_sources: true,
        },
    }
}

#[test]
fn the_layout_matches_the_documented_case_directory() {
    let layout = CaseLayout::new(Path::new("C:/work"), "case-0001").unwrap();
    assert_eq!(layout.root(), Path::new("C:/work/case-0001"));
    assert_eq!(layout.case_file(), Path::new("C:/work/case-0001/case.json"));
    assert_eq!(layout.input(), Path::new("C:/work/case-0001/input"));
    assert_eq!(
        layout.side(Side::Oracle),
        Path::new("C:/work/case-0001/oracle")
    );
    assert_eq!(layout.side(Side::Rust), Path::new("C:/work/case-0001/rust"));
    assert_eq!(layout.report(), Path::new("C:/work/case-0001/report.md"));

    for bad in ["", ".", "..", "a/b", "a\\b", "C:"] {
        assert!(
            CaseLayout::new(Path::new("C:/work"), bad).is_err(),
            "`{bad}`"
        );
    }
}

#[test]
fn provisioning_copies_the_input_into_both_sides_with_private_resources() {
    let temp = TempDir::new("provision");
    let layout = CaseLayout::new(temp.path(), "case-1").unwrap();
    write(
        &layout.input(),
        "mods/DryMod/textures/a.dds",
        b"\x00\x01texture\r\n",
    );
    std::fs::create_dir_all(layout.input().join("mods/DryMod/empty")).unwrap();
    let profiles = temp.path().join("profile-source");
    write(&profiles, "SSE/profile.ini", b"[BSA]\r\nbsaGame=4\r\n");
    write(&profiles, "common.ini", b"[General]\r\n");
    let hkxcmd = temp.path().join("hkxcmd.exe");
    std::fs::write(&hkxcmd, b"MZ").unwrap();

    layout
        .provision(&SideResources {
            profiles: &profiles,
            hkxcmd: Some(&hkxcmd),
        })
        .unwrap();

    for side in [Side::Oracle, Side::Rust] {
        let root = layout.side(side);
        assert_eq!(
            std::fs::read(root.join("mods/DryMod/textures/a.dds")).unwrap(),
            b"\x00\x01texture\r\n",
            "{side:?}: same Mod Root names, bytes copied exactly"
        );
        assert!(root.join("mods/DryMod/empty").is_dir());
        assert_eq!(
            std::fs::read(root.join("profiles/SSE/profile.ini")).unwrap(),
            b"[BSA]\r\nbsaGame=4\r\n"
        );
        assert_eq!(std::fs::read(root.join("bin/hkxcmd.exe")).unwrap(), b"MZ");
        assert!(root.join("logs").is_dir());
    }
    // Private copies: a write on one side never reaches the other.
    std::fs::write(
        layout.side(Side::Oracle).join("profiles/common.ini"),
        b"changed",
    )
    .unwrap();
    assert_eq!(
        std::fs::read(layout.side(Side::Rust).join("profiles/common.ini")).unwrap(),
        b"[General]\r\n"
    );
}

#[test]
fn provisioning_without_hkxcmd_leaves_bin_empty_and_rejects_harness_names_in_the_input() {
    let temp = TempDir::new("provision-reject");
    let layout = CaseLayout::new(temp.path(), "case-2").unwrap();
    write(&layout.input(), "mods/M/a.txt", b"a");
    let profiles = temp.path().join("profiles-source");
    write(&profiles, "common.ini", b"");
    layout
        .provision(&SideResources {
            profiles: &profiles,
            hkxcmd: None,
        })
        .unwrap();
    assert!(!layout.side(Side::Rust).join("bin/hkxcmd.exe").exists());

    let layout = CaseLayout::new(temp.path(), "case-3").unwrap();
    write(&layout.input(), "logs/oops.txt", b"a");
    let error = layout
        .provision(&SideResources {
            profiles: &profiles,
            hkxcmd: None,
        })
        .unwrap_err();
    assert!(matches!(error, HarnessError::InvalidCase(_)), "{error}");
}

#[test]
fn a_spec_renders_into_the_oracle_command_line() {
    let root = Path::new("C:/work/case-1/oracle");
    let arguments = oracle_arguments(&spec(), root).unwrap();
    let expected: Vec<OsString> = [
        root.join("mods").join("DryMod").into_os_string(),
        "om".into(),
        "SSE".into(),
    ]
    .into_iter()
    .chain(
        [
            "--dr", "--m", "2", "--mh", "--t0", "--t1", "--trrw", "1", "--trrh", "1", "--trs",
            "--trsw", "2048", "--trsh", "1024", "--be", "--bcomp", "1", "--bdum", "0", "--bmi",
            "1", "--bmt", "0", "--bds", "1",
        ]
        .map(OsString::from),
    )
    .collect();
    assert_eq!(arguments, expected);

    let mut several = spec();
    several.mod_selection = ModSelection::SeveralMods {
        folder: "mods".into(),
    };
    several.dry_run = false;
    let arguments = oracle_arguments(&several, root).unwrap();
    assert_eq!(arguments[1], "sm");
    assert!(!arguments.contains(&OsString::from("--dr")));
}

#[test]
fn a_spec_the_oracle_cannot_express_is_a_harness_error_at_render_time() {
    let root = Path::new("C:/work/case-1/oracle");
    let mut cases: Vec<(&str, CaseSpec)> = Vec::new();
    for selection in [
        "",
        "/mods/A",
        "C:/mods",
        "mods/../A",
        "mods\\A",
        "mods//A",
        "profiles/A",
        "logs",
    ] {
        let mut spec = spec();
        spec.mod_selection = ModSelection::OneMod {
            folder: selection.into(),
        };
        cases.push(("selection", spec));
    }
    for profile in ["", "../SSE", "a/b"] {
        let mut spec = spec();
        spec.profile = profile.into();
        cases.push(("profile", spec));
    }
    let mut spec4 = spec();
    spec4.meshes.level = 4;
    cases.push(("mesh level", spec4));

    for (what, spec) in cases {
        let error = oracle_arguments(&spec, root).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidCase(_)),
            "{what} {spec:?}: {error}"
        );
    }
}

#[test]
fn case_json_holds_the_spec_and_rejects_unknown_fields() {
    let file = CaseFile::new(spec());
    let json = serde_json::to_string_pretty(&file).unwrap();
    assert_eq!(serde_json::from_str::<CaseFile>(&json).unwrap(), file);
    assert!(json.contains("\"kind\": \"one_mod\""), "{json}");

    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["spec"]["textures"]["sharpen"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<CaseFile>(value).is_err());
}

/// The oracle transcript used by the runner tests, as the oracle would print
/// it from `root`.
fn transcript_at(root: &Path) -> String {
    transcript("dry_run_textures").replace(
        &format!("{CAPTURE}/dry_run_textures"),
        &root.to_str().unwrap().replace('\\', "/"),
    )
}

/// Stand-in drivers: the oracle replays a transcript and the Rust driver
/// copies a prepared facts file, but only once the oracle's capture exists.
struct FakeDrivers {
    transcript: std::path::PathBuf,
    exit_code: i32,
    facts: std::path::PathBuf,
}

impl CaseDrivers for FakeDrivers {
    fn oracle(&self, layout: &CaseLayout, _spec: &CaseSpec) -> Result<Command, HarnessError> {
        let mut command = Command::new("cmd");
        command
            .raw_arg(format!(
                "/c type \"{}\" & exit /b {}",
                self.transcript.display(),
                self.exit_code
            ))
            .current_dir(layout.side(Side::Oracle));
        Ok(command)
    }

    fn rust(&self, layout: &CaseLayout) -> Result<Command, HarnessError> {
        let mut command = Command::new("cmd");
        command
            .raw_arg(format!(
                "/c if exist \"{}\" (copy /y \"{}\" \"{}\") else (exit /b 9)",
                layout.stdout(Side::Oracle).display(),
                self.facts.display(),
                layout.rust_facts().display()
            ))
            .current_dir(layout.side(Side::Rust));
        Ok(command)
    }
}

/// Prepares a provisioned case whose Rust facts are the transcript as the Rust
/// side would report it, edited by `edit_rust`.
fn prepared_case(temp: &TempDir, edit_rust: fn(String) -> String) -> (CaseLayout, FakeDrivers) {
    let layout = CaseLayout::new(&temp.path().join("work"), "case-7").unwrap();
    write(
        &layout.input(),
        "mods/DryMod/textures/broken.dds",
        b"not a texture",
    );
    let profiles = temp.path().join("profiles-source");
    write(&profiles, "common.ini", b"");
    layout.write_case(&CaseFile::new(spec())).unwrap();
    layout
        .provision(&SideResources {
            profiles: &profiles,
            hkxcmd: None,
        })
        .unwrap();

    let transcript = temp.path().join("oracle.transcript");
    std::fs::write(&transcript, transcript_at(&layout.side(Side::Oracle))).unwrap();
    let rust_text = edit_rust(transcript_at(&layout.side(Side::Rust)));
    let rust_facts = oracle::parse(rust_text.as_bytes(), 1).unwrap();
    let facts = temp.path().join("rust.facts.fixture.json");
    std::fs::write(&facts, serde_json::to_vec(&rust_facts).unwrap()).unwrap();
    (
        layout,
        FakeDrivers {
            transcript,
            exit_code: 1,
            facts,
        },
    )
}

#[test]
fn a_passing_case_runs_the_oracle_first_and_is_deleted() {
    let temp = TempDir::new("run-pass");
    let (layout, drivers) = prepared_case(&temp, |text| text);
    let result = run_case(&layout, &drivers, &DefaultRules, Duration::from_secs(60)).unwrap();
    assert_eq!(result.facts, Verdict::Identical);
    assert_eq!(result.tree, Verdict::Identical);
    assert!(result.passed());
    assert!(!layout.root().exists(), "passing cases are deleted");
}

#[test]
fn a_different_case_is_kept_with_a_report_naming_the_broken_rule() {
    let temp = TempDir::new("run-different");
    let (layout, drivers) = prepared_case(&temp, |text| {
        text.replace("Cancellation Observed|no", "Cancellation Observed|yes")
    });
    std::fs::write(layout.side(Side::Rust).join("mods/DryMod/extra.txt"), b"x").unwrap();

    let result = run_case(&layout, &drivers, &DefaultRules, Duration::from_secs(60)).unwrap();
    assert!(!result.passed());
    let report = std::fs::read_to_string(layout.report()).unwrap();
    assert!(report.contains("Cancellation Observed"), "{report}");
    assert!(report.contains("mods/DryMod/extra.txt"), "{report}");
    assert!(report.contains("Same Relative Paths"), "{report}");
    assert!(
        report.contains("cao-parity case case-7"),
        "the replay command: {report}"
    );
    assert!(layout.stdout(Side::Oracle).exists(), "captures are kept");
}

#[test]
fn a_harness_error_keeps_the_case_with_a_report() {
    let temp = TempDir::new("run-harness-error");
    let (layout, mut drivers) = prepared_case(&temp, |text| text);
    drivers.exit_code = 0; // Contradicts the transcript's Completed With Failures.
    let error = run_case(&layout, &drivers, &DefaultRules, Duration::from_secs(60)).unwrap_err();
    assert!(
        matches!(error, HarnessError::ExitCodeMismatch { .. }),
        "{error}"
    );
    let report = std::fs::read_to_string(layout.report()).unwrap();
    assert!(report.contains("Harness error"), "{report}");
}

/// An oracle that never finishes within the timeout.
struct HangingOracle(FakeDrivers);

impl CaseDrivers for HangingOracle {
    fn oracle(&self, layout: &CaseLayout, _spec: &CaseSpec) -> Result<Command, HarnessError> {
        // ping is the process itself, so killing it on timeout leaves nothing behind.
        let mut command = Command::new("ping");
        command
            .args(["-n", "30", "127.0.0.1"])
            .current_dir(layout.side(Side::Oracle));
        Ok(command)
    }

    fn rust(&self, layout: &CaseLayout) -> Result<Command, HarnessError> {
        self.0.rust(layout)
    }
}

#[test]
fn a_side_past_the_timeout_is_killed_and_is_a_harness_error() {
    let temp = TempDir::new("run-timeout");
    let (layout, drivers) = prepared_case(&temp, |text| text);
    let started = std::time::Instant::now();
    let error = run_case(
        &layout,
        &HangingOracle(drivers),
        &DefaultRules,
        Duration::from_secs(1),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            HarnessError::Timeout {
                side: Side::Oracle,
                ..
            }
        ),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the oracle was killed, not awaited"
    );
    assert!(layout.report().exists());
}

#[test]
fn a_dry_run_that_both_builds_mutate_alike_is_still_different() {
    let temp = TempDir::new("run-dry-run-mutated");
    let (layout, drivers) = prepared_case(&temp, |text| text);
    // The same change on both sides: comparing the sides alone would pass.
    for side in [Side::Oracle, Side::Rust] {
        std::fs::write(
            layout.side(side).join("mods/DryMod/textures/broken.dds"),
            b"rewritten",
        )
        .unwrap();
    }

    let result = run_case(&layout, &drivers, &DefaultRules, Duration::from_secs(60)).unwrap();

    assert_eq!(result.facts, Verdict::Identical);
    let Verdict::Different(differences) = &result.tree else {
        panic!("expected Different, got {:?}", result.tree);
    };
    let paths: Vec<_> = differences
        .iter()
        .map(|difference| (difference.path.as_str(), difference.rule))
        .collect();
    assert_eq!(
        paths,
        [
            ("oracle/mods/DryMod/textures/broken.dds", DRY_RUN_UNCHANGED),
            ("rust/mods/DryMod/textures/broken.dds", DRY_RUN_UNCHANGED),
        ]
    );
    let report = std::fs::read_to_string(layout.report()).unwrap();
    assert!(report.contains(DRY_RUN_UNCHANGED), "{report}");
}

#[test]
fn an_apply_case_may_change_the_input_when_both_builds_agree() {
    let temp = TempDir::new("run-apply-mutated");
    let (layout, drivers) = prepared_case(&temp, |text| text);
    let mut apply = spec();
    apply.dry_run = false;
    layout.write_case(&CaseFile::new(apply)).unwrap();
    for side in [Side::Oracle, Side::Rust] {
        std::fs::write(
            layout.side(side).join("mods/DryMod/textures/broken.dds"),
            b"rewritten",
        )
        .unwrap();
    }

    let result = run_case(&layout, &drivers, &DefaultRules, Duration::from_secs(60)).unwrap();

    assert!(result.passed(), "{result:?}");
}
