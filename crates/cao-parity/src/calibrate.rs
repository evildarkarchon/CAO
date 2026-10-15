//! The BC7/BC6H PSNR threshold calibration (#513): `cao-parity calibrate`.
//!
//! The Texture rule lets BC7 and BC6H pixels differ by up to a PSNR floor
//! ([`MIN_BC7_BC6H_PSNR_DB`]) because C++'s own output already depends on the
//! host: it encodes them on the GPU when it gets a D3D11 device and on the CPU
//! otherwise. Calibration measures that spread once, on the oracle alone: each
//! BC7/BC6H case runs through the oracle twice, once on the GPU and once with
//! the CPU codec forced through [`FORCE_CPU_VARIABLE`], and every BC7 or BC6H
//! image the two runs wrote is compared with [`psnr_per_image`], the very
//! measurement the Texture rule applies. The threshold stays 40 dB unless the
//! lowest PSNR observed is below it, in which case it becomes that floor minus
//! 1 dB ([`calibrated_threshold`]).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use cao_archive::ReadArchive;
use directxtex::DXGI_FORMAT;

use crate::HarnessError;
use crate::case::{
    CaseFile, CaseLayout, CaseSpec, Deadline, Side, is_harness_owned, list_dir, read_file,
    run_captured,
};
use crate::corpus::remove_if_present;
use crate::materialise::{Environment, Readiness, materialise};
use crate::recipe::ContentEntry;
use crate::textures::{
    CALIBRATION_HOST, ImagePsnr, MIN_BC7_BC6H_PSNR_DB, decode_to_float, header_len, is_bc7_or_bc6h,
    load, psnr_per_image,
};

/// The oracle-only environment variable that makes C++ `createDevice` fail, so
/// BC6H and BC7 take the CPU codec. Any non-empty value forces it.
pub const FORCE_CPU_VARIABLE: &str = "CAO_ORACLE_FORCE_CPU_BC";

/// Whether calibration runs `case`: one that can encode a Texture, so BC7 or
/// BC6H output may come of it.
///
/// That is a case that is not a Dry Run, applies some texture option, and
/// holds a `texture` entry, loose or packed in an input Archive. Which formats
/// come out depends on the profile, its overrides and each source, so the
/// selection is generous and measurement keeps only BC7 and BC6H images.
pub fn is_calibration_case(case: &CaseFile) -> bool {
    let options = &case.spec.textures;
    let encodes = options.necessary
        || options.compress
        || options.mipmaps
        || options.resize_by_ratio
        || options.resize_by_size;
    let holds_texture = case.tree.content.iter().any(|entry| match entry {
        ContentEntry::Texture(_) => true,
        ContentEntry::Archive(archive) => archive
            .content
            .iter()
            .any(|packed| matches!(packed, ContentEntry::Texture(_))),
        _ => false,
    });
    !case.spec.dry_run && encodes && holds_texture
}

/// The warning C++ logs when it gets no D3D11 device, so BC6H and BC7 take
/// the CPU codec. The CPU run must log it and the GPU run must not.
pub const CPU_CODEC_WARNING: &str = "DirectCompute is not available";

/// What calibrating a case needs.
pub struct Calibrator<'a> {
    /// The work directory every case directory sits in.
    pub work: &'a Path,
    pub environment: Environment<'a>,
    /// Builds one oracle run over the side root it is given. The calibrator
    /// sets the working directory to that root, and sets or clears
    /// [`FORCE_CPU_VARIABLE`] itself, so the builder need not.
    pub oracle: &'a dyn Fn(&Path, &CaseSpec) -> Result<Command, HarnessError>,
    /// Each oracle run's timeout.
    pub timeout: Duration,
}

/// How calibrating one case ended.
#[derive(Debug, Clone, PartialEq)]
pub enum CalibrationOutcome {
    /// Both runs finished and every BC7 and BC6H Texture they wrote was
    /// measured; the case directory is deleted.
    Measured(TreeMeasurement),
    /// The host lacks a prerequisite of the case, with which one.
    NotRun(String),
    /// The case could not be measured, with why; its directory is kept.
    Error(String),
}

