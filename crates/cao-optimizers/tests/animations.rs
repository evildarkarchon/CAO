//! Animations through the production wiring (#502): `hkxcmd.exe convert <src>
//! -o <dst> -v AMD64` as a subprocess, resolved from the app directory's `bin/`.
//!
//! This binary is its own fake `hkxcmd.exe`. Each scenario copies it into an
//! install's `bin/`, and when it is started with `convert` as its first
//! argument it behaves as the converter instead of running the scenarios. What
//! it does depends on the source's bytes (see [`fake_hkxcmd`]), and it records
//! every call in `calls.txt` beside itself, so a scenario can tell which copy
//! ran and with what arguments.
//!
//! It has no libtest harness (`harness = false`) because libtest would reject
//! the converter's arguments before any code here ran. For the same reason the
//! whole binary runs from one decoy working directory with its own
//! `bin/hkxcmd.exe`: a converter resolved against the working directory would
//! leave a `calls.txt` there (deviation 1).

mod common;

use std::ffi::OsString;
use std::io::Write as _;
use std::os::windows::fs::OpenOptionsExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cao_core::execution::{AssetExecutionFailure, MutationState};
use cao_core::run::{OptimizationRunResult, RunOutcome};
use cao_optimizers::animations::{AnimationError, Hkxcmd};
use cao_optimizers::composition::ApplicationRun;
use cao_profiles::Options;
use common::{
    STAGING_CONTROL_FILES, app_dir, profile_options, scratch_dir, snapshot_tree, staging_leftovers,
    write,
};

/// Each scenario, by name; the arguments after `--` filter them by substring.
const SCENARIOS: &[(&str, fn())] = &[
    (
        "an_apply_converts_animations_with_the_app_directory_hkxcmd",
        an_apply_converts_animations_with_the_app_directory_hkxcmd,
    ),
    (
        "without_an_app_directory_hkxcmd_each_animation_fails_even_with_one_in_the_working_directory",
        without_an_app_directory_hkxcmd_each_animation_fails_even_with_one_in_the_working_directory,
    ),
    (
        "a_failed_conversion_is_an_asset_failure_and_the_run_carries_on",
        a_failed_conversion_is_an_asset_failure_and_the_run_carries_on,
    ),
    (
        "a_dry_run_evaluates_animations_without_running_hkxcmd",
        a_dry_run_evaluates_animations_without_running_hkxcmd,
    ),
    (
        "a_converter_that_does_not_finish_is_stopped_before_convert_returns",
        a_converter_that_does_not_finish_is_stopped_before_convert_returns,
    ),
    (
        "conversion_requires_an_empty_hkx_staging_file_apart_from_the_source",
        conversion_requires_an_empty_hkx_staging_file_apart_from_the_source,
    ),
];

/// Acts as the fake converter when started with `convert`; otherwise runs the
/// scenarios from the decoy working directory and fails if any panicked.
///
/// Arguments starting with `-` are ignored, so `cargo test` flags pass
/// through harmlessly; every other argument is a name filter. A flag's
/// separate value (as in `--test-threads 1`) therefore counts as a filter.
fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|first| first == "convert") {
        return fake_hkxcmd(&args[1..]);
    }

    std::env::set_current_dir(decoy_working_directory()).unwrap();
    let filters: Vec<String> = args[1..]
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let mut failed = Vec::new();
    for (name, scenario) in SCENARIOS {
        if !filters.is_empty() && !filters.iter().any(|filter| name.contains(filter.as_str())) {
            continue;
        }
        println!("test {name} ...");
        // Scenarios run one after another, so one active run per process holds.
        if catch_unwind(AssertUnwindSafe(scenario)).is_err() {
            println!("test {name} ... FAILED");
            failed.push(*name);
        } else {
            println!("test {name} ... ok");
        }
    }
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        println!("failures: {failed:?}");
        ExitCode::FAILURE
    }
}

