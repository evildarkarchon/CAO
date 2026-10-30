//! The options form into a Run Request.
//!
//! Given the selected profile and the options model, [`run_request`] validates
//! the options and builds each run's Run Request. It reads no profile file
//! (#570): given the Profile Capabilities the caller already holds, as the GUI
//! holds those of the `profile.ini` it loaded, it holds back the work they do
//! not support, and given none it leaves the profile to Preparing. Preparing's
//! read of the profile always has the last word, even over capabilities a
//! request was built with.
//!
//! Which options ask for which Requested Work is stated once, as the form's
//! [`WorkGroup`]s. A caller asks the module the same questions a run does:
//! [`admission`] builds a run's Admission Rules from its mode and the Profile
//! Capabilities the caller holds, and [`WorkGroup::held_back`] answers
//! whether those rules hold a whole group back.

use std::path::PathBuf;

use cao_core::admission::Admission;
use cao_core::routing::{ExecutionMode, ProfileCapabilities, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationChoices, MeshChoices, MeshLevel, ModSelection, ResizeRatio, ResizeSize,
    RunRequest, SourceCleanup, TextureChoices, TextureResizeChoice, WorkChoiceError, WorkChoices,
};
use cao_profiles::{OptimizationMode, Options};

/// Why the options cannot start a run. Nothing has run and nothing has changed.
///
/// The form's mode and folder are checked here; every Work Choice invariant is
/// checked only by its core constructor, whose [`WorkChoiceError`] this wraps.
/// The GUI shows these messages as they are in its Start Error box, so each
/// names the problem in the form's own terms.
#[derive(Debug, thiserror::Error)]
pub enum RunSetupError {
    /// A `mode` other than one mod or several mods, from a hand-edited file.
    #[error("This mode does not exist.")]
    UnsupportedMode(i32),
    /// A relative folder would resolve against the working directory.
    #[error("The selected folder `{}` is not an absolute path.", .0.display())]
    RelativeSelection(PathBuf),
    /// A Work Choice the form holds cannot be built: a Mesh level outside 0
    /// to 3, or a zero ratio or a zero or odd size for the resize mode the
    /// run uses.
    #[error(transparent)]
    WorkChoice(#[from] WorkChoiceError),
}

/// A part of the options form that asks for Requested Work: the one table of
/// which options request which work.
///
/// Every [`RequestedWork`] belongs to exactly one group. Mesh Reference
/// Maintenance and Texture conversion belong to none: they are no Requested
/// Work, since the form never asks for them and Preparing derives them from
/// the Conversion Preference.
///
/// It is a code name for parts of the form, not a domain term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkGroup {
    /// Native Texture optimization, chosen by any Texture option.
    Textures,
    /// Standard and Terrain Mesh optimization, chosen by a Mesh level above 0
    /// or by resaving.
    ///
    /// Mesh Reference Maintenance is not in this group and does not enable
    /// the Meshes tab: Texture conversion drives it, and nothing on that tab
    /// affects it.
    Meshes,
    /// Animation optimization, chosen by the Animations option.
    Animations,
    /// Archive extraction, chosen by Extract BSA. Delete backups is one of its
    /// Work Choices.
    ArchiveExtraction,
    /// Archive creation, chosen by Create BSA.
    ArchiveCreation,
}

impl WorkGroup {
    /// Every work group, in the order the form's tabs show them.
    pub const ALL: [Self; 5] = [
        Self::Textures,
        Self::Meshes,
        Self::Animations,
        Self::ArchiveExtraction,
        Self::ArchiveCreation,
    ];

    /// Whether `options` choose this group, so that a Run Request built from
    /// them asks for its [`Self::work`].
    pub fn chosen_by(self, options: &Options) -> bool {
        match self {
            Self::Textures => {
                options.textures_necessary
                    || options.textures_compress
                    || options.textures_mipmaps
                    || options.textures_resize_size
                    || options.textures_resize_ratio
            }
            Self::Meshes => requests_mesh_work(options),
            Self::Animations => options.animations_optimization,
            Self::ArchiveExtraction => options.bsa_extract,
            Self::ArchiveCreation => options.bsa_create,
        }
    }

    /// The Requested Work this group asks for when the options choose it.
    pub fn work(self) -> &'static [RequestedWork] {
        match self {
            Self::Textures => &[RequestedWork::NativeTextureOptimization],
            Self::Meshes => &[
                RequestedWork::StandardMeshOptimization,
                RequestedWork::TerrainMeshOptimization,
            ],
            Self::Animations => &[RequestedWork::AnimationOptimization],
            Self::ArchiveExtraction => &[RequestedWork::ArchiveExtraction],
            Self::ArchiveCreation => &[RequestedWork::ArchiveCreation],
        }
    }

    /// Whether `admission` holds this group back: true only when it admits
    /// none of the group's [`Self::work`].
    ///
    /// A partly supported group is not held back, since a Run Request asking
    /// for it carries its admitted part. An `admission` built without Profile
    /// Capabilities holds back no group.
    pub fn held_back(self, admission: &Admission) -> bool {
        !self.work().iter().any(|work| admission.admits(*work))
    }
}

/// The Admission Rules of a run in `mode`, under the Profile Capabilities the
/// caller holds: the one place a mode becomes "Several Mods" for Admission.
///
/// An [`OptimizationMode::Unsupported`] mode counts as not Several Mods, so it
/// caps no Mesh level; [`run_request`] rejects that mode with
/// [`RunSetupError::UnsupportedMode`] before it admits anything. `capabilities` of `None` hold back no work, and
/// Preparing judges the Profile.
pub fn admission(mode: OptimizationMode, capabilities: Option<&ProfileCapabilities>) -> Admission {
    Admission::new(mode == OptimizationMode::SeveralMods, capabilities)
}