impl CalibrationOutcome {
    /// The outcome's name, as progress lines print it.
    pub fn name(&self) -> &'static str {
        match self {
            CalibrationOutcome::Measured(_) => "measured",
            CalibrationOutcome::NotRun(_) => "not run",
            CalibrationOutcome::Error(_) => "error",
        }
    }
}

/// Calibrates one case: materialises it afresh under the work directory, runs
/// the oracle over its `oracle/` copy on the GPU and over its `rust/` copy
/// with the CPU codec forced, then measures what the two runs wrote.
///
/// The `rust/` copy hosts the CPU run only because materialisation makes two
/// identical copies; no Rust build runs here. Captures go to `gpu.stdout.txt`,
/// `gpu.stderr.txt`, `cpu.stdout.txt` and `cpu.stderr.txt` in the case
/// directory.
///
/// The runs must exit alike, the GPU run must not log [`CPU_CODEC_WARNING`],
/// and, once any BC7 or BC6H Texture was measured, the CPU run must log it:
/// otherwise the host gave the oracle no GPU, or the oracle predates
/// [`FORCE_CPU_VARIABLE`], and the measurement would not compare the two
/// encoders at all.
pub fn calibrate_case(
    calibrator: &Calibrator<'_>,
    id: &str,
    case: &CaseFile,
) -> CalibrationOutcome {
    let layout = match CaseLayout::new(calibrator.work, id) {
        Ok(layout) => layout,
        Err(error) => return CalibrationOutcome::Error(error.to_string()),
    };
    match try_calibrate(calibrator, &layout, case) {
        Ok(CalibrationOutcome::Measured(measurement)) => {
            match std::fs::remove_dir_all(layout.root()) {
                Ok(()) => CalibrationOutcome::Measured(measurement),
                Err(error) => CalibrationOutcome::Error(format!(
                    "deleting {}: {error}",
                    layout.root().display()
                )),
            }
        }
        Ok(outcome) => outcome,
        Err(error) => CalibrationOutcome::Error(format!(
            "{error} (case directory: {})",
            layout.root().display()
        )),
    }
}

/// [`calibrate_case`]'s body, with harness errors as `Err`.
fn try_calibrate(
    calibrator: &Calibrator<'_>,
    layout: &CaseLayout,
    case: &CaseFile,
) -> Result<CalibrationOutcome, HarnessError> {
    remove_if_present(layout.root())?;
    layout.write_case(case)?;
    if let Readiness::NotRun(reason) = materialise(layout, case, &calibrator.environment)? {
        return Ok(CalibrationOutcome::NotRun(reason));
    }

    let (gpu_root, cpu_root) = (layout.side(Side::Oracle), layout.side(Side::Rust));
    let run = |root: &Path, encoder: &str, force_cpu: bool| {
        let mut command = (calibrator.oracle)(root, &case.spec)?;
        command.current_dir(root);
        if force_cpu {
            command.env(FORCE_CPU_VARIABLE, "1");
        } else {
            // The harness's own environment must not force the GPU run onto
            // the CPU.
            command.env_remove(FORCE_CPU_VARIABLE);
        }
        run_captured(
            command,
            &layout.root().join(format!("{encoder}.stdout.txt")),
            &layout.root().join(format!("{encoder}.stderr.txt")),
            Side::Oracle,
            Deadline::after(calibrator.timeout),
        )
    };
    let gpu_exit = run(&gpu_root, "gpu", false)?;
    let cpu_exit = run(&cpu_root, "cpu", true)?;
    if gpu_exit != cpu_exit {
        return Err(HarnessError::Calibration(format!(
            "the GPU run exited with {gpu_exit} and the CPU run with {cpu_exit}"
        )));
    }

    let measurement = measure_trees(&gpu_root, &cpu_root)?;
    if logs_mention(&gpu_root, CPU_CODEC_WARNING)? {
        return Err(HarnessError::Calibration(format!(
            "the GPU run logged \"{CPU_CODEC_WARNING}\": the oracle got no D3D11 device, so \
             calibration needs a host with a GPU"
        )));
    }
    if !measurement.textures.is_empty() && !logs_mention(&cpu_root, CPU_CODEC_WARNING)? {
        return Err(HarnessError::Calibration(format!(
            "the CPU run never logged \"{CPU_CODEC_WARNING}\", so it encoded on the GPU too: \
             the oracle ignores {FORCE_CPU_VARIABLE}; rebuild it from this checkout"
        )));
    }
    Ok(CalibrationOutcome::Measured(measurement))
}

