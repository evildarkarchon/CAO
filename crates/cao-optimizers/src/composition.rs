//! The composition root: production wiring shared by `cao-gui` and `cao-parity`.
//!
//! Ported from C++ `ApplicationRunSetup` and `ApplicationRunWork`. Given the app
//! directory, the selected profile and the options model, [`ApplicationRun::new`]
//! validates the options, builds the Run Request, refuses one that contradicts
//! the profile's Profile Capabilities, and wires a profile-backed Run
//! Configuration Provider and the production Run Work Service into an
//! Optimization Run Service. Both binaries go through it, so the parity driver
//! exercises the wiring users run.
//!
//! Everything resolves against the app directory the caller passes: `profiles/`
//! and `bin/hkxcmd.exe` here, and later `logs/` (deviation 1). Only `cao-gui`'s
//! `main` derives that directory from the exe.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use cao_archive::{Game, Settings};
use cao_core::Error;
use cao_core::execution::{AssetExecutor, quarantine_failed_load};
use cao_core::routing::{
    ExecutionMode, PolicyValidationError, RequestedWork, RoutingPolicyRequest,
};
use cao_core::run::{
    ArchiveAdapters, ArchiveExtractor, ArchiveFinalization, ArchiveFinalizationSettings,
    AssetRunAdapters, CancellationToken, ModSelection, OptimizationRunService, RunConfiguration,
    RunConfigurationProvider, RunEventDispatcher, RunHandle, RunPreparation, RunRequest,
    RunWorkEvidence, RunWorkMilestones, RunWorkService, SelectedProfileFacts, SourceCleanup,
    StartError, TemporaryArtifactRegistry, execute_asset_run,
};
use cao_profiles::{BsaGame, OptimizationMode, Options, ProfileError, ProfileSettings, Profiles};
use directxtex::DXGI_FORMAT;

use crate::animations::Hkxcmd;
use crate::archives::{ArchiveFileReader, GameArchivePacker, VolumeProbes};
use crate::backend::{OptimizerBackend, TextureResize, TextureSettings};
use crate::textures::TextureProfile;