/// The fake converter, given `convert <src> -o <dst> -v AMD64`.
///
/// By the source's bytes: `exit-failure` exits 3; `not-loadable` and
/// `failed-to-save` write output but print an error hkxcmd reports while still
/// exiting 0; `no-output` exits 0 without writing; `hang` holds the output open
/// with no sharing, then sleeps for a minute. Anything else is converted:
/// the output is `AMD64:` and the source's bytes.
fn fake_hkxcmd(args: &[OsString]) -> ExitCode {
    let exe = std::env::current_exe().unwrap();
    let line: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let mut calls = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(exe.with_file_name("calls.txt"))
        .unwrap();
    writeln!(calls, "{}", line.join("|")).unwrap();

    let [_, source, dash_o, output, dash_v, amd64] = args else {
        eprintln!("usage: hkxcmd convert <src> -o <dst> -v AMD64");
        return ExitCode::from(2);
    };
    if dash_o != "-o" || dash_v != "-v" || amd64 != "AMD64" {
        eprintln!("unexpected arguments");
        return ExitCode::from(2);
    }
    let input = std::fs::read(source).unwrap();
    let convert = || {
        let mut bytes = b"AMD64:".to_vec();
        bytes.extend_from_slice(&input);
        std::fs::write(output, bytes).unwrap();
    };
    match input.as_slice() {
        b"exit-failure" => ExitCode::from(3),
        b"not-loadable" => {
            convert();
            println!("File '{}' is not loadable", Path::new(source).display());
            ExitCode::SUCCESS
        }
        b"failed-to-save" => {
            convert();
            eprintln!("Failed to save file '{}'", Path::new(output).display());
            ExitCode::SUCCESS
        }
        b"no-output" => ExitCode::SUCCESS,
        b"hang" => {
            let _held = std::fs::OpenOptions::new()
                .write(true)
                .share_mode(0)
                .open(output)
                .unwrap();
            std::thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        _ => {
            // hkxcmd prints its progress to stderr.
            eprintln!("Converting '{}' ...", Path::new(source).display());
            convert();
            ExitCode::SUCCESS
        }
    }
}

/// The working directory of the whole binary: an install whose own
/// `bin/hkxcmd.exe` would record a call if anything resolved against it.
fn decoy_working_directory() -> PathBuf {
    let decoy = scratch_dir("animations-decoy");
    install_fake_hkxcmd(&decoy);
    decoy
}

/// Copies this binary to `<install>/bin/hkxcmd.exe`.
fn install_fake_hkxcmd(install: &Path) -> PathBuf {
    let exe = install.join("bin").join("hkxcmd.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    exe
}

/// The calls the fake at `<install>/bin/hkxcmd.exe` recorded, one argument
/// list each; none when it never ran.
fn calls(install: &Path) -> Vec<Vec<String>> {
    match std::fs::read_to_string(install.join("bin").join("calls.txt")) {
        Ok(text) => text
            .lines()
            .map(|line| line.split('|').map(str::to_owned).collect())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("{error}"),
    }
}

/// The decoy working directory, which no scenario may ever run.
fn assert_decoy_untouched() {
    let decoy = std::env::current_dir().unwrap();
    assert_eq!(calls(&decoy), Vec::<Vec<String>>::new(), "the decoy ran");
}

/// SSE options over `mod_root` with only Animation work.
fn animations_only(app: &Path, mod_root: &Path, dry_run: bool) -> Options {
    let mut options = profile_options(app, "SSE");
    options.dry_run = dry_run;
    options.user_path = mod_root.to_string_lossy().into_owned();
    options.textures_necessary = false;
    options.textures_compress = false;
    options.textures_mipmaps = false;
    options.textures_resize_ratio = false;
    options.textures_resize_size = false;
    options.meshes_optimization_level = 0;
    options.meshes_resave = false;
    options.animations_optimization = true;
    options.bsa_extract = false;
    options.bsa_create = false;
    options
}

/// Runs `options` under SSE in the install at `app` and waits for the result.
fn run(app: &Path, options: &Options) -> Arc<OptimizationRunResult> {
    let run = ApplicationRun::new(app, "SSE", options).unwrap();
    run.start(None).unwrap().wait()
}

/// Spec (#476, #502): an Apply runs `hkxcmd.exe convert <src> -o <dst> -v
/// AMD64` from the app directory's `bin/`, never the working directory's,
/// with absolute native paths and the output staged beside the source, then
/// publishes the converted bytes over the source as a Committed Mutation.
fn an_apply_converts_animations_with_the_app_directory_hkxcmd() {
    let app = app_dir("animations-apply");
    install_fake_hkxcmd(&app);
    let mod_root = app.join("mods").join("Mod");
    write(&mod_root, "meshes/actors/walk.hkx", b"walk");
    write(&mod_root, "meshes/actors/run.hkx", b"run");
    write(&mod_root, "readme.txt", b"untouched");

    let result = run(&app, &animations_only(&app, &mod_root, false));

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.asset_attempts()
    );
    for name in ["walk", "run"] {
        let path = mod_root.join(format!("meshes/actors/{name}.hkx"));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            format!("AMD64:{name}").as_bytes()
        );
    }
    assert_eq!(
        std::fs::read(mod_root.join("readme.txt")).unwrap(),
        b"untouched"
    );
    for attempt in result.asset_attempts() {
        assert_eq!(attempt.result.mutation_state(), MutationState::Committed);
    }
    assert_eq!(result.mutation_summaries()[0].committed, 2);

    let mut calls = calls(&app);
    calls.sort();
    assert_eq!(calls.len(), 2, "{calls:?}");
    for call in &calls {
        let [convert, source, dash_o, output, dash_v, amd64] = call.as_slice() else {
            panic!("{call:?}");
        };
        assert_eq!(
            (
                convert.as_str(),
                dash_o.as_str(),
                dash_v.as_str(),
                amd64.as_str()
            ),
            ("convert", "-o", "-v", "AMD64")
        );
        let (source, output) = (Path::new(source), Path::new(output));
        assert!(source.is_absolute() && output.is_absolute(), "{call:?}");
        assert!(!call[1].contains('/') && !call[3].contains('/'), "{call:?}");
        assert_eq!(
            source.parent(),
            Some(mod_root.join("meshes/actors").as_path())
        );
        assert_eq!(output.parent(), source.parent(), "staged beside its source");
        assert_ne!(output, source);
        assert!(call[3].to_ascii_lowercase().ends_with(".hkx"), "{call:?}");
    }
    assert_eq!(staging_leftovers(&mod_root), STAGING_CONTROL_FILES);
    assert_decoy_untouched();
}