/// The host a calibration ran on, which the threshold constant records: the
/// GPU encoder's output depends on the adapter and its driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationHost {
    /// The machine's name.
    pub name: String,
    /// The DXGI adapter the oracle encodes on: adapter 0, as C++ asks for.
    pub adapter: String,
    /// That adapter's driver version.
    pub driver: String,
}

/// A whole calibration: every case's outcome, in run order, and the host.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationSummary {
    pub host: CalibrationHost,
    pub records: Vec<(String, CalibrationOutcome)>,
}

impl CalibrationSummary {
    /// Every measurement, with its case id.
    fn measurements(&self) -> impl Iterator<Item = (&str, &TreeMeasurement)> {
        self.records
            .iter()
            .filter_map(|(id, outcome)| match outcome {
                CalibrationOutcome::Measured(measurement) => Some((id.as_str(), measurement)),
                _ => None,
            })
    }

    fn errors(&self) -> usize {
        self.records
            .iter()
            .filter(|(_, outcome)| matches!(outcome, CalibrationOutcome::Error(_)))
            .count()
    }

    /// The lowest PSNR of any measured image, infinite when every one was
    /// identical; `None` when no BC7 or BC6H Texture was measured at all.
    pub fn observed_floor(&self) -> Option<f64> {
        self.measurements()
            .filter_map(|(_, measurement)| measurement.floor())
            .reduce(f64::min)
    }

    /// The threshold [`calibrated_threshold`] gives for the observed floor;
    /// `None` when nothing was measured.
    pub fn threshold(&self) -> Option<f64> {
        self.observed_floor().map(calibrated_threshold)
    }

    /// `calibrate`'s exit code: 2 when any case hit an error, since the floor
    /// may then be missing its lowest image; else 3 when no BC7 or BC6H Texture
    /// was measured, so nothing was calibrated; else 0.
    pub fn exit_code(&self) -> u8 {
        if self.errors() > 0 {
            2
        } else if self.observed_floor().is_none() {
            3
        } else {
            0
        }
    }

    /// The summary as plain text: the host, each case's floor or outcome with
    /// its notes, then the observed floor and the threshold it gives.
    pub fn render(&self) -> String {
        let host = &self.host;
        let mut text = format!(
            "cao-parity calibrate: the BC7/BC6H PSNR threshold\n\n\
             Host: {}\nAdapter: {}\nDriver: {}\n\nCases:\n",
            host.name, host.adapter, host.driver
        );
        for (id, outcome) in &self.records {
            let line = match outcome {
                CalibrationOutcome::Measured(measurement) => match measurement.lowest() {
                    None => "no BC7 or BC6H Texture".to_owned(),
                    Some((_, image)) if image.db.is_infinite() => format!(
                        "identical on both encoders ({} BC7/BC6H Textures)",
                        measurement.textures.len()
                    ),
                    Some((texture, image)) => format!(
                        "floor {} at {}, {} ({} BC7/BC6H Textures)",
                        decibels(image.db),
                        texture.path,
                        image.label(),
                        measurement.textures.len()
                    ),
                },
                CalibrationOutcome::NotRun(reason) => format!("not run: {reason}"),
                CalibrationOutcome::Error(message) => format!("error: {message}"),
            };
            text.push_str(&format!("  {id}: {line}\n"));
            if let CalibrationOutcome::Measured(measurement) = outcome {
                for texture in &measurement.textures {
                    let floor = match texture.lowest() {
                        Some(image) if image.db.is_finite() => {
                            format!("{} at {}", decibels(image.db), image.label())
                        }
                        _ => decibels(f64::INFINITY),
                    };
                    text.push_str(&format!("    {}: {floor}\n", texture.path));
                }
                for note in &measurement.notes {
                    text.push_str(&format!("    note: {note}\n"));
                }
            }
        }

        text.push('\n');
        match self.observed_floor() {
            None => text.push_str(
                "Observed floor: none; no case wrote a BC7 or BC6H Texture, so nothing was \
                 calibrated\n",
            ),
            Some(floor) => {
                text.push_str(&format!("Observed floor: {}\n", decibels(floor)));
                let threshold = calibrated_threshold(floor);
                if floor >= 40.0 {
                    text.push_str("Threshold: 40 dB, confirmed: the floor is not below it\n");
                } else {
                    text.push_str(&format!(
                        "Threshold: {}, the floor minus 1 dB (rounded down to 0.01 dB)\n",
                        decibels(threshold)
                    ));
                }
                let current = MIN_BC7_BC6H_PSNR_DB;
                text.push_str(&if current == threshold {
                    format!(
                        "MIN_BC7_BC6H_PSNR_DB is already {}, recorded on: {CALIBRATION_HOST}\n",
                        decibels(current)
                    )
                } else {
                    format!(
                        "MIN_BC7_BC6H_PSNR_DB is {}: set it to {threshold:?}, and \
                         CALIBRATION_HOST to this host, adapter and driver\n",
                        decibels(current)
                    )
                });
            }
        }
        let errors = self.errors();
        if errors > 0 {
            text.push_str(&format!(
                "{errors} case(s) hit an error, so the calibration is incomplete: the floor \
                 may lack their images\n"
            ));
        }
        text
    }
}