/// Why the options cannot start a run. Nothing has run and nothing has changed.
///
/// The option checks are C++ `makeApplicationRunRequest`'s; the C++ messages are
/// kept so the GUI can show them unchanged.
#[derive(Debug, thiserror::Error)]
pub enum RunSetupError {
    /// A `mode` other than one mod or several mods, from a hand-edited file.
    #[error("This mode does not exist.")]
    UnsupportedMode(i32),
    #[error("Mesh optimization level must be between 0 and 3.")]
    MeshLevel(i32),
    /// Deviation 18: checked only when resizing by size is enabled.
    #[error("Texture target width and height must be even.")]
    OddTextureSize,
    #[error("Texture resizing requires non-zero width and height ratios.")]
    ZeroTextureRatio,
    #[error("Texture resizing requires non-zero width and height.")]
    ZeroTextureSize,
    /// A relative folder would resolve against the working directory.
    #[error("The selected folder `{}` is not an absolute path.", .0.display())]
    RelativeSelection(PathBuf),
    /// Work whose optimizer is not ported yet. Refusing it up front means a run
    /// never silently skips work the user asked for.
    #[error("{0} is not available in this build.")]
    Unavailable(&'static str),
    /// The request contradicts the selected profile's Profile Capabilities,
    /// such as Mesh work under FO4. Every conflict is kept, in compiler order.
    #[error("{}", conflict_messages(.0))]
    PolicyConflict(Vec<PolicyValidationError>),
    /// The selected profile's `profile.ini` could not be read.
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

/// One line per conflict, worded as C++ `policyValidationErrorMessages` did.
fn conflict_messages(conflicts: &[PolicyValidationError]) -> String {
    conflicts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One run, set up and ready to start: its Run Request and the service that
/// owns its production wiring.
///
/// Keep it alive while the run executes: dropping it cancels and joins any run
/// it started, as the C++ GUI's `_runService` did.
pub struct ApplicationRun {
    service: OptimizationRunService,
    request: RunRequest,
}

impl ApplicationRun {
    /// Sets up a run of `options` under `profile`, in the install at `app_dir`.
    ///
    /// The Run Request takes its intent from `options`, and freezes the
    /// profile's TGA-conversion choice as C++ does: only that is read from
    /// `profile.ini` now. Everything else the run needs from the profile is read
    /// again by its Run Configuration Provider during Preparing, so an edit made
    /// before the run starts is honoured.
    ///
    /// The request is also checked against the profile's Profile Capabilities
    /// now, so a request the profile cannot honour never starts a run. That
    /// check is repeated against the profile Preparing reads.
    ///
    /// # Errors
    /// [`RunSetupError`] when the options are invalid, contradict the
    /// profile's capabilities, ask for work this build cannot do, or the
    /// profile's TGA choice cannot be read.
    pub fn new(app_dir: &Path, profile: &str, options: &Options) -> Result<Self, RunSetupError> {
        let request = run_request(app_dir, profile, options)?;
        check_profile_capabilities(app_dir, profile, &request)?;
        let settings = OptimizerSettings::from_options(options)?;
        let configuration = Arc::new(ProfileConfigurationProvider::new(app_dir));
        let work = Arc::new(ApplicationRunWork {
            settings,
            hkxcmd: Hkxcmd::in_app_dir(app_dir),
            configuration: Arc::clone(&configuration),
            profile: Mutex::new(None),
        });
        Ok(Self {
            service: OptimizationRunService::new(Some(configuration), Some(work)),
            request,
        })
    }

    /// The Run Request this run starts with.
    pub fn request(&self) -> &RunRequest {
        &self.request
    }

    /// Starts the run, sending its events to `dispatcher` on the Run Worker.
    ///
    /// # Errors
    /// The service's synchronous [`StartError`], such as another active run.
    pub fn start(&self, dispatcher: Option<RunEventDispatcher>) -> Result<RunHandle, StartError> {
        self.service.start(self.request.clone(), dispatcher)
    }
}

/// Builds the Run Request for `options` under `profile`, as C++
/// `makeApplicationRunRequest` does.
///
/// Texture work is requested when any Texture option is on; conversion of TGAs
/// joins it when the profile converts them. Mesh work is requested when the
/// level is above 0 or resaving is on; it covers both Standard and Terrain
/// Meshes.
///
/// # Errors
/// [`RunSetupError`] when an option is invalid or `profile.ini` cannot be read.
pub fn run_request(
    app_dir: &Path,
    profile: &str,
    options: &Options,
) -> Result<RunRequest, RunSetupError> {
    validate(options)?;
    let selected = PathBuf::from(&options.user_path);
    // An empty folder is a structural Start Error the service reports itself.
    if !options.user_path.is_empty() && !selected.is_absolute() {
        return Err(RunSetupError::RelativeSelection(selected));
    }
    let mod_selection = match options.mode {
        OptimizationMode::SingleMod => ModSelection::SingleModRoot(selected),
        OptimizationMode::SeveralMods => ModSelection::ChildModRoots(selected),
        OptimizationMode::Unsupported(mode) => return Err(RunSetupError::UnsupportedMode(mode)),
    };

    let optimize_native_textures = options.textures_necessary
        || options.textures_compress
        || options.textures_mipmaps
        || options.textures_resize_size
        || options.textures_resize_ratio;
    // Read only when Texture work is selected; the lenient read matches the C++
    // GUI's, which ignored lines QSettings rejected. Preparing reads strictly.
    let convert_textures = optimize_native_textures
        && Profiles::new(app_dir)
            .open(profile)
            .load_settings()?
            .textures_convert_tga;
    let optimize_meshes = requests_mesh_work(options);
    let choices = [
        (
            optimize_native_textures,
            RequestedWork::NativeTextureOptimization,
        ),
        (
            convert_textures,
            RequestedWork::ConvertibleTextureConversion,
        ),
        (optimize_meshes, RequestedWork::StandardMeshOptimization),
        (optimize_meshes, RequestedWork::TerrainMeshOptimization),
        (
            options.animations_optimization,
            RequestedWork::AnimationOptimization,
        ),
        (options.bsa_extract, RequestedWork::ArchiveExtraction),
        (options.bsa_create, RequestedWork::ArchiveCreation),
    ];
    let work = choices
        .into_iter()
        .filter(|(selected, _)| *selected)
        .map(|(_, work)| work)
        .collect();
    let mode = if options.dry_run {
        ExecutionMode::DryRun
    } else {
        ExecutionMode::Apply
    };
    Ok(RunRequest::new(profile, mode, mod_selection, work))
}

/// Compiles `request`'s Routing Policy against the profile's current Profile
/// Capabilities, as C++ `prepareApplicationRun` does.
///
/// The C++ adapters never called that check before starting, so a
/// contradicting request started and then failed Preparing with a Policy
/// Conflict. Here it is refused before any run exists.
///
/// A profile that cannot be read is not judged here: the run starts and its
/// Preparing reports the failure, as it did before this check existed.
///
/// # Errors
/// [`RunSetupError::PolicyConflict`] with every conflict.
fn check_profile_capabilities(
    app_dir: &Path,
    profile: &str,
    request: &RunRequest,
) -> Result<(), RunSetupError> {
    // Lenient, as the C++ GUI's profile reads were; Preparing reads strictly.
    let Ok(settings) = Profiles::new(app_dir).open(profile).load_settings() else {
        return Ok(());
    };
    profile_facts(&settings)
        .compile_policy(RoutingPolicyRequest::for_work(
            request.execution_mode(),
            request.requested_work(),
        ))
        .map(|_| ())
        .map_err(RunSetupError::PolicyConflict)
}

/// Whether the options ask for Mesh work: a level above 0, or resaving, which
/// is independent of the level, as in C++ `choicesFrom`.
fn requests_mesh_work(options: &Options) -> bool {
    options.meshes_optimization_level > 0 || options.meshes_resave
}

/// C++ `makeApplicationRunRequest`'s numeric checks, in its order.
fn validate(options: &Options) -> Result<(), RunSetupError> {
    if let OptimizationMode::Unsupported(mode) = options.mode {
        return Err(RunSetupError::UnsupportedMode(mode));
    }
    if !(0..=3).contains(&options.meshes_optimization_level) {
        return Err(RunSetupError::MeshLevel(options.meshes_optimization_level));
    }
    if options.textures_resize_size
        && (!options.textures_target_width.is_multiple_of(2)
            || !options.textures_target_height.is_multiple_of(2))
    {
        return Err(RunSetupError::OddTextureSize);
    }
    if options.textures_resize_ratio
        && (options.textures_target_width_ratio == 0 || options.textures_target_height_ratio == 0)
    {
        return Err(RunSetupError::ZeroTextureRatio);
    }
    if options.textures_resize_size
        && (options.textures_target_width == 0 || options.textures_target_height == 0)
    {
        return Err(RunSetupError::ZeroTextureSize);
    }
    Ok(())
}

/// The per-run optimizer settings, taken from the options model when the run is
/// set up, as C++ `OptionsSnapshot` captures them.
#[derive(Debug, Clone, Copy)]
struct OptimizerSettings {
    textures: TextureSettings,
    /// What becomes of an extracted Archive: removed when "delete backup" is
    /// on (`bBsaDeleteBackup`), otherwise kept as a `.bak`.
    extracted_archive_cleanup: SourceCleanup,
    /// Archive Finalization's choices. Whether it packs at all is the Routing
    /// Policy's Archive creation request.
    finalization: ArchiveFinalizationSettings,
}

impl OptimizerSettings {
    /// Captures the options, refusing work whose optimizer is not ported yet.
    fn from_options(options: &Options) -> Result<Self, RunSetupError> {
        if requests_mesh_work(options) {
            return Err(RunSetupError::Unavailable("Mesh optimization"));
        }
        // Ratio wins when both are on, as in `MainOptimizer::optimizeTexture`.
        let resize = if options.textures_resize_ratio {
            TextureResize::Ratio {
                width: options.textures_target_width_ratio,
                height: options.textures_target_height_ratio,
            }
        } else if options.textures_resize_size {
            TextureResize::Size {
                width: options.textures_target_width,
                height: options.textures_target_height,
            }
        } else {
            TextureResize::None
        };
        Ok(Self {
            textures: TextureSettings {
                necessary: options.textures_necessary,
                compress: options.textures_compress,
                mipmaps: options.textures_mipmaps,
                resize,
            },
            extracted_archive_cleanup: if options.bsa_delete_backup {
                SourceCleanup::Remove
            } else {
                SourceCleanup::Backup
            },
            finalization: ArchiveFinalizationSettings {
                compress: options.bsa_compress,
                delete_sources: options.bsa_delete_source,
                create_dummy_plugins: options.bsa_create_dummies,
                merge_incompressible: options.bsa_merge_incompressible,
                merge_textures: options.bsa_merge_textures,
            },
        })
    }
}

/// Locks a mutex, recovering the data if a panicking holder poisoned it. Each
/// holder here writes one whole value, so the data is always consistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What Preparing read from the selected profile, pinned for the whole run.
#[derive(Debug)]
pub struct PreparedProfile {
    /// `profile.ini`.
    pub settings: Arc<ProfileSettings>,
    /// The Packing Exclusion rules of `FilesToNotPack.txt`, as C++'s profile
    /// snapshot loaded them during Preparing.
    pub files_to_not_pack: Vec<String>,
}

/// The Run Configuration Provider over a `profiles/` directory, ported from C++
/// `ApplicationRunConfigurationProvider`.
///
/// Each load re-reads the named profile's `profile.ini`, `ignoredMods.txt`
/// and `FilesToNotPack.txt` during Preparing, on the Run Worker, and
/// publishes what it read so the run's work uses the very same profile facts
/// its routing did.
pub struct ProfileConfigurationProvider {
    profiles: Profiles,
    prepared: Mutex<Option<Arc<PreparedProfile>>>,
}

impl ProfileConfigurationProvider {
    /// A provider for the profiles of the install in `app_dir`.
    pub fn new(app_dir: &Path) -> Self {
        Self {
            profiles: Profiles::new(app_dir),
            prepared: Mutex::new(None),
        }
    }

    /// The settings the last successful load read, or `None` before one.
    pub fn prepared_settings(&self) -> Option<Arc<ProfileSettings>> {
        self.prepared_profile()
            .map(|prepared| Arc::clone(&prepared.settings))
    }

    /// Everything the last successful load read, or `None` before one.
    pub fn prepared_profile(&self) -> Option<Arc<PreparedProfile>> {
        lock(&self.prepared).clone()
    }
}

impl RunConfigurationProvider for ProfileConfigurationProvider {
    fn load(&self, profile_identity: &str) -> Result<RunConfiguration, Error> {
        let failed = |error: ProfileError| Error::ConfigurationLoading(error.to_string());
        let profile = self.profiles.open(profile_identity);
        // Strict, as C++ run setup rejected any QSettings status but NoError.
        let settings = profile.load_settings_checked().map_err(failed)?;
        let ignored_mods = profile.ignored_mods().map_err(failed)?;
        // A missing or unreadable list is empty, as in C++; finalization logs it.
        let files_to_not_pack = profile.files_to_not_pack();
        let facts = profile_facts(&settings);
        // Published only after every read succeeded, so work never sees a
        // partial profile.
        *lock(&self.prepared) = Some(Arc::new(PreparedProfile {
            settings: Arc::new(settings),
            files_to_not_pack,
        }));
        Ok(RunConfiguration {
            profile: facts,
            ignored_mods,
            // Deviation 20: MO2 names its separators `<name>_separator`. C++
            // passed the marker "separator" and excluded any child containing it.
            separator_suffixes: vec![SEPARATOR_SUFFIX.to_owned()],
        })
    }
}

/// The suffix Mod Organizer 2 gives its separator folders.
const SEPARATOR_SUFFIX: &str = "_separator";

/// The Profile Capabilities a profile's settings declare, as C++
/// `ApplicationRunConfigurationProvider` and `factsFromSelectedProfile` map them.
fn profile_facts(settings: &ProfileSettings) -> SelectedProfileFacts {
    let textures = settings.textures_enabled;
    let meshes = settings.meshes_enabled;
    let archives = settings.bsa_enabled;
    SelectedProfileFacts {
        archive_extension: Some(archive_extension(settings.bsa_game).to_owned()),
        supports_native_texture_optimization: textures,
        supports_texture_conversion: textures,
        supports_standard_mesh_optimization: meshes,
        supports_terrain_mesh_optimization: meshes,
        supports_animation_optimization: settings.animations_enabled,
        supports_archive_extraction: archives,
        // Reference maintenance belongs to Texture conversion.
        supports_mesh_reference_maintenance: textures,
        supports_archive_creation: archives,
    }
}

/// The Archive extension of a game, from `cao-archive`'s per-game tables.
fn archive_extension(game: BsaGame) -> &'static str {
    Settings::get(archive_game(game)).extension
}

/// The `cao-archive` game whose tables a profile's `bsaGame` names.
fn archive_game(game: BsaGame) -> Game {
    match game {
        BsaGame::Tes5 => Game::Tes5,
        BsaGame::Sse => Game::Sse,
        BsaGame::Fo4 => Game::Fo4,
    }
}

/// The packer for a profile's game, with its maximum Archive size raised to
/// the profile's `maxBsaUncompressedSize` when that is larger, as C++
/// `archiveSettings` did.
fn archive_packer(settings: &ProfileSettings) -> GameArchivePacker {
    GameArchivePacker::new(
        Settings::get(archive_game(settings.bsa_game))
            .with_profile_max_size(settings.max_bsa_uncompressed_size),
    )
}

/// The profile's Texture settings, as the texture optimizer takes them.
fn texture_profile(settings: &ProfileSettings) -> TextureProfile {
    TextureProfile {
        format: DXGI_FORMAT::from(settings.textures_format),
        unwanted_formats: settings
            .textures_unwanted_formats
            .iter()
            .map(|&format| DXGI_FORMAT::from(format))
            .collect(),
        compress_interface: settings.textures_compress_interface,
    }
}

/// The production Run Work Service, ported from C++ `ApplicationRunWork`.
struct ApplicationRunWork {
    settings: OptimizerSettings,
    /// The app directory's `bin/hkxcmd.exe`; each run's backend gets a fresh
    /// copy, so each run checks for the exe once, as each C++ run did.
    hkxcmd: Hkxcmd,
    configuration: Arc<ProfileConfigurationProvider>,
    /// The provider's Preparing snapshot, pinned for the whole run.
    profile: Mutex<Option<Arc<PreparedProfile>>>,
}

impl RunWorkService for ApplicationRunWork {
    fn prepare(&self) -> Result<(), Error> {
        let prepared = self.configuration.prepared_profile().ok_or_else(|| {
            Error::ConfigurationLoading("The selected profile was not prepared".to_owned())
        })?;
        *lock(&self.profile) = Some(prepared);
        Ok(())
    }

    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let profile = lock(&self.profile)
            .clone()
            .ok_or_else(|| Error::WorkService("The run's profile was not prepared".to_owned()))?;
        let texture_profile = texture_profile(&profile.settings);
        // Created on the first Asset, as C++ creates its MainOptimizer, so a run
        // with nothing to process never sets up an optimizer.
        let mut backend: Option<OptimizerBackend> = None;
        // Declared before `adapters`, which borrows them, so they outlive it.
        let reader = ArchiveFileReader::default();
        let packer = archive_packer(&profile.settings);
        // C++ always wires Archive Finalization, so Apply executes the phase:
        // it packs only when the Routing Policy requests Archive creation, and
        // prunes empty directories either way.
        let finalization = ArchiveFinalization::new(
            &packer,
            &reader,
            &VolumeProbes,
            &VolumeProbes,
            self.settings.finalization,
            &profile.files_to_not_pack,
        );
        let cleanup = self.settings.extracted_archive_cleanup;
        let mut adapters = AssetRunAdapters::new(Box::new(|asset, mod_root, artifacts| {
            let backend = backend.get_or_insert_with(|| {
                // This closure runs on the Run Worker, the thread that must
                // join COM. C++'s texture optimizer threw when it could not,
                // and the Asset Run contained the exception as an unsafe
                // Operation Failure; it contains this panic the same way.
                OptimizerBackend::new(
                    self.settings.textures,
                    texture_profile.clone(),
                    self.hkxcmd.clone(),
                )
                .unwrap_or_else(|error| panic!("{error}"))
            });
            let result = AssetExecutor::new(backend).execute(asset, artifacts, mod_root);
            Ok(quarantine_failed_load(asset, result))
        }));
        adapters.finalize_archive_lifecycle = Some(Box::new(|evidence, artifacts| {
            finalization.run(preparation, evidence, artifacts, &|| stop.is_cancelled())
        }));
        // Dry Run never reads a manifest, so wiring the Archive seams
        // unconditionally is harmless.
        adapters.archives = Some(ArchiveAdapters {
            reader: &reader,
            capacity: &VolumeProbes,
            volume_identity: &VolumeProbes,
            extract: Box::new(|plan, artifacts| {
                let result = ArchiveExtractor::new(&reader, &VolumeProbes)
                    .extract_with_source_cleanup(plan, cleanup, artifacts);
                if result.succeeded() {
                    log::info!(
                        "BSA successfully extracted: {}",
                        plan.archive_path.display()
                    );
                }
                result
            }),
        });
        execute_asset_run(
            preparation,
            evidence,
            artifacts,
            milestones,
            stop,
            &mut adapters,
        )
    }
}