/// Spec (#502, deviation 1): with no `bin/hkxcmd.exe` in the app directory,
/// each Animation is an Asset Failure that mutated nothing, even though the
/// working directory has one, and the run carries on to the next Animation.
fn without_an_app_directory_hkxcmd_each_animation_fails_even_with_one_in_the_working_directory() {
    let app = app_dir("animations-missing-hkxcmd");
    let mod_root = app.join("mods").join("Mod");
    write(&mod_root, "meshes/walk.hkx", b"walk");
    write(&mod_root, "meshes/run.hkx", b"run");
    let before = snapshot_tree(&mod_root);

    let result = run(&app, &animations_only(&app, &mod_root, false));

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert_eq!(result.asset_attempts().len(), 2);
    for attempt in result.asset_attempts() {
        let failure = &attempt.result;
        assert_eq!(
            failure.failure(),
            Some(AssetExecutionFailure::OperationFailed)
        );
        assert_eq!(failure.mutation_state(), MutationState::None);
        assert!(failure.safe_to_continue());
        assert_eq!(failure.operation(), "optimize_animation");
        let expected = app.join("bin").join("hkxcmd.exe");
        assert!(
            failure
                .service_detail()
                .contains(&expected.display().to_string()),
            "{}",
            failure.service_detail()
        );
    }
    let mut after = snapshot_tree(&mod_root);
    after.retain(|path, _| !path.starts_with(".cao-staging"));
    assert_eq!(after, before);
    assert_decoy_untouched();
}

