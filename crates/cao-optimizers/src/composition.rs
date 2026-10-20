//! The composition root: production wiring shared by `cao-gui` and `cao-parity`.
//!
//! Ported from C++ `ApplicationRunSetup` and `ApplicationRunWork`. Given the app
//! directory, the selected profile and the options model, [`ApplicationRun::new`]
//! validates the options, builds the Run Request, and wires a profile-backed Run
//! Configuration Provider into an Optimization Run Service. During each run's
//! Preparing, that provider reads the run's `SelectedProfile` once and builds
//! both the Run Configuration the run's Routing Policy compiles from and the
//! production Run Toolset from it. Both binaries go through it, so
//! the parity driver exercises the wiring users run.
//!
//! Everything resolves against the app directory the caller passes: `profiles/`
//! and `bin/hkxcmd.exe` here, and later `logs/` (deviation 1). Only `cao-gui`'s
//! `main` derives that directory from the exe.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::Error;
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveExtractionAdapter, ArchiveFinalizationAdapter, ArchiveFinalizationSettings,
    AssetAttemptAdapter, AssetBackendFactory, BackendCreation, BackendCreationError,
    CancellationToken, ModSelection, OptimizationRunService, RunConfiguration,
    RunConfigurationProvider, RunDiagnostic, RunDiagnosticCode, RunEventDispatcher, RunHandle,
    RunPhase, RunRequest, RunToolset, SourceCleanup, StartError,
};
use cao_profiles::{OptimizationMode, Options, Profiles};

