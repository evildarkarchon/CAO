//! The Rust driver behind `cao-parity run`: the options model it fills from a
//! case spec, and the facts it reports through the composition root, compared
//! with a captured oracle transcript of the same hand-written case. No oracle,
//! GPU or harness process is needed.

mod common;

use std::path::Path;

use cao_parity::HarnessError;
use cao_parity::case::ModSelection;
use cao_parity::cases::hand_written;
use cao_parity::compare::{Verdict, compare_facts};
use cao_parity::driver::{drive, options};
use cao_parity::normalise::normalise;
use cao_parity::oracle;
use cao_profiles::OptimizationMode;
use common::{CAPTURE, TempDir, transcript, write};

const TRACER: &str = "tracer-dry-run-textures";

/// Copies the repository's shipped `profiles/` into `app/profiles`.
fn copy_profiles(app: &Path) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles"),
        &app.join("profiles"),
    );
}

/// Every file under `root` with its bytes.
fn snapshot(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn the_options_model_is_filled_as_the_gui_fills_it_from_its_widgets() {
    let temp = TempDir::new("driver-options");
    let app = temp.path();
    copy_profiles(app);
    // A value no widget sets keeps what the profile's settings.ini says.
    write(
        app,
        "profiles/SSE/settings.ini",
        b"[General]\r\nbDebugLog=true\r\nmode=1\r\n",
    );
    let mut spec = (hand_written(TRACER).unwrap().spec)();
    spec.archives.merge_textures = true;

    let filled = options(&spec, app).unwrap();

    assert!(filled.debug_log);
    assert!(filled.dry_run);
    assert_eq!(filled.mode, OptimizationMode::SingleMod);
    assert_eq!(
        Path::new(&filled.user_path),
        app.join("mods").join("TracerMod")
    );
    assert!(filled.textures_necessary && filled.textures_compress && filled.textures_mipmaps);
    assert!(filled.textures_resize_size && !filled.textures_resize_ratio);
    assert_eq!(
        (filled.textures_target_width, filled.textures_target_height),
        (32, 32)
    );
    assert_eq!(filled.meshes_optimization_level, 0);
    assert!(filled.bsa_merge_textures && filled.bsa_compress && filled.bsa_delete_source);
}

#[test]
fn a_selection_the_builds_cannot_express_is_an_invalid_case() {
    let temp = TempDir::new("driver-invalid");
    copy_profiles(temp.path());
    let mut spec = (hand_written(TRACER).unwrap().spec)();
    spec.mod_selection = ModSelection::OneMod {
        folder: "../outside".into(),
    };
    let error = options(&spec, temp.path()).unwrap_err();
    assert!(matches!(error, HarnessError::InvalidCase(_)), "{error}");
}

#[test]
fn the_tracer_case_reports_what_the_oracle_reported_and_changes_nothing() {
    let temp = TempDir::new("driver-tracer");
    let app = temp.path().join("rust");
    let case = hand_written(TRACER).unwrap();
    (case.materialise)(&app).unwrap();
    copy_profiles(&app);
    let before = snapshot(&app);

    let rust = drive(&(case.spec)(), &app).unwrap();

    assert_eq!(snapshot(&app), before, "a Dry Run never mutates");
    let oracle_root = format!("{CAPTURE}/tracer_dry_run_textures");
    let oracle_facts = oracle::parse(transcript("tracer_dry_run_textures").as_bytes(), 1).unwrap();
    let verdict = compare_facts(
        &normalise(&oracle_facts, Path::new(&oracle_root)).unwrap(),
        &normalise(&rust, &app).unwrap(),
    );
    // Equivalent rather than Identical: the Rust side explains its load
    // failures in the service detail, where C++ left it empty.
    assert!(verdict.passed(), "{verdict:?}");
    assert_ne!(verdict, Verdict::Identical);
}
