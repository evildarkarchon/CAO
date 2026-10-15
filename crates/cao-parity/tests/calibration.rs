//! `cao-parity calibrate` (#513): the BC7/BC6H PSNR threshold, calibrated
//! once by running the oracle against itself, on the GPU and with the CPU
//! codec forced.

mod common;

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ba2::prelude::*;
use ba2::tes4;
use cao_parity::HarnessError;
use cao_parity::calibrate::{
    CalibrationHost, CalibrationOutcome, CalibrationSummary, Calibrator, FORCE_CPU_VARIABLE,
    TextureMeasurement, TreeMeasurement, calibrate_case, calibrated_threshold, is_calibration_case,
    measure_trees,
};
use cao_parity::case::{CaseSpec, SideResources};
use cao_parity::cases::{fixtures_dir, seed};
use cao_parity::materialise::Environment;
use cao_parity::recipe::{ArchiveGame, ArchiveKind, ArchiveRecipe, ContentEntry};
use cao_parity::textures::ImagePsnr;
use common::{TempDir, shipped_profiles, write};
use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT, ScratchImage, TEX_COMPRESS_BC7_QUICK,
    TEX_COMPRESS_DEFAULT, TEX_COMPRESS_FLAGS, TEX_FILTER_FORCE_NON_WIC, TEX_THRESHOLD_DEFAULT,
};

const BC7: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM;
const BC3: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_BC3_UNORM;
const RGBA8: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM;

/// The case a committed seed holds.
fn seed_case(id: &str) -> cao_parity::case::CaseFile {
    seed(id)
        .unwrap()
        .unwrap_or_else(|| panic!("no seed `{id}`"))
}