/// Builds the Run Request for `options` under the profile identity `profile`:
/// a pure function of the caller's intent that reads no profile file (#570).
///
/// The request asks for the work of each [`WorkGroup`] the options choose.
/// Texture work is requested when any Texture option is on. Conversion of
/// TGAs is never requested: Routing Policy compilation derives it during
/// Preparing from the Conversion Preference of the profile read there. Mesh
/// work is requested when the level is above 0 or resaving is on; it covers
/// both Standard and Terrain Meshes.
///
/// `capabilities` are the Profile Capabilities the caller already holds for
/// `profile`, such as the GUI's from the `profile.ini` it loaded. Through
/// core's [`Admission`], the request holds back the requested work they do
/// not support, as its Several Mods rule holds back Mesh levels above
/// Necessary; a held-back choice is neither a Start Error nor a Policy
/// Conflict. A Dry Run holds back nothing: its request keeps the Archive work
/// chosen, and its run mutates no Archive.
/// `None` holds back nothing for capabilities, so the profile Preparing reads
/// judges the request, and work it does not support fails the run there with
/// a Policy Conflict. So do capabilities that went stale: Preparing's read
/// always has the last word.
///
/// The request also carries the form's Work Choices: its Texture, Mesh,
/// Archive extraction and Archive Finalization choices, with ratio winning
/// when both resize modes are on. They are carried whether or not their work
/// is requested; requested work alone decides routing.
///
/// # Errors
/// The first [`RunSetupError`] found. The form's structure is checked first,
/// its mode and then its selected folder, and its Work Choices only after
/// that, in the order [`work_choices`] builds them.
pub fn run_request(
    profile: &str,
    options: &Options,
    capabilities: Option<&ProfileCapabilities>,
) -> Result<RunRequest, RunSetupError> {
    let mod_selection: fn(PathBuf) -> ModSelection = match options.mode {
        OptimizationMode::SingleMod => ModSelection::SingleModRoot,
        OptimizationMode::SeveralMods => ModSelection::ChildModRoots,
        OptimizationMode::Unsupported(mode) => return Err(RunSetupError::UnsupportedMode(mode)),
    };
    let selected = PathBuf::from(&options.user_path);
    // An empty folder is a structural Start Error the service reports itself.
    if !options.user_path.is_empty() && !selected.is_absolute() {
        return Err(RunSetupError::RelativeSelection(selected));
    }
    let mod_selection = mod_selection(selected);
    let work_choices = work_choices(options)?;

    let work: Vec<RequestedWork> = WorkGroup::ALL
        .into_iter()
        .filter(|group| group.chosen_by(options))
        .flat_map(|group| group.work().iter().copied())
        .collect();
    let mode = ExecutionMode::from_dry_run(options.dry_run);
    // Only the capability rule holds back requested work. The Several Mods
    // rule caps the Mesh level, which `with_work_choices` applies to every
    // request itself, so applying it here too changes nothing.
    let work = admission(options.mode, capabilities).admitted_work(&work);
    Ok(RunRequest::new(profile, mode, mod_selection, work).with_work_choices(work_choices))
}

/// The Work Choices of `options`, each built by its core constructor, the only
/// place its invariant is checked.
///
/// Ratio wins when both resize modes are on: the form lets both be on, and a
/// run resizes one way. Only the resize mode the run uses is checked, so the
/// unused one may hold any value without keeping the run from starting.
///
/// # Errors
/// The first [`WorkChoiceError`] found: the resize choice's, then the Mesh
/// level's.
fn work_choices(options: &Options) -> Result<WorkChoices, WorkChoiceError> {
    let resize = if options.textures_resize_ratio {
        TextureResizeChoice::Ratio(ResizeRatio::new(
            options.textures_target_width_ratio,
            options.textures_target_height_ratio,
        )?)
    } else if options.textures_resize_size {
        TextureResizeChoice::Size(ResizeSize::new(
            options.textures_target_width,
            options.textures_target_height,
        )?)
    } else {
        TextureResizeChoice::None
    };
    Ok(WorkChoices {
        textures: TextureChoices {
            necessary: options.textures_necessary,
            compress: options.textures_compress,
            mipmaps: options.textures_mipmaps,
            resize,
        },
        meshes: MeshChoices {
            level: MeshLevel::new(options.meshes_optimization_level)?,
            headparts: options.meshes_headparts,
            resave: options.meshes_resave,
        },
        // "Delete backup" (`bBsaDeleteBackup`) removes an extracted Archive;
        // otherwise it is kept as a `.bak`.
        archive_extraction: if options.bsa_delete_backup {
            SourceCleanup::Remove
        } else {
            SourceCleanup::Backup
        },
        archive_finalization: ArchiveFinalizationChoices {
            compress: options.bsa_compress,
            delete_sources: options.bsa_delete_source,
            create_dummy_plugins: options.bsa_create_dummies,
            merge_incompressible: options.bsa_merge_incompressible,
            merge_textures: options.bsa_merge_textures,
        },
    })
}

/// Whether the options ask for Mesh work: a level above 0, or resaving, which
/// is independent of the level, so resaving alone at level 0 still asks for
/// it.
fn requests_mesh_work(options: &Options) -> bool {
    options.meshes_optimization_level > 0 || options.meshes_resave
}
