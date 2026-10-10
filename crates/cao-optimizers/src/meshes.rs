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
use nifly_sys::{LoadOptions, Nif, NifError, NifVersion, OptimizeOptions, OptimizeReport};

use crate::headparts::HeadpartList;

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

impl MeshSettings {
    /// Whether Headpart Meshes get their own rule: from the necessary level
    /// up, as in C++. Below it a run needs no headpart list.
    pub fn recognises_headparts(&self) -> bool {
        self.level >= 1
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
    /// saved again. With head parts off, a Headpart Mesh gets no `OptimizeFor`
    /// but is still saved when its level says so, as in C++.
    ///
    /// Dry Run evaluates the same levels without touching `nif`, the facegen
    /// rule included (deviation 17).
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

        if mode == ExecutionMode::DryRun {
            if is_headpart && self.settings.headparts {
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
        if is_headpart {
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

/// The `/`-separated game path of the Mesh at `path` within `mod_root`, such
/// as `meshes/actors/hair.nif` (deviation 17).
///
/// Discovery joins every path it finds onto its canonical Mod Root, so the
/// prefix strips; a path spelled otherwise is resolved first, as the Asset Run
/// resolved it to attribute the attempt. A path still outside `mod_root`, or
/// any path when `mod_root` is empty (a standalone call), is matched whole.
fn game_path(path: &Path, mod_root: &Path) -> String {
    let within = |path: &Path| {
        (!mod_root.as_os_str().is_empty())
            .then(|| path.strip_prefix(mod_root).ok().map(Path::to_path_buf))
            .flatten()
    };
    let relative = within(path)
        .or_else(|| {
            cao_winfs::msvc_weakly_canonical(path)
                .ok()
                .and_then(|resolved| within(&resolved))
        })
        .unwrap_or_else(|| path.to_path_buf());
    relative.to_string_lossy().replace('\\', "/")
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