/// A 16×16 RGBA8 texture with its full mip chain (five images), whose colour
/// changes along one axis so BC7 fits it closely. The chain is generated
/// without WIC: test threads never join COM.
fn source() -> ScratchImage {
    const SIZE: usize = 16;
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(RGBA8, SIZE, SIZE, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    let row_pitch = scratch.images()[0].row_pitch;
    for row in scratch.pixels_mut().chunks_exact_mut(row_pitch) {
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().take(SIZE).enumerate() {
            let x = x as u8;
            pixel.copy_from_slice(&[x * 8, x * 4, 255 - x * 8, 128 + x * 4]);
        }
    }
    scratch
        .generate_mip_maps(TEX_FILTER_FORCE_NON_WIC, 0)
        .unwrap()
}

/// `source()` encoded to `format` with `flags`, as DDS file bytes.
fn encoded(format: DXGI_FORMAT, flags: TEX_COMPRESS_FLAGS) -> Vec<u8> {
    source()
        .compress(format, flags, TEX_THRESHOLD_DEFAULT)
        .unwrap()
        .save_dds(DDS_FLAGS_NONE)
        .unwrap()
        .buffer()
        .to_vec()
}

/// Writes an uncompressed SSE BSA holding `entries` (directory, file name,
/// bytes) at `path`.
fn sse_bsa(path: &Path, entries: &[(&str, &str, &[u8])]) {
    let mut archive = tes4::Archive::new();
    for &(directory, name, bytes) in entries {
        let file = tes4::File::from_decompressed(bytes);
        let key = tes4::ArchiveKey::from(directory);
        match archive.get_mut(&key) {
            Some(existing) => {
                existing.insert(tes4::DirectoryKey::from(name), file);
            }
            None => {
                let directory: tes4::Directory = [(tes4::DirectoryKey::from(name), file)]
                    .into_iter()
                    .collect();
                archive.insert(key, directory);
            }
        }
    }
    let options = tes4::ArchiveOptions::builder()
        .version(tes4::Version::v105)
        .flags(tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS)
        .types(tes4::ArchiveTypes::empty())
        .build();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    archive
        .write(&mut std::fs::File::create(path).unwrap(), &options)
        .unwrap();
}

/// The spec's rule: 40 dB stands unless the observed floor is lower, and
/// then the threshold is the floor minus 1 dB, rounded down to 0.01 dB so it
/// never ends up above that.
#[test]
fn the_threshold_is_40_db_unless_the_floor_is_lower() {
    assert_eq!(calibrated_threshold(f64::INFINITY), 40.0);
    assert_eq!(calibrated_threshold(52.3), 40.0);
    assert_eq!(calibrated_threshold(40.0), 40.0);
    assert_eq!(calibrated_threshold(39.5), 38.5);
    assert_eq!(calibrated_threshold(31.377), 30.37);
}

/// The BC7 and BC6H cases are those that can encode a Texture: they hold one,
/// loose or packed, and apply some texture option. A Dry Run encodes nothing.
#[test]
fn calibration_runs_the_cases_that_can_encode_a_texture() {
    for id in [
        "apply-texture-bc7",
        "apply-texture-bc6h",
        "apply-loose-textures",
    ] {
        assert!(is_calibration_case(&seed_case(id)), "{id}");
    }
    for id in ["tracer-dry-run-textures", "apply-meshes-necessary"] {
        assert!(!is_calibration_case(&seed_case(id)), "{id}");
    }

    // A Texture packed in an input Archive counts too: extraction hands it to
    // the texture optimizer like a loose one.
    let mut packed = seed_case("apply-texture-bc7");
    let ContentEntry::Texture(mut texture) = packed.tree.content[0].clone() else {
        panic!("the seed starts with a texture entry");
    };
    texture.path = "textures/plain.dds".into();
    packed.tree.content = vec![ContentEntry::Archive(ArchiveRecipe {
        path: "mods/BcSevenMod/BcSevenMod - Textures.bsa".into(),
        game: ArchiveGame::Sse,
        archive_type: ArchiveKind::Textures,
        compress: true,
        content: vec![ContentEntry::Texture(texture)],
        fault: None,
        note: None,
    })];
    assert!(is_calibration_case(&packed), "a packed Texture");

    let mut untouched = seed_case("apply-texture-bc7");
    let textures = &mut untouched.spec.textures;
    textures.necessary = false;
    textures.compress = false;
    textures.mipmaps = false;
    textures.resize_by_ratio = false;
    textures.resize_by_size = false;
    assert!(!is_calibration_case(&untouched), "no texture option is on");
}

/// Measurement pairs every BC7 and BC6H Texture the two runs wrote, loose or
/// in an Archive, and measures each image the way the Texture rule does.
/// Other formats are not measured, and a difference in one is noted, since
/// only BC6H and BC7 take a different encoder on the CPU.
#[test]
fn measurement_covers_every_bc7_texture_both_runs_wrote() {
    let temp = TempDir::new("calibration-measure");
    let (gpu, cpu) = (temp.path().join("gpu"), temp.path().join("cpu"));
    let (thorough, quick) = (
        encoded(BC7, TEX_COMPRESS_DEFAULT),
        encoded(BC7, TEX_COMPRESS_BC7_QUICK),
    );
    assert_ne!(thorough, quick, "the encoders must disagree for this test");
    let (plain_bc3, dithered_bc3) = (
        encoded(BC3, TEX_COMPRESS_DEFAULT),
        encoded(BC3, directxtex::TEX_COMPRESS_DITHER),
    );
    assert_ne!(plain_bc3, dithered_bc3, "dithering must change BC3 here");

    for (root, bc7, bc3) in [(&gpu, &thorough, &plain_bc3), (&cpu, &quick, &dithered_bc3)] {
        write(root, "mods/A/textures/a.dds", bc7);
        write(root, "mods/A/textures/same.dds", &thorough);
        write(root, "mods/A/textures/b.dds", &plain_bc3);
        write(root, "mods/A/textures/dithered.dds", bc3);
        // Harness-owned folders are not output.
        write(root, "profiles/stray.dds", bc7);
        sse_bsa(
            &root.join("mods/A/A - Textures.bsa"),
            &[("textures", "packed.dds", bc7)],
        );
    }
    write(&gpu, "mods/A/textures/only-gpu.dds", &thorough);

    let measured = measure_trees(&gpu, &cpu).unwrap();

    let paths: Vec<&str> = measured
        .textures
        .iter()
        .map(|texture| texture.path.as_str())
        .collect();
    // In path order: `A - Textures.bsa` sorts before `textures/`.
    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(
        paths[0].starts_with("mods/A/A - Textures.bsa:") && paths[0].ends_with("packed.dds"),
        "{paths:?}"
    );
    assert_eq!(paths[1], "mods/A/textures/a.dds");
    assert_eq!(paths[2], "mods/A/textures/same.dds");

    let differing = &measured.textures[1];
    assert_eq!(differing.format, BC7);
    assert_eq!(differing.images.len(), 5, "16x16 has five mips");
    let floor = differing.floor();
    assert!(floor.is_finite() && floor > 20.0, "{floor}");
    assert!(differing.images.iter().all(|image| image.db >= floor));
    assert_eq!(measured.textures[2].floor(), f64::INFINITY, "same bytes");
    assert_eq!(measured.textures[0].floor(), floor, "the packed copy");
    assert_eq!(measured.floor(), Some(floor));

    assert_eq!(measured.notes.len(), 1, "{:?}", measured.notes);
    assert!(
        measured.notes[0].contains("mods/A/textures/dithered.dds"),
        "{:?}",
        measured.notes
    );
}

/// A stand-in oracle: a `cmd` line run in the side root, which writes the
/// seed's `plain.dds` from `gpu_dds` normally and from `cpu_dds` when the CPU
/// override is set. `honours_override` false ignores the variable, as an
/// oracle built before it would; `log_cpu_codec` decides when it logs the
/// CPU-codec warning C++ logs when it has no D3D11 device.
struct StandIn {
    gpu_dds: PathBuf,
    cpu_dds: PathBuf,
    honours_override: bool,
    log_cpu_codec: LogCpuCodec,
}

#[derive(Clone, Copy)]
enum LogCpuCodec {
    /// Only when the override is set, as the real oracle on a GPU host.
    WhenForced,
    /// Always, as on a host without a GPU.
    Always,
}

impl StandIn {
    fn command(&self, _root: &Path, _spec: &CaseSpec) -> Result<Command, HarnessError> {
        const TARGET: &str = r"mods\BcSevenMod\textures\plain.dds";
        let warning =
            r"echo DirectCompute is not available, using BC6H / BC7 CPU codec.>logs\CAO.html";
        let (gpu, cpu) = (self.gpu_dds.display(), self.cpu_dds.display());
        let gpu_run = match self.log_cpu_codec {
            LogCpuCodec::WhenForced => format!("copy /y \"{gpu}\" {TARGET}"),
            LogCpuCodec::Always => format!("{warning} & copy /y \"{gpu}\" {TARGET}"),
        };
        let cpu_run = format!("{warning} & copy /y \"{cpu}\" {TARGET}");
        let line = if self.honours_override {
            format!("/c if defined {FORCE_CPU_VARIABLE} ({cpu_run}) else ({gpu_run})")
        } else {
            format!("/c {gpu_run}")
        };
        let mut command = Command::new("cmd");
        command.raw_arg(line);
        Ok(command)
    }
}

/// Runs the BC7 seed through `stand_in` under a fresh work directory.
fn calibrate_seed(temp: &TempDir, stand_in: &StandIn) -> CalibrationOutcome {
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let profiles = shipped_profiles();
    let fixtures = fixtures_dir();
    let oracle = |root: &Path, spec: &CaseSpec| stand_in.command(root, spec);
    let calibrator = Calibrator {
        work: &work,
        environment: Environment {
            resources: SideResources {
                profiles: &profiles,
                hkxcmd: None,
            },
            fixtures: &fixtures,
            symlink_rights: false,
            local_assets: common::empty_pool(),
        },
        oracle: &oracle,
        timeout: Duration::from_secs(60),
    };
    calibrate_case(
        &calibrator,
        "apply-texture-bc7",
        &seed_case("apply-texture-bc7"),
    )
}

/// Writes the two encodes the stand-in hands out: BC7 from the thorough
/// encoder for the GPU run and from the quick one for the CPU run.
fn encodes(temp: &TempDir) -> (PathBuf, PathBuf) {
    let (gpu, cpu) = (temp.path().join("gpu.dds"), temp.path().join("cpu.dds"));
    std::fs::write(&gpu, encoded(BC7, TEX_COMPRESS_DEFAULT)).unwrap();
    std::fs::write(&cpu, encoded(BC7, TEX_COMPRESS_BC7_QUICK)).unwrap();
    (gpu, cpu)
}

/// The tracer: the case runs through the oracle on the GPU and with the CPU
/// codec forced, its BC7 Texture is measured, and the case directory is
/// deleted, as a passing parity case's is.
#[test]
fn a_case_runs_on_the_gpu_and_with_the_cpu_forced_and_is_measured() {
    let temp = TempDir::new("calibration-case");
    let (gpu_dds, cpu_dds) = encodes(&temp);
    let outcome = calibrate_seed(
        &temp,
        &StandIn {
            gpu_dds,
            cpu_dds,
            honours_override: true,
            log_cpu_codec: LogCpuCodec::WhenForced,
        },
    );

    let CalibrationOutcome::Measured(measured) = outcome else {
        panic!("expected a measurement, got {outcome:?}");
    };
    let paths: Vec<&str> = measured
        .textures
        .iter()
        .map(|texture| texture.path.as_str())
        .collect();
    assert_eq!(paths, ["mods/BcSevenMod/textures/plain.dds"]);
    let floor = measured.floor().unwrap();
    assert!(floor.is_finite(), "the two encodes differ: {floor}");
    assert!(
        !temp.path().join("work/apply-texture-bc7").exists(),
        "a measured case's directory is deleted"
    );
}

/// An oracle that ignores the override encodes on the GPU both times, which
/// would measure no spread at all; calibration refuses to trust it.
#[test]
fn an_oracle_that_ignores_the_cpu_override_is_an_error() {
    let temp = TempDir::new("calibration-ignored");
    let (gpu_dds, _) = encodes(&temp);
    let outcome = calibrate_seed(
        &temp,
        &StandIn {
            cpu_dds: gpu_dds.clone(),
            gpu_dds,
            honours_override: false,
            log_cpu_codec: LogCpuCodec::WhenForced,
        },
    );

    let CalibrationOutcome::Error(message) = outcome else {
        panic!("expected an error, got {outcome:?}");
    };
    assert!(message.contains(FORCE_CPU_VARIABLE), "{message}");
    assert!(
        temp.path().join("work/apply-texture-bc7").is_dir(),
        "a failed case is kept for diagnosis"
    );
}

/// On a host where the oracle gets no D3D11 device, the "GPU" run is a CPU
/// run too; calibration needs a GPU.
#[test]
fn a_gpu_run_without_a_device_is_an_error() {
    let temp = TempDir::new("calibration-no-gpu");
    let (gpu_dds, cpu_dds) = encodes(&temp);
    let outcome = calibrate_seed(
        &temp,
        &StandIn {
            gpu_dds,
            cpu_dds,
            honours_override: true,
            log_cpu_codec: LogCpuCodec::Always,
        },
    );

    let CalibrationOutcome::Error(message) = outcome else {
        panic!("expected an error, got {outcome:?}");
    };
    assert!(message.contains("GPU run"), "{message}");
}

/// A tree with no BC7 or BC6H Texture has no floor: it says nothing about
/// the threshold, rather than confirming it.
#[test]
fn a_tree_without_bc7_or_bc6h_has_no_floor() {
    let temp = TempDir::new("calibration-no-bc7");
    let (gpu, cpu) = (temp.path().join("gpu"), temp.path().join("cpu"));
    write(&gpu, "a.dds", &encoded(BC3, TEX_COMPRESS_DEFAULT));
    write(&cpu, "a.dds", &encoded(BC3, TEX_COMPRESS_DEFAULT));

    let measured = measure_trees(&gpu, &cpu).unwrap();

    assert!(measured.textures.is_empty());
    assert_eq!(measured.floor(), None);
    assert!(measured.notes.is_empty(), "{:?}", measured.notes);
}

/// A measured BC7 Texture whose images reach `dbs`, mip by mip.
fn texture(path: &str, dbs: &[f64]) -> TextureMeasurement {
    TextureMeasurement {
        path: path.into(),
        format: BC7,
        images: dbs
            .iter()
            .enumerate()
            .map(|(mip, &db)| ImagePsnr {
                mip,
                item: 0,
                slice: 0,
                db,
            })
            .collect(),
    }
}

fn host() -> CalibrationHost {
    CalibrationHost {
        name: "BENCH".into(),
        adapter: "Example GPU 9000".into(),
        driver: "32.0.15.6614".into(),
    }
}

/// The summary prints each case's floor, the overall floor and the
/// threshold that follows from it, with the host and driver to record.
#[test]
fn the_summary_reports_each_floor_and_the_calibrated_threshold() {
    let summary = CalibrationSummary {
        host: host(),
        records: vec![
            (
                "apply-texture-bc7".into(),
                CalibrationOutcome::Measured(TreeMeasurement {
                    textures: vec![
                        texture("mods/A/textures/a.dds", &[36.5, 34.25, 41.0]),
                        texture("mods/A/textures/b.dds", &[f64::INFINITY]),
                    ],
                    notes: vec!["mods/A/textures/c.dds: the BC3 bytes differ".into()],
                }),
            ),
            (
                "apply-texture-bc6h".into(),
                CalibrationOutcome::Measured(TreeMeasurement {
                    textures: vec![texture("mods/B/textures/sky.dds", &[f64::INFINITY])],
                    notes: vec![],
                }),
            ),
            (
                "pw-001-sse-om-apply".into(),
                CalibrationOutcome::Measured(TreeMeasurement::default()),
            ),
            (
                "apply-le-animations".into(),
                CalibrationOutcome::NotRun("it requests Animations".into()),
            ),
        ],
    };

    assert_eq!(summary.observed_floor(), Some(34.25));
    assert_eq!(summary.threshold(), Some(33.25));
    assert_eq!(summary.exit_code(), 0);

    let text = summary.render();
    for expected in [
        "Host: BENCH",
        "Adapter: Example GPU 9000",
        "Driver: 32.0.15.6614",
        "apply-texture-bc7: floor 34.25 dB at mods/A/textures/a.dds, mip 1 of item 0",
        // Each Texture's own floor, so a reader sees where the low ones are.
        "mods/A/textures/a.dds: 34.25 dB at mip 1 of item 0",
        "mods/A/textures/b.dds: infinite (identical)",
        "mods/A/textures/c.dds: the BC3 bytes differ",
        "apply-texture-bc6h: identical on both encoders",
        "pw-001-sse-om-apply: no BC7 or BC6H Texture",
        "apply-le-animations: not run: it requests Animations",
        "Observed floor: 34.25 dB",
        "Threshold: 33.25 dB",
    ] {
        assert!(
            text.contains(expected),
            "`{expected}` missing from:\n{text}"
        );
    }
}

/// With every image identical, or none below 40 dB, the threshold is
/// confirmed at 40 dB; with nothing measured at all there is no threshold,
/// and with an error the floor cannot be trusted.
#[test]
fn the_threshold_is_confirmed_unmeasured_or_untrusted() {
    let measured = |dbs: &[f64]| {
        CalibrationOutcome::Measured(TreeMeasurement {
            textures: vec![texture("a.dds", dbs)],
            notes: vec![],
        })
    };
    let summary = |records: Vec<(String, CalibrationOutcome)>| CalibrationSummary {
        host: host(),
        records,
    };

    let confirmed = summary(vec![("a".into(), measured(&[44.0, f64::INFINITY]))]);
    assert_eq!(confirmed.threshold(), Some(40.0));
    assert!(confirmed.render().contains("Threshold: 40 dB, confirmed"));
    assert_eq!(confirmed.exit_code(), 0);

    let unmeasured = summary(vec![(
        "a".into(),
        CalibrationOutcome::Measured(TreeMeasurement::default()),
    )]);
    assert_eq!(unmeasured.observed_floor(), None);
    assert_eq!(unmeasured.threshold(), None);
    assert_eq!(unmeasured.exit_code(), 3, "nothing calibrated");

    let failed = summary(vec![
        ("a".into(), measured(&[44.0])),
        ("b".into(), CalibrationOutcome::Error("broken".into())),
    ]);
    assert_eq!(failed.exit_code(), 2);
    let text = failed.render();
    assert!(text.contains("b: error: broken"), "{text}");
    assert!(text.contains("incomplete"), "{text}");
}