/// A PSNR for the summary: whole decibels as they are, others to 0.01 dB,
/// and an infinite one as identical.
pub fn decibels(value: f64) -> String {
    if value.is_infinite() {
        "infinite (identical)".to_owned()
    } else if value.fract() == 0.0 {
        format!("{value:.0} dB")
    } else {
        format!("{value:.2} dB")
    }
}

/// Whether any file under the side's `logs/` folder holds `text`. A side
/// without `logs/` mentions nothing.
fn logs_mention(side_root: &Path, text: &str) -> Result<bool, HarnessError> {
    let logs = side_root.join("logs");
    if !logs.is_dir() {
        return Ok(false);
    }
    let mut files = Vec::new();
    collect_files(&logs, "", &mut files)?;
    for relative in files {
        let bytes = read_file(&logs.join(relative))?;
        if bytes
            .windows(text.len())
            .any(|window| window == text.as_bytes())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The threshold the Texture rule should apply, given the lowest PSNR the
/// calibration observed (infinite when every image decoded identically).
///
/// 40 dB stands unless the floor is lower; then it is the floor minus 1 dB,
/// rounded down to 0.01 dB so the recorded constant never exceeds that.
pub fn calibrated_threshold(observed_floor: f64) -> f64 {
    const PROVISIONAL: f64 = 40.0;
    if observed_floor >= PROVISIONAL {
        PROVISIONAL
    } else {
        ((observed_floor - 1.0) * 100.0).floor() / 100.0
    }
}

/// One BC7 or BC6H Texture both runs wrote, measured image by image.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureMeasurement {
    /// The Texture's `/`-separated path relative to the side root, or
    /// `<archive path>:<entry name>` for one inside an Archive.
    pub path: String,
    /// The format the GPU run wrote, typeless formats read as UNORM.
    pub format: DXGI_FORMAT,
    /// Every image, in DirectXTex's image order; infinite where the two runs
    /// decoded identically.
    pub images: Vec<ImagePsnr>,
}

impl TextureMeasurement {
    /// The lowest PSNR of any image, infinite when every image decoded
    /// identically.
    pub fn floor(&self) -> f64 {
        self.lowest().map_or(f64::INFINITY, |image| image.db)
    }

    /// The image with the lowest PSNR, if the Texture has any image.
    pub fn lowest(&self) -> Option<&ImagePsnr> {
        // Measurement rejects NaN, so the order is total here.
        self.images.iter().min_by(|a, b| a.db.total_cmp(&b.db))
    }
}

/// What one case's two runs wrote, as calibration sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TreeMeasurement {
    /// Every BC7 and BC6H Texture both runs wrote at the same path, loose ones
    /// in path order and each Archive's entries in name order at the
    /// Archive's place.
    pub textures: Vec<TextureMeasurement>,
    /// Differences calibration does not measure but a reader should see, such
    /// as a Texture of another format whose bytes differ between the runs:
    /// only BC6H and BC7 take a different encoder on the CPU.
    pub notes: Vec<String>,
}

