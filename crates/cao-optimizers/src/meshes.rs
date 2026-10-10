//! Mesh optimization with nifly, ported from C++ `MeshesOptimizer` (#504).
//!
//! [`MeshOptimizer`] loads a Mesh with `nifly-sys`, decides from the mesh
//! level what `OptimizeFor` call, if any, the Mesh gets, and saves it. Meshes
//! are processed one at a time on the Run Worker, as in C++: a [`Nif`] is
//! `Send` but not `Sync`, and a nifly call cannot be interrupted, so
//! cancellation waits for the Mesh in progress.
//!
//! Three C++ behaviours are ported as-is (#460):
//!
//! - `scan()` asks whether the profile's *target* version is Skyrim LE, not
//!   the Mesh's own, so under an LE target every valid Mesh is a critical
//!   issue.
//! - `OptimizeFor` converts only between Skyrim LE and SSE. For any other pair,
//!   an FO4 target included, it changes nothing.
//! - That version mismatch is never logged.
//!
//! A C++ exception inside nifly ([`NifError::Exception`]) is returned from
//! here; the backend turns it into the outcome C++ CAO reported for it (see
//! `unless_exception` in [`crate::backend`]).

use std::path::Path;

use cao_core::routing::{ExecutionMode, MeshVariant};
use nifly_sys::{LoadOptions, Nif, NifError, NifVersion, OptimizeOptions, OptimizeReport};

/// The per-run Mesh options: C++ `OptionsCAO`'s Mesh fields, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshSettings {
    /// `iMeshesOptimizationLevel`: 0 (off), 1 (necessary), 2 (medium) or 3
    /// (full).
    pub level: u8,
    /// `bMeshesHeadparts`: optimize Headpart Meshes as head parts.
    pub headparts: bool,
    /// `bMeshesResave`: save every loaded Mesh again, whatever the level.
    pub resave: bool,
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
    /// The Headpart Mesh list, as paths from `meshes/`. Recognising Headpart
    /// Meshes from the profile and plugins is #505's, so it is empty here and
    /// only the facegen rule applies.
    headparts: Vec<String>,
}

impl MeshOptimizer {
    /// An optimizer applying `settings`, converting towards `target`.
    pub fn new(settings: MeshSettings, target: NifVersion) -> Self {
        Self {
            settings,
            target,
            headparts: Vec::new(),
        }
    }

    /// Loads the Mesh at `path`, as a terrain file for a Terrain Mesh.
    ///
    /// # Errors
    /// The [`NifError`] nifly's load reported; it is logged here, as C++
    /// `loadMesh` logged it.
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
    /// # Errors
    /// The [`NifError`] nifly's save reported; it is logged here.
    pub fn save(&self, nif: &mut Nif, path: &Path) -> Result<(), NifError> {
        log::trace!("Saving mesh: {}", path.display());
        nif.save(path)
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

    /// Optimizes the loaded `nif` from `path` at the run's mesh level, as C++
    /// `MeshesOptimizer::optimize` does, and reports whether the Mesh changed
    /// (Apply) or would change (Dry Run), so whether it must be saved.
    ///
    /// Apply runs `OptimizeFor` on a Headpart Mesh (with head-part options)
    /// from the necessary level up, on a critical issue from the necessary
    /// level up, and on any Mesh at the full level. The medium level runs it
    /// on no other Mesh but still reports every Mesh as changed, so it is
    /// saved again. `path` is only matched and logged.
    ///
    /// Dry Run evaluates the same levels without touching `nif`, but as in
    /// C++ only a listed Headpart Mesh, not a facegen path, gets the headpart
    /// rule there; deviation 17 (#505) changes that.
    ///
    /// # Errors
    /// The [`NifError`] a nifly call reported.
    pub fn optimize(
        &self,
        nif: &mut Nif,
        path: &Path,
        mode: ExecutionMode,
    ) -> Result<bool, NifError> {
        let scan = self.scan(nif)?;
        if scan == Scan::DoNotProcess {
            return Ok(false);
        }
        let level = self.settings.level;
        let shown = path.display();
        // Folded once: Qt matched both rules ignoring case.
        let relative = relative_mesh_path(path).to_lowercase();
        let listed_headpart = self
            .headparts
            .iter()
            .any(|headpart| headpart.to_lowercase() == relative);

        if mode == ExecutionMode::DryRun {
            if level >= 1 && self.settings.headparts && listed_headpart {
                log::info!(
                    "{shown} would be optimized as an headpart due to necessary optimization"
                );
                return Ok(true);
            }
            let would_change = match scan {
                Scan::Good if level >= 3 => {
                    log::info!("{shown} would be optimized due to full optimization");
                    true
                }
                Scan::Good if level >= 2 => {
                    log::info!("{shown} would be optimized due to medium optimization");
                    true
                }
                Scan::CriticalIssue if level >= 1 => {
                    log::info!("{shown} would be optimized due to necessary optimization");
                    true
                }
                _ => false,
            };
            return Ok(would_change || self.settings.resave);
        }

        let mut options = OptimizeOptions {
            target: self.target,
            head_parts: false,
            remove_parallax: false,
        };
        let mut processed_headpart = false;
        if level >= 1 && (listed_headpart || relative.contains("facegen")) {
            if self.settings.headparts {
                options.head_parts = true;
                log::info!("Optimizing: {shown} as an headpart due to necessary optimization");
                log_report(&nif.optimize_for(&options)?);
                processed_headpart = true;
            } else {
                log::trace!("Headpart mesh ignored: {shown}");
            }
        } else {
            match scan {
                Scan::Good if level >= 3 => {
                    log::info!("Optimizing: {shown} due to full optimization");
                    log_report(&nif.optimize_for(&options)?);
                }
                Scan::CriticalIssue if level >= 1 => {
                    log::info!("Optimizing: {shown} due to necessary optimization");
                    log_report(&nif.optimize_for(&options)?);
                }
                _ => {}
            }
        }
        let modified =
            self.settings.resave || (level >= 1 && scan >= Scan::CriticalIssue) || level >= 2;
        Ok(modified || processed_headpart)
    }
}

/// `path` from its first `/meshes/` folder on, `/`-separated, as C++ matched
/// headparts: `meshes/...`. Without one it is the whole path. The search
/// ignores ASCII case, as Qt's did for this ASCII needle.
fn relative_mesh_path(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    // ASCII lowercasing keeps every byte offset, so an index into the folded
    // copy is an index into `normalized`.
    match normalized.to_ascii_lowercase().find("/meshes/") {
        Some(index) => normalized[index + 1..].to_owned(),
        None => normalized,
    }
}

/// Logs what one `OptimizeFor` call did, as C++'s `print_res` did, at the
/// verbose level. `version_mismatch` is left out, as in C++.
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