use crate::animations::Hkxcmd;
use crate::archives::{ArchiveFileReader, GameArchivePacker, VolumeProbes};
use crate::backend::{OptimizerBackend, TextureResize, TextureSettings};
use crate::headparts::{HeadpartList, ScanCancelled, scan_headparts};
use crate::meshes::{MeshOptimizer, MeshSettings};
use crate::selected_profile::SelectedProfile;

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
    /// The Run Request takes its intent from `options` alone. Everything the
    /// run needs from the profile, including the Conversion Preference that
    /// decides TGA conversion, is read by its Run Configuration Provider during
    /// Preparing, so an edit made before the run starts is honoured.
    ///
    /// The request is not checked against the profile's Profile Capabilities
    /// here: a request the profile cannot honour sets up, and its run fails
    /// Preparing with a Policy Conflict carrying every conflict (#570).
    ///
    /// # Errors
    /// [`RunSetupError`] when the options are invalid.
    pub fn new(app_dir: &Path, profile: &str, options: &Options) -> Result<Self, RunSetupError> {
        let request = run_request(profile, options)?;
        let configuration = Arc::new(ProfileConfigurationProvider::for_runs(
            app_dir,
            RunWorkSettings {
                settings: OptimizerSettings::from_options(options),
                selected_folder: request.mod_selection().directory().to_path_buf(),
                hkxcmd: Hkxcmd::in_app_dir(app_dir),
            },
        ));
        Ok(Self {
            // The provider's Run Toolset does the work.
            service: OptimizationRunService::new(Some(configuration)),
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
/// Texture work is requested when any Texture option is on. TGA conversion is
/// never requested here: Preparing adds it to that Texture work when the
/// Selected Profile's Conversion Preference is on. Mesh work is requested when
/// the level is above 0 or resaving is on; it covers both Standard and Terrain
/// Meshes. No profile is read.
///
/// # Errors
/// [`RunSetupError`] when an option is invalid.
fn run_request(profile: &str, options: &Options) -> Result<RunRequest, RunSetupError> {
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
    let optimize_meshes = requests_mesh_work(options);
    let choices = [
        (
            optimize_native_textures,
            RequestedWork::NativeTextureOptimization,
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
    meshes: MeshSettings,
    /// What becomes of an extracted Archive: removed when "delete backup" is
    /// on (`bBsaDeleteBackup`), otherwise kept as a `.bak`.
    extracted_archive_cleanup: SourceCleanup,
    /// Archive Finalization's choices. Whether it packs at all is the Routing
    /// Policy's Archive creation request.
    finalization: ArchiveFinalizationSettings,
}

impl OptimizerSettings {
    /// Captures the options, which [`validate`] has already checked.
    ///
    /// # Panics
    /// If the mesh level is outside 0 to 3, which `validate` rejects first.
    fn from_options(options: &Options) -> Self {
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
        Self {
            textures: TextureSettings {
                necessary: options.textures_necessary,
                compress: options.textures_compress,
                mipmaps: options.textures_mipmaps,
                resize,
            },
            meshes: MeshSettings {
                level: u8::try_from(options.meshes_optimization_level)
                    .expect("validate() keeps the mesh level within 0 to 3"),
                headparts: options.meshes_headparts,
                resave: options.meshes_resave,
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
        }
    }
}

/// The Run Configuration Provider over a `profiles/` directory, ported from C++
/// `ApplicationRunConfigurationProvider`.
///
/// Each load reads the run's [`SelectedProfile`] once during Preparing, on
/// the Run Worker, and builds both the Run Configuration and the run's
/// production Run Toolset from that one read, so the run's work uses the very
/// same profile facts its routing did.
struct ProfileConfigurationProvider {
    profiles: Profiles,
    /// The per-run choices the Run Toolset is built with.
    work: RunWorkSettings,
}

impl ProfileConfigurationProvider {
    /// A provider for the profiles of the install in `app_dir` that builds
    /// each run's production Run Toolset with `work`.
    fn for_runs(app_dir: &Path, work: RunWorkSettings) -> Self {
        Self {
            profiles: Profiles::new(app_dir),
            work,
        }
    }
}

impl RunConfigurationProvider for ProfileConfigurationProvider {
    /// Reads the Selected Profile once and builds the Run Configuration and
    /// the run's production Run Toolset from that read. Built only after every
    /// read succeeded, so work never sees a partial profile.
    fn load(
        &self,
        profile_identity: &str,
    ) -> Result<(RunConfiguration, Option<RunToolset>), Error> {
        let profile = SelectedProfile::read(&self.profiles, profile_identity)
            .map_err(|error| Error::ConfigurationLoading(error.to_string()))?;
        let configuration = RunConfiguration {
            profile: profile.capabilities().clone(),
            conversion_preference: profile.conversion_preference(),
            ignored_mods: profile.ignored_mods().to_vec(),
            // Deviation 20: MO2 names its separators `<name>_separator`. C++
            // passed the marker "separator" and excluded any child containing it.
            separator_suffixes: vec![SEPARATOR_SUFFIX.to_owned()],
        };
        let toolset = self.work.toolset(Arc::new(profile));
        Ok((configuration, Some(toolset)))
    }
}

/// The suffix Mod Organizer 2 gives its separator folders.
const SEPARATOR_SUFFIX: &str = "_separator";

/// The per-run choices the production Run Toolset is built with, ported from
/// what C++ `ApplicationRunWork` held.
#[derive(Clone)]
struct RunWorkSettings {
    settings: OptimizerSettings,
    /// The selected folder, which the headpart scan walks whole: C++ scanned
    /// `userPath`, so mods with a Mod Exclusion are scanned too.
    selected_folder: PathBuf,
    /// The app directory's `bin/hkxcmd.exe`; each run's backend gets a fresh
    /// copy, so each run checks for the exe once, as each C++ run did.
    hkxcmd: Hkxcmd,
}

impl RunWorkSettings {
    /// The production Run Toolset of one run, over the profile its Preparing
    /// read.
    fn toolset(&self, profile: Arc<SelectedProfile>) -> RunToolset {
        // One reader serves discovery, extraction and finalization for the
        // whole run.
        let reader = Arc::new(ArchiveFileReader::default());
        let probes = Arc::new(VolumeProbes);
        // C++ always wires Archive Finalization, so Apply executes the phase:
        // it packs only when the Routing Policy requests Archive creation, and
        // prunes empty directories either way.
        let finalization = ArchiveFinalizationAdapter::new(
            // The packer names Archives by the same Archive settings value the
            // run's routing took its extension from.
            Box::new(GameArchivePacker::new(*profile.archive_settings())),
            reader.clone(),
            probes.clone(),
            probes.clone(),
            self.settings.finalization,
            profile.packing_exclusions().to_vec(),
        );
        RunToolset {
            // The adapter creates the backend on the first Asset, as C++
            // creates its MainOptimizer, so a run with nothing to process
            // never sets up an optimizer.
            asset_attempt: Box::new(AssetAttemptAdapter::new(OptimizerBackendFactory {
                work: self.clone(),
                profile,
            })),
            archive_extraction: Box::new(ArchiveExtractionAdapter::new(
                reader.clone(),
                probes.clone(),
                self.settings.extracted_archive_cleanup,
            )),
            archive_finalization: Box::new(finalization),
            archive_reader: reader,
            capacity: probes.clone(),
            volume_identity: probes,
        }
    }
}

/// The production backend factory: it lists the run's Headpart Meshes, then
/// creates the [`OptimizerBackend`] on the Run Worker.
///
/// The Asset attempt adapter calls it on the first Routed Asset, after
/// Archive extraction, so the scan sees any plugin extraction produced.
struct OptimizerBackendFactory {
    /// This run's copy of the per-run choices: the selected folder, which the
    /// headpart scan walks whole, and this run's copy of the app directory's
    /// `hkxcmd.exe`.
    work: RunWorkSettings,
    /// The profile Preparing read, which the backend's Texture profile, Mesh
    /// target and custom Headpart Meshes come from.
    profile: Arc<SelectedProfile>,
}

impl OptimizerBackendFactory {
    /// The run's Headpart Meshes, from the profile's `customHeadparts.txt` and
    /// every plugin under the selected folder, as C++ `listHeadparts` gathered
    /// them.
    ///
    /// Only the necessary mesh level and above consult the list, so a run
    /// below it scans nothing; C++ scanned regardless, which no run fact
    /// showed. Each plugin that cannot be read is returned as a
    /// `PluginUnreadable` Run Diagnostic for the run to retain, and the scan
    /// carries on (deviation 17).
    ///
    /// # Errors
    /// [`ScanCancelled`] when `stop` is cancelled during the scan, as C++'s
    /// scan threw it.
    fn list_headparts(
        &self,
        stop: &CancellationToken,
    ) -> Result<(HeadpartList, Vec<RunDiagnostic>), ScanCancelled> {
        if !self.work.settings.meshes.recognises_headparts() {
            return Ok((HeadpartList::default(), Vec::new()));
        }
        let profile = &self.profile;
        // The Archive rules' plugin extensions, so Loading Plugins and
        // Headpart plugins are judged by one game table.
        let extensions = profile.archive_settings().plugin_extensions;
        let scan = scan_headparts(
            profile.custom_headparts(),
            &self.work.selected_folder,
            extensions,
            &|| stop.is_cancelled(),
        )?;
        let diagnostics = scan
            .unreadable
            .into_iter()
            .map(|plugin| {
                RunDiagnostic::new(
                    RunDiagnosticCode::PluginUnreadable,
                    RunPhase::ProcessingAssets,
                    format!(
                        "The plugin's headparts cannot be read, so it names no Headpart Mesh: {}",
                        plugin.error
                    ),
                )
                .with_path(plugin.path)
            })
            .collect();
        Ok((scan.headparts, diagnostics))
    }
}

impl AssetBackendFactory for OptimizerBackendFactory {
    type Backend = OptimizerBackend;

    /// Lists the Headpart Meshes, then creates the backend that recognises them.
    fn create(
        &mut self,
        stop: &CancellationToken,
    ) -> Result<BackendCreation<OptimizerBackend>, BackendCreationError> {
        // C++ listed Headpart Meshes as it built its MainOptimizer for the
        // first Asset, so the scan sees any plugin extraction produced.
        let Ok((headparts, diagnostics)) = self.list_headparts(stop) else {
            return Ok(BackendCreation::Cancelled);
        };
        let profile = &self.profile;
        let meshes = MeshOptimizer::new(self.work.settings.meshes, profile.mesh_target())
            .with_headparts(headparts);
        // The adapter calls this on the Run Worker, the thread that must join
        // COM. C++'s texture optimizer threw when it could not, and its run
        // contained the exception as an unsafe Operation Failure; the adapter
        // contains this error the same way. The scan's diagnostics travel with
        // the error, so the run still retains the plugins it could not read.
        let backend = match OptimizerBackend::new(
            self.work.settings.textures,
            profile.texture_profile().clone(),
            meshes,
            self.work.hkxcmd.clone(),
        ) {
            Ok(backend) => backend,
            Err(error) => {
                return Err(BackendCreationError::new(error).with_diagnostics(diagnostics));
            }
        };
        Ok(BackendCreation::Created {
            backend,
            diagnostics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::join_single_threaded_apartment;

    /// A plugin whose HDPT group claims more bytes than the file holds: an
    /// empty TES4 record, then a group header naming HDPT with a size of 1000.
    fn truncated_headpart_plugin() -> Vec<u8> {
        let mut bytes = vec![0; 48];
        bytes[..4].copy_from_slice(b"TES4");
        bytes[24..28].copy_from_slice(b"GRUP");
        bytes[28..32].copy_from_slice(&1000_u32.to_le_bytes());
        bytes[32..36].copy_from_slice(b"HDPT");
        bytes
    }

    /// Spec #546 review: when the backend cannot be created after the
    /// Headpart Mesh scan, the plugins the scan could not read are still
    /// handed back with the error, so the run retains them as it did when the
    /// backend's failure was a panic.
    ///
    /// COM fails on a thread that already joined a single-threaded apartment,
    /// which is the one backend failure a test can reach. This is a unit test
    /// because only the factory, not a run, can be put on such a thread.
    #[test]
    fn a_backend_failure_still_hands_back_the_unreadable_plugins() {
        let app_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let profiles = Profiles::new(&app_dir);
        let mut options = profiles
            .open("SSE")
            .load_options(&Options::default())
            .unwrap();
        options.meshes_optimization_level = 1;
        let selection = std::env::temp_dir().join("cao-optimizers-unit/backend-failure");
        // A missing directory is the expected case; anything else surfaces below.
        let _ = std::fs::remove_dir_all(&selection);
        std::fs::create_dir_all(&selection).unwrap();
        std::fs::write(selection.join("Broken.esp"), truncated_headpart_plugin()).unwrap();
        let mut factory = OptimizerBackendFactory {
            work: RunWorkSettings {
                settings: OptimizerSettings::from_options(&options),
                selected_folder: selection.clone(),
                hkxcmd: Hkxcmd::in_app_dir(&app_dir),
            },
            profile: Arc::new(SelectedProfile::read(&profiles, "SSE").unwrap()),
        };

        // The thread exits after the attempt, leaving its apartment; neither
        // the backend nor the error is `Send`, so only their facts leave it.
        let created = std::thread::spawn(move || {
            join_single_threaded_apartment();
            match factory.create(&CancellationToken::new()) {
                Ok(_) => Ok(()),
                Err(failure) => Err((failure.error.to_string(), failure.diagnostics)),
            }
        })
        .join()
        .unwrap();

        let (message, diagnostics) = created.expect_err("COM cannot join the MTA on an STA thread");
        assert!(message.contains("Failed to initialize COM"), "{message}");
        let reported: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code, diagnostic.path.clone()))
            .collect();
        assert_eq!(
            reported,
            [(
                RunDiagnosticCode::PluginUnreadable,
                selection.join("Broken.esp")
            )]
        );
    }
}
