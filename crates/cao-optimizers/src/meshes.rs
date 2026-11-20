//! Mesh optimization with nifly, ported from C++ `MeshesOptimizer` (#504).
//!
//! [`MeshOptimizer`] loads a Mesh with `nifly-sys`, decides from the mesh
//! level what `OptimizeFor` call, if any, the Mesh gets, and saves it. Meshes
//! are processed one at a time on the Run Worker, as in C++: a [`Nif`] is
//! `Send` but not `Sync`, and a nifly call cannot be interrupted, so
//! cancellation waits for the Mesh in progress.
//!
//! Two C++ behaviours are ported as-is (#460):
//!
//! - `scan()` asks whether the profile's *target* version is Skyrim LE, not
//!   the Mesh's own, so under an LE target every valid Mesh is a critical
//!   issue.
//! - `OptimizeFor` converts only between Skyrim LE and SSE. For any other pair,
//!   an FO4 target included, it changes nothing.
//!
//! What C++ did with that no-op is not ported (audit BE-6). C++ counted a
//! Mesh as changed from its level and scan alone, so a Mesh `OptimizeFor` left
//! alone was still saved again, and logged nothing. Here a Mesh changes when
//! `OptimizeFor` converted it, when the medium level or above saves every
//! valid Mesh again, or when Resave is on. A Mesh already in the target's
//! version is left alone quietly; one in a version that cannot be converted
//! at all is a warning. Dry Run predicts the same from the Mesh's version.
//! A save that only reproduces the Mesh is then dropped by core. nifly's save
//! reaches a fixed point within two saves (pinned by `nifly-sys`'s fixture
//! tests), so a Mesh re-saved on every run is a change on the first run, at
//! most once more on the second, and never after.
//!
//! A C++ exception inside nifly ([`NifError::Exception`]) is returned from
//! here like any other nifly error; the backend reports it as the failure of
//! the call that raised it, and an exception on load as content it cannot
//! decode, which Apply quarantines (see [`crate::backend`]).
//!
//! Headpart Meshes (#505) are those the run's [`HeadpartList`] names and those
//! on a facegen path. Deviation 17 fixes two things C++ did:
//!
//! - It matched a Mesh by its absolute path cut at the first `/meshes/`, so a
//!   `meshes` folder at or above the Mod Root shifted the path. Here a Mesh is
//!   matched by its game path within its Mod Root.
//! - Its Dry Run left out the facegen rule, so it reported less than Apply
//!   did. Here Dry Run applies it too.

use std::path::Path;

use cao_core::routing::{ExecutionMode, MeshVariant};
use cao_core::run::MeshLevel;
use nifly_sys::{LoadOptions, Nif, NifError, NifVersion, OptimizeOptions, OptimizeReport};

use crate::headparts::HeadpartList;

/// The per-run Mesh options: C++ `OptionsCAO`'s Mesh fields, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshSettings {
    /// `iMeshesOptimizationLevel`: 0 (off), 1 (necessary), 2 (medium) or 3
    /// (full).
    pub level: MeshLevel,
    /// `bMeshesHeadparts`: optimize Headpart Meshes as head parts.
    pub headparts: bool,
    /// `bMeshesResave`: save every loaded Mesh again, whatever the level.
    pub resave: bool,
}

impl MeshSettings {
    /// Whether Headpart Meshes get their own rule: from the necessary level
    /// up, as in C++. Below it a run needs no headpart list.
    pub fn recognises_headparts(&self) -> bool {
        self.level >= MeshLevel::NECESSARY
    }

    /// Whether every valid Mesh is saved again whatever `OptimizeFor` does:
    /// from the medium level up, or with Resave on. nifly's save itself
    /// updates bounds, drops unreferenced blocks and sorts them, which is the
    /// light optimization the medium level adds.
    fn resaves(&self) -> bool {
        self.resave || self.level >= MeshLevel::MEDIUM
    }
}

/// What `scan()` makes of a loaded Mesh. C++ also had `lightIssue`, which no
/// scan ever returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Scan {
    /// The Mesh is not valid; nothing is done with it.
    DoNotProcess,
    /// SSE-compatible under a target other than Skyrim LE: only the full
    /// level runs `OptimizeFor` on it.
    Good,
    /// The Mesh needs `OptimizeFor` from the necessary level up.
    CriticalIssue,
}

/// The Mesh optimizer of one run.
#[derive(Debug, Clone)]
pub struct MeshOptimizer {
    settings: MeshSettings,
    /// The profile's `meshesFileVersion`, `meshesUser` and `meshesStream`.
    target: NifVersion,
    /// The run's Headpart Mesh list, from the profile and its plugins. Empty
    /// until [`MeshOptimizer::with_headparts`], when only the facegen rule
    /// applies.
    headparts: HeadpartList,
}