impl TreeMeasurement {
    /// The lowest PSNR of any measured image, or `None` when no BC7 or BC6H
    /// Texture was measured.
    pub fn floor(&self) -> Option<f64> {
        self.textures
            .iter()
            .map(TextureMeasurement::floor)
            .reduce(f64::min)
    }

    /// The lowest image of any measured Texture, with its Texture.
    pub fn lowest(&self) -> Option<(&TextureMeasurement, &ImagePsnr)> {
        self.textures
            .iter()
            .filter_map(|texture| texture.lowest().map(|image| (texture, image)))
            .min_by(|(_, a), (_, b)| a.db.total_cmp(&b.db))
    }
}

/// Measures every BC7 and BC6H Texture that both the GPU run's tree (`gpu`)
/// and the CPU run's tree (`cpu`) hold at the same path: loose `.dds` files,
/// and `.dds` entries of `.bsa` and `.ba2` Archives at the same path.
///
/// Harness-owned top-level folders are skipped, and anything only one run
/// wrote is ignored: calibration measures encoders, and the parity corpus
/// compares trees.
///
/// # Errors
/// [`HarnessError::Calibration`] when a BC7 or BC6H Texture whose bytes differ
/// cannot be measured: a side does not load or decode, the DDS headers
/// differ, or an image's PSNR is not a number. [`HarnessError::Io`] when a
/// tree cannot be read.
pub fn measure_trees(gpu: &Path, cpu: &Path) -> Result<TreeMeasurement, HarnessError> {
    let mut files = Vec::new();
    collect_files(gpu, "", &mut files)?;
    files.sort();
    let mut measurement = TreeMeasurement::default();
    for relative in files {
        let (gpu_path, cpu_path) = (gpu.join(&relative), cpu.join(&relative));
        if !cpu_path.is_file() {
            continue;
        }
        if has_extension(&relative, ".dds") {
            let (gpu_bytes, cpu_bytes) = (read_file(&gpu_path)?, read_file(&cpu_path)?);
            measure_dds(&relative, &gpu_bytes, &cpu_bytes, &mut measurement)?;
        } else if has_extension(&relative, ".bsa") || has_extension(&relative, ".ba2") {
            measure_archive(&relative, &gpu_path, &cpu_path, &mut measurement)?;
        }
    }
    Ok(measurement)
}

/// Collects the `/`-separated paths of every file beneath `directory`, which
/// is `relative` below the side root, skipping the side's harness-owned
/// top-level folders. Links are not followed: a measured Texture is a file
/// a run wrote.
fn collect_files(
    directory: &Path,
    relative: &str,
    found: &mut Vec<String>,
) -> Result<(), HarnessError> {
    for item in list_dir(directory)? {
        let name = item.file_name().to_string_lossy().into_owned();
        if relative.is_empty() && is_harness_owned(&name) {
            continue;
        }
        let child = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        let file_type = item.file_type().map_err(|error| {
            HarnessError::io(format!("reading {}", item.path().display()), error)
        })?;
        if file_type.is_dir() {
            collect_files(&item.path(), &child, found)?;
        } else if file_type.is_file() {
            found.push(child);
        }
    }
    Ok(())
}

/// Whether `path` ends in `extension` (with its dot), ignoring ASCII case as
/// Asset Routing does.
fn has_extension(path: &str, extension: &str) -> bool {
    path.len() > extension.len()
        && path.is_char_boundary(path.len() - extension.len())
        && path[path.len() - extension.len()..].eq_ignore_ascii_case(extension)
}

