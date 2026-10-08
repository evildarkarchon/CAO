//! The composition root: production wiring shared by `cao-gui` and `cao-parity`.
//!
//! Ported from C++ `ApplicationRunSetup` and `ApplicationRunWork`. Given the app
//! directory, the selected profile and the options model, [`ApplicationRun::new`]
//! validates the options, builds the Run Request, and wires a profile-backed Run
//! Configuration Provider and the production Run Work Service into an
//! Optimization Run Service. Both binaries go through it, so the parity driver
//! exercises the wiring users run.
//!
//! Everything resolves against the app directory the caller passes: `profiles/`
//! here, and later `logs/` and `bin/hkxcmd.exe` (deviation 1). Only `cao-gui`'s
//! `main` derives that directory from the exe.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use cao_core::Error;
use cao_core::execution::AssetExecutor;
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    AssetRunAdapters, CancellationToken, ModSelection, OptimizationRunService, RunConfiguration,
    RunConfigurationProvider, RunEventDispatcher, RunHandle, RunPreparation, RunRequest,
    RunWorkEvidence, RunWorkMilestones, RunWorkService, SelectedProfileFacts, StartError,
    execute_asset_run,
};
use cao_profiles::{BsaGame, OptimizationMode, Options, ProfileError, ProfileSettings, Profiles};
use directxtex::DXGI_FORMAT;

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
    /// The selected profile's `profile.ini` could not be read.
    #[error(transparent)]
    Profile(#[from] ProfileError),
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
    /// # Errors
    /// [`RunSetupError`] when the options are invalid, ask for work this build
    /// cannot do, or the profile cannot be read.
    pub fn new(app_dir: &Path, profile: &str, options: &Options) -> Result<Self, RunSetupError> {
        let request = run_request(app_dir, profile, options)?;
        let settings = OptimizerSettings::from_options(options)?;
        let configuration = Arc::new(ProfileConfigurationProvider::new(app_dir));
        let work = Arc::new(ApplicationRunWork {
            settings,
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
}

impl OptimizerSettings {
    /// Captures the options, refusing work whose optimizer is not ported yet.
    fn from_options(options: &Options) -> Result<Self, RunSetupError> {
        if requests_mesh_work(options) {
            return Err(RunSetupError::Unavailable("Mesh optimization"));
        }
        if options.animations_optimization {
            return Err(RunSetupError::Unavailable("Animation optimization"));
        }
        // Dry Run skips Archive Finalization, so only Apply would pack.
        if options.bsa_create && !options.dry_run {
            return Err(RunSetupError::Unavailable("Archive creation"));
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

/// The Run Configuration Provider over a `profiles/` directory, ported from C++
/// `ApplicationRunConfigurationProvider`.
///
/// Each load re-reads the named profile's `profile.ini` and `ignoredMods.txt`
/// during Preparing, on the Run Worker, and publishes the settings it read so
/// the run's work uses the very same profile facts its routing did.
pub struct ProfileConfigurationProvider {
    profiles: Profiles,
    prepared: Mutex<Option<Arc<ProfileSettings>>>,
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
        let textures = settings.textures_enabled;
        let meshes = settings.meshes_enabled;
        let archives = settings.bsa_enabled;
        let facts = SelectedProfileFacts {
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
        };
        // Published only after every read succeeded, so work never sees a
        // partial profile.
        *lock(&self.prepared) = Some(Arc::new(settings));
        Ok(RunConfiguration {
            profile: facts,
            ignored_mods,
            // C++'s marker. Deviation 20 replaces it with the `_separator`
            // suffix when Several Mods selection is ported (#486).
            separator_markers: vec!["separator".to_owned()],
        })
    }
}

/// The Archive extension of a game, from bethutil's per-game tables. It moves
/// to `cao-archive` with the rest of those tables.
fn archive_extension(game: BsaGame) -> &'static str {
    match game {
        BsaGame::Tes5 | BsaGame::Sse => ".bsa",
        BsaGame::Fo4 => ".ba2",
    }
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
    configuration: Arc<ProfileConfigurationProvider>,
    /// The provider's Preparing snapshot, pinned for the whole run.
    profile: Mutex<Option<Arc<ProfileSettings>>>,
}

impl RunWorkService for ApplicationRunWork {
    fn prepare(&self) -> Result<(), Error> {
        let prepared = self.configuration.prepared_settings().ok_or_else(|| {
            Error::ConfigurationLoading("The selected profile was not prepared".to_owned())
        })?;
        *lock(&self.profile) = Some(prepared);
        Ok(())
    }

    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let profile = lock(&self.profile)
            .clone()
            .ok_or_else(|| Error::WorkService("The run's profile was not prepared".to_owned()))?;
        let texture_profile = texture_profile(&profile);
        // Created on the first Asset, as C++ creates its MainOptimizer, so a run
        // with nothing to process never sets up an optimizer.
        let mut backend: Option<OptimizerBackend> = None;
        let mut adapters = AssetRunAdapters::new(Box::new(|asset, mod_root| {
            let backend = backend.get_or_insert_with(|| {
                OptimizerBackend::new(self.settings.textures, texture_profile.clone())
            });
            Ok(AssetExecutor::new(backend).execute(asset, mod_root))
        }));
        execute_asset_run(preparation, evidence, milestones, stop, &mut adapters)
    }
}