impl MeshOptimizer {
    /// An optimizer applying `settings`, converting towards `target`.
    pub fn new(settings: MeshSettings, target: NifVersion) -> Self {
        Self {
            settings,
            target,
            headparts: HeadpartList::default(),
        }
    }

    /// This optimizer, recognising the Headpart Meshes `headparts` names.
    #[must_use]
    pub fn with_headparts(mut self, headparts: HeadpartList) -> Self {
        self.headparts = headparts;
        self
    }

    /// Loads the Mesh at `path`, as a terrain file for a Terrain Mesh.
    ///
    /// # Errors
    /// The [`NifError`] the load reported, logged here: [`NifError::Read`]
    /// when the file could not be read, otherwise nifly's refusal of its
    /// bytes.
    pub fn load(&self, path: &Path, variant: MeshVariant) -> Result<Nif, NifError> {
        log::trace!("Loading mesh: {}", path.display());
        let mut nif = Nif::new();
        let options = LoadOptions {
            is_terrain: variant == MeshVariant::Terrain,
        };
        nif.load(path, options)
            .inspect_err(|_| log::error!("Cannot load mesh: {}", path.display()))?;
        Ok(nif)
    }

    /// Saves `nif` to `path`.
    ///
    /// nifly's `std::ofstream` passes the path to Win32 as it is given, so a
    /// path longer than `MAX_PATH` is handed over in its extended-length form,
    /// which opens whatever the host's `LongPathsEnabled` setting (WS-1).
    ///
    /// # Errors
    /// The [`NifError`] nifly's save reported; it is logged here.
    pub fn save(&self, nif: &mut Nif, path: &Path) -> Result<(), NifError> {
        log::trace!("Saving mesh: {}", path.display());
        // Only an empty path or one holding NUL cannot be converted, and the
        // shim's open refuses those as well, reporting NifError::Save, so the
        // path as given is the right thing to pass on.
        let target = cao_winfs::extended_length_path(path).unwrap_or_else(|_| path.to_path_buf());
        nif.save(&target)
            .inspect_err(|_| log::error!("Cannot save mesh: {}", path.display()))
    }

    /// C++ `scan()`: a valid Mesh is a critical issue when it is not
    /// SSE-compatible, or when the *target* is Skyrim LE (ported as-is).
    fn scan(&self, nif: &mut Nif) -> Result<Scan, NifError> {
        if !nif.is_valid() {
            return Ok(Scan::DoNotProcess);
        }
        // `||` keeps C++'s order: compatibility is always checked first.
        if !nif.is_sse_compatible()? || self.target.is_sk() {
            Ok(Scan::CriticalIssue)
        } else {
            Ok(Scan::Good)
        }
    }

    /// Optimizes the loaded `nif` from `path` at the run's mesh level and
    /// reports whether the Mesh changed (Apply) or would change (Dry Run), so
    /// whether it must be saved.
    ///
    /// `OptimizeFor` is run on a Headpart Mesh (with head-part options) from
    /// the necessary level up, on a critical issue from the necessary level
    /// up, and on any Mesh at the full level; with head parts off, a Headpart
    /// Mesh gets none. The Mesh changed when that call converted it, and also,
    /// whatever the call did, from the medium level up or with Resave on
    /// ([`MeshSettings::resaves`]). A call nifly answers with a version
    /// mismatch changed nothing, so on its own it no longer saves the Mesh
    /// (audit BE-6); see [`log_mismatch`] for what is logged.
    ///
    /// Dry Run evaluates the same rules without touching `nif`, the facegen
    /// rule included (deviation 17), predicting the conversion from the Mesh's
    /// version as nifly decides it ([`NifVersion::converts_to`]).
    ///
    /// `path` is matched by its game path within `mod_root`, the Mod Root the
    /// attempt is attributed to (deviation 17), and logged; see
    /// [`game_path`].
    ///
    /// # Errors
    /// The [`NifError`] a nifly call reported.
    pub fn optimize(
        &self,
        nif: &mut Nif,
        path: &Path,
        mod_root: &Path,
        mode: ExecutionMode,
    ) -> Result<bool, NifError> {
        let scan = self.scan(nif)?;
        if scan == Scan::DoNotProcess {
            return Ok(false);
        }
        let level = self.settings.level;
        let shown = path.display();
        let game_path = game_path(path, mod_root);
        // Qt matched both rules ignoring case.
        let is_headpart = self.settings.recognises_headparts()
            && (self.headparts.contains(&game_path)
                || game_path.to_lowercase().contains("facegen"));

        // Why `OptimizeFor` runs, if it does: the same in both modes.
        let reason = if is_headpart {
            if !self.settings.headparts {
                log::trace!("Headpart mesh ignored: {shown}");
            }
            self.settings
                .headparts
                .then_some("as an headpart due to necessary optimization")
        } else {
            match scan {
                Scan::Good if level >= MeshLevel::FULL => Some("due to full optimization"),
                Scan::CriticalIssue if level >= MeshLevel::NECESSARY => {
                    Some("due to necessary optimization")
                }
                _ => None,
            }
        };
        let version = nif.version()?;

        if mode == ExecutionMode::DryRun {
            let converts = reason.is_some() && version.converts_to(&self.target);
            if let Some(reason) = reason.filter(|_| converts) {
                log::info!("{shown} would be optimized {reason}");
            } else {
                if reason.is_some() {
                    self.log_mismatch(&shown, version);
                }
                if level >= MeshLevel::MEDIUM {
                    log::info!("{shown} would be optimized due to medium optimization");
                }
            }
            return Ok(converts || self.settings.resaves());
        }

        let mut converted = false;
        if let Some(reason) = reason {
            let options = OptimizeOptions {
                target: self.target,
                head_parts: is_headpart,
                remove_parallax: false,
            };
            log::info!("Optimizing: {shown} {reason}");
            let report = nif.optimize_for(&options)?;
            if report.version_mismatch {
                self.log_mismatch(&shown, version);
            } else {
                log_report(&report);
                converted = true;
            }
        }
        Ok(converted || self.settings.resaves())
    }