/// Measures the `.dds` entries two Archives at `relative` both hold.
///
/// An Archive either run cannot read is noted rather than measured: the
/// Archive rule judges Archives, and this only looks for Textures inside them.
fn measure_archive(
    relative: &str,
    gpu: &Path,
    cpu: &Path,
    measurement: &mut TreeMeasurement,
) -> Result<(), HarnessError> {
    let open = |path: &Path| match ReadArchive::open(path) {
        Ok(Some(archive)) => archive
            .archived_assets()
            .map(|assets| (archive, assets))
            .map_err(|error| error.to_string()),
        Ok(None) => Err("it has no known magic".to_owned()),
        Err(error) => Err(error.to_string()),
    };
    let ((gpu_archive, gpu_assets), (cpu_archive, cpu_assets)) = match (open(gpu), open(cpu)) {
        (Ok(gpu), Ok(cpu)) => (gpu, cpu),
        (gpu, cpu) => {
            let reason = gpu.err().or(cpu.err()).unwrap_or_default();
            measurement.notes.push(format!(
                "{relative}: not searched for Textures, since a run's Archive does not read: \
                 {reason}"
            ));
            return Ok(());
        }
    };
    let mut names: Vec<&str> = gpu_assets
        .iter()
        .map(|asset| asset.name.as_str())
        .filter(|name| has_extension(name, ".dds"))
        .filter(|name| cpu_assets.iter().any(|asset| asset.name == *name))
        .collect();
    names.sort_unstable();
    for name in names {
        let path = format!("{relative}:{name}");
        let extract = |archive: &ReadArchive| {
            let mut bytes = Vec::new();
            archive
                .extract(name, &mut bytes)
                .map(|()| bytes)
                .map_err(|error| HarnessError::Calibration(format!("{path}: {error}")))
        };
        let (gpu_bytes, cpu_bytes) = (extract(&gpu_archive)?, extract(&cpu_archive)?);
        measure_dds(&path, &gpu_bytes, &cpu_bytes, measurement)?;
    }
    Ok(())
}

/// Measures one DDS file both runs wrote at `path`, adding it to
/// `measurement` when it is BC7 or BC6H.
///
/// A file whose bytes match on both sides but does not load (a damaged input
/// carried through) is not a Texture to measure. A file of another format is
/// not measured, and is noted when its bytes differ.
fn measure_dds(
    path: &str,
    gpu: &[u8],
    cpu: &[u8],
    measurement: &mut TreeMeasurement,
) -> Result<(), HarnessError> {
    let fail = |message: String| HarnessError::Calibration(format!("{path}: {message}"));
    let same = gpu == cpu;
    let gpu_image = match load(gpu) {
        Ok(image) => image,
        Err(_) if same => return Ok(()),
        Err(error) => return Err(fail(format!("the GPU run's file: {error}"))),
    };
    let format = gpu_image.metadata().format;
    if !is_bc7_or_bc6h(format) {
        if !same {
            measurement.notes.push(format!(
                "{path}: the {format:?} bytes differ between the GPU and CPU runs, though only \
                 BC6H and BC7 take another encoder"
            ));
        }
        return Ok(());
    }

    let cpu_image = if same {
        None
    } else {
        let gpu_header = header_len(gpu).map_err(|error| fail(format!("GPU run: {error}")))?;
        let cpu_header = header_len(cpu).map_err(|error| fail(format!("CPU run: {error}")))?;
        if gpu[..gpu_header] != cpu[..cpu_header] {
            return Err(fail("the DDS headers differ between the runs".to_owned()));
        }
        Some(load(cpu).map_err(|error| fail(format!("the CPU run's file: {error}")))?)
    };
    let decode = |run: &str, image: &directxtex::ScratchImage| {
        decode_to_float(image).map_err(|error| fail(format!("{run} run: cannot decode: {error}")))
    };
    let gpu_float = decode("GPU", &gpu_image)?;
    let images = match &cpu_image {
        // The same bytes decode the same: every image is infinitely close.
        None => psnr_per_image(&gpu_float, &gpu_float),
        Some(cpu_image) => psnr_per_image(&gpu_float, &decode("CPU", cpu_image)?),
    }
    .map_err(fail)?;
    if let Some(image) = images.iter().find(|image| image.db.is_nan()) {
        return Err(fail(format!("{} has no PSNR (NaN)", image.label())));
    }
    measurement.textures.push(TextureMeasurement {
        path: path.to_owned(),
        format,
        images,
    });
    Ok(())
}