/// C++ `AnimationsOptimizer::convert`: a non-zero exit, a log line reporting a
/// load or save failure despite a zero exit, or no output file each make the
/// Animation an Asset Failure that leaves its source as it was. The staged
/// output is never published, and the other Animations are still converted.
fn a_failed_conversion_is_an_asset_failure_and_the_run_carries_on() {
    let app = app_dir("animations-failures");
    install_fake_hkxcmd(&app);
    let mod_root = app.join("mods").join("Mod");
    let failing = [
        "exit-failure",
        "not-loadable",
        "failed-to-save",
        "no-output",
    ];
    for marker in failing {
        write(
            &mod_root,
            &format!("meshes/{marker}.hkx"),
            marker.as_bytes(),
        );
    }
    write(&mod_root, "meshes/good.hkx", b"good");

    let result = run(&app, &animations_only(&app, &mod_root, false));

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert_eq!(
        std::fs::read(mod_root.join("meshes/good.hkx")).unwrap(),
        b"AMD64:good"
    );
    for marker in failing {
        assert_eq!(
            std::fs::read(mod_root.join(format!("meshes/{marker}.hkx"))).unwrap(),
            marker.as_bytes(),
            "{marker}"
        );
    }
    let mut failed: Vec<String> = result
        .asset_attempts()
        .iter()
        .filter(|attempt| !attempt.result.succeeded())
        .map(|attempt| {
            assert_eq!(attempt.result.mutation_state(), MutationState::None);
            assert!(attempt.result.safe_to_continue());
            let name = attempt.result.affected_path().file_stem().unwrap();
            name.to_string_lossy().into_owned()
        })
        .collect();
    failed.sort();
    let mut expected: Vec<String> = failing.iter().map(|marker| (*marker).to_owned()).collect();
    expected.sort();
    assert_eq!(failed, expected);
    assert_eq!(calls(&app).len(), 5);
    assert_eq!(staging_leftovers(&mod_root), STAGING_CONTROL_FILES);
    assert_decoy_untouched();
}

/// C++ `MainOptimizer::optimizeAnimation`: a Dry Run reports that each
/// Animation would be converted, without starting `hkxcmd` or touching the
/// tree.
fn a_dry_run_evaluates_animations_without_running_hkxcmd() {
    let app = app_dir("animations-dry-run");
    install_fake_hkxcmd(&app);
    let mod_root = app.join("mods").join("Mod");
    write(&mod_root, "meshes/walk.hkx", b"walk");
    let before = snapshot_tree(&mod_root);

    let result = run(&app, &animations_only(&app, &mod_root, true));

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.asset_attempts().len(), 1);
    assert!(result.asset_attempts()[0].result.succeeded());
    assert_eq!(snapshot_tree(&mod_root), before);
    assert_eq!(calls(&app), Vec::<Vec<String>>::new());
    assert_decoy_untouched();
}

/// C++ killed a converter that had not finished and waited for it before
/// returning, so that cleanup never races a writer. Here the fake holds the
/// output open with no sharing; deleting it straight after `convert` returns
/// proves the process is gone.
fn a_converter_that_does_not_finish_is_stopped_before_convert_returns() {
    let install = scratch_dir("animations-timeout");
    let exe = install_fake_hkxcmd(&install);
    write(&install, "walk.hkx", b"hang");
    write(&install, "staged.hkx", b"");
    let mut hkxcmd = Hkxcmd::new(exe).with_timeout(Duration::from_millis(500));

    let started = Instant::now();
    let error = hkxcmd
        .convert(&install.join("walk.hkx"), &install.join("staged.hkx"))
        .unwrap_err();

    assert!(matches!(error, AnimationError::Timeout { .. }), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    std::fs::remove_file(install.join("staged.hkx")).unwrap();
}

/// C++ `AnimationsOptimizer::convert`'s precondition: the output must be an
/// empty, registered `.hkx` staging file that is not the source itself, or the
/// converter never starts.
fn conversion_requires_an_empty_hkx_staging_file_apart_from_the_source() {
    let install = scratch_dir("animations-staging-checks");
    let exe = install_fake_hkxcmd(&install);
    write(&install, "walk.hkx", b"walk");
    write(&install, "written.hkx", b"already written");
    write(&install, "staged.nif", b"");
    let mut hkxcmd = Hkxcmd::new(exe);

    // An empty source passes every other check, so only its identity with
    // the output, in any case, rejects the last two.
    write(&install, "empty.hkx", b"");
    for (source, output) in [
        ("walk.hkx", "written.hkx"),
        ("walk.hkx", "staged.nif"),
        ("walk.hkx", "missing.hkx"),
        ("empty.hkx", "empty.hkx"),
        ("empty.hkx", "EMPTY.HKX"),
    ] {
        let output = install.join(output);
        let error = hkxcmd.convert(&install.join(source), &output).unwrap_err();
        assert!(
            matches!(error, AnimationError::InvalidStaging),
            "{}: {error}",
            output.display()
        );
    }
    assert_eq!(std::fs::read(install.join("walk.hkx")).unwrap(), b"walk");
    assert_eq!(calls(&install), Vec::<Vec<String>>::new());
}