    /// Logs an `OptimizeFor` call that could not convert the Mesh, whose
    /// version is `version`.
    ///
    /// A Mesh already in the target's version is nothing to report beyond
    /// the verbose log: under a Skyrim LE target every valid Mesh scans as a
    /// critical issue, so each LE Mesh gets here on every run. Any other
    /// version, such as a Fallout or Oblivion Mesh in a Skyrim mod, cannot be
    /// converted at all, and the user is warned.
    fn log_mismatch(&self, shown: &std::path::Display<'_>, version: NifVersion) {
        let target = self.target;
        if version.file == target.file && version.stream == target.stream {
            log::trace!("{shown} is already in the target Mesh version");
        } else {
            log::warn!(
                "{shown} was not converted: its Mesh version (stream {}) cannot be converted \
                 to the target's (stream {}); only Skyrim LE and SSE Meshes convert",
                version.stream,
                target.stream
            );
        }
    }
}

/// The `/`-separated game path of the Mesh at `path` within `mod_root`, such
/// as `meshes/actors/hair.nif` (deviation 17).
///
/// Discovery joins every path it finds onto its canonical Mod Root, so the
/// prefix strips; a path spelled otherwise is resolved first, as the Run
/// Executor resolved it to attribute the attempt. A path still outside `mod_root`, or
/// any path when `mod_root` is empty (a standalone call), is matched whole.
fn game_path(path: &Path, mod_root: &Path) -> String {
    let within = |path: &Path| {
        (!mod_root.as_os_str().is_empty())
            .then(|| path.strip_prefix(mod_root).ok().map(Path::to_path_buf))
            .flatten()
    };
    let relative = within(path)
        .or_else(|| {
            cao_winfs::weakly_canonical(path)
                .ok()
                .and_then(|resolved| within(&resolved))
        })
        .unwrap_or_else(|| path.to_path_buf());
    relative.to_string_lossy().replace('\\', "/")
}

/// Logs what one `OptimizeFor` call that converted a Mesh did, as C++'s
/// `print_res` did, at the verbose level. A version mismatch, which changes
/// nothing, is logged by [`MeshOptimizer::log_mismatch`] instead.
fn log_report(report: &OptimizeReport) {
    let list = |names: &[String]| format!("[{}]", names.join(", "));
    log::trace!(
        "Details of mesh optimization:\n\
         res.dupesRenamed: {}\n\
         res.shapesNormalsRemoved: {}\n\
         res.shapesParallaxRemoved: {}\n\
         res.shapesVColorsRemoved: {}\n\
         res.shapesPartTriangulated: {}\n\
         res.shapesTangentsAdded: {}",
        report.dupes_renamed,
        list(&report.shapes_normals_removed),
        list(&report.shapes_parallax_removed),
        list(&report.shapes_vcolors_removed),
        list(&report.shapes_part_triangulated),
        list(&report.shapes_tangents_added),
    );
}
