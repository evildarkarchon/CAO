//! The composition root: `cao-gui`'s production wiring.
//!
//! Given the app directory, [`run_service`] wires a profile-backed Run
//! Configuration Provider into the Optimization Run Service that serves a
//! whole session. It holds no options-form rules: each run's Run Request is
//! built by [`crate::run_request`], and carries all the run's intent.
//!
//! During each run's Preparing, the provider builds the production Run Toolset
//! from the same profile read as the run's Routing Policy and from the run's
//! Run Request, and a request that contradicts the profile's Profile
//! Capabilities fails there with a Policy Conflict. That read always has the
//! last word, even over capabilities a request was built with. The
//! integration tests go through them too, so they exercise the wiring users
//! run.
//!
//! Everything resolves against the app directory the caller passes: `profiles/`
//! and `bin/hkxcmd.exe` here, and later `logs/` (deviation 1). Only `cao-gui`'s
//! `main` derives that directory from the exe.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_archive::Settings;
use cao_core::Error;
use cao_core::routing::ExecutionMode;
use cao_core::run::{
    ArchiveSeams, AssetAttemptAdapter, AssetBackendFactory, BackendCreation, BackendCreationError,
    CancellationToken, MeshChoices, OptimizationRunService, RunConfigurationProvider,
    RunDiagnostic, RunDiagnosticCode, RunPhase, RunRequest, RunToolset, SelectedProfile,
    TextureChoices, TextureResizeChoice,
};
use cao_profiles::{ProfileError, ProfileSettings, Profiles};

use crate::animations::Hkxcmd;
use crate::archives::{ArchiveFileReader, GameArchivePacker, VolumeProbes};
use crate::backend::{OptimizerBackend, TextureResize, TextureSettings};
use crate::headparts::{HeadpartList, ScanCancelled, scan_headparts};
use crate::meshes::{MeshOptimizer, MeshSettings};
use crate::profile::{mesh_target, rules_and_capabilities, texture_profile};

/// The production Optimization Run Service for the install at `app_dir`,
/// wired with the profile-backed Run Configuration Provider.
///
/// One service serves every run of a session: the provider captures nothing
/// per run, so each run's request, built by
/// [`run_request`](crate::run_request::run_request), carries all its
/// intent. Nothing is read from the profile now. Everything a run needs from
/// the profile, including its Conversion Preference, is read once by the
/// provider during that run's Preparing, so an edit made before the run starts
/// is honoured. A request the profile's Profile Capabilities cannot honour
/// still starts, and its Preparing fails with a Policy Conflict Run Failure.
///
/// Keep the service alive while its runs execute: dropping it cancels and
/// joins every run it still retains (ADR-0001), as the C++ GUI's
/// `_runService` did.
pub fn run_service(app_dir: &Path) -> OptimizationRunService {
    // The provider's Run Toolset does the work.
    OptimizationRunService::new(Some(Arc::new(ProfileConfigurationProvider::new(app_dir))))
}

/// The backend's Texture settings for a run's Texture choices.
///
/// The choice already holds one resize mode at most, since
/// [`run_request`](crate::run_request::run_request) let ratio win, and its
/// constructors kept ratios and sizes non-zero.
fn texture_settings(choices: &TextureChoices) -> TextureSettings {
    let resize = match choices.resize {
        TextureResizeChoice::None => TextureResize::None,
        TextureResizeChoice::Ratio(ratio) => TextureResize::Ratio {
            width: ratio.width(),
            height: ratio.height(),
        },
        TextureResizeChoice::Size(size) => TextureResize::Size {
            width: size.width(),
            height: size.height(),
        },
    };
    TextureSettings {
        necessary: choices.necessary,
        compress: choices.compress,
        mipmaps: choices.mipmaps,
        resize,
    }
}

/// The backend's Mesh settings for a run's Mesh choices. The level is
/// already within 0 to 3, so nothing here can fail.
fn mesh_settings(choices: &MeshChoices) -> MeshSettings {
    MeshSettings {
        level: choices.level.get(),
        headparts: choices.headparts,
        resave: choices.resave,
    }
}

/// What Preparing read from the selected profile for the backend and the
/// packer, pinned for the whole run. Core's share of the same read is the
/// [`SelectedProfile`]; these facts stay private to the production provider,
/// so core never learns DXGI formats or NIF versions.
#[derive(Debug)]
struct PreparedProfile {
    /// `profile.ini`.
    settings: Arc<ProfileSettings>,
    /// The Profile's Archive Rules, built once from `settings`: the same
    /// value the Selected Profile's Archive extension came from, so the
    /// packer and the headpart scan follow exactly the rules routing used.
    archive_rules: Settings,
    /// The Headpart Meshes `customHeadparts.txt` lists, loaded with
    /// `FilesToNotPack.txt`, as C++'s profile snapshot loaded them.
    custom_headparts: Vec<String>,
}

/// The Run Configuration Provider over a `profiles/` directory, ported from C++
/// `ApplicationRunConfigurationProvider`.
///
/// Each load re-reads the profile the run's Run Request names
/// (`profile.ini`, `ignoredMods.txt`, `FilesToNotPack.txt` and
/// `customHeadparts.txt`) during Preparing, on the Run Worker. From that one
/// read it builds the run's [`SelectedProfile`] and its production Run
/// Toolset, so the run's work uses the very same profile facts its routing
/// did. The run's Work Choices and Mod Selection come from the same request,
/// so the provider holds no per-run state and can serve any number of runs.
pub struct ProfileConfigurationProvider {
    profiles: Profiles,
    /// The app directory's `bin/hkxcmd.exe`. It is never used to convert:
    /// each run's backend gets a fresh copy, so each run checks for the exe
    /// once, as each C++ run did.
    hkxcmd: Hkxcmd,
}

impl ProfileConfigurationProvider {
    /// A provider for the profiles and `hkxcmd.exe` of the install in
    /// `app_dir`.
    ///
    /// It reads nothing now: each run's profile, Work Choices and Mod
    /// Selection are read at load, from that run's Run Request.
    pub fn new(app_dir: &Path) -> Self {
        Self {
            profiles: Profiles::new(app_dir),
            hkxcmd: Hkxcmd::in_app_dir(app_dir),
        }
    }

    /// Reads the named profile once, returning the Selected Profile its
    /// routing compiles from and the backend's facts the run's work is built
    /// from.
    ///
    /// # Errors
    /// [`Error::ConfigurationLoading`] when `profile.ini` or
    /// `ignoredMods.txt` cannot be read.
    fn read_profile(
        &self,
        profile_identity: &str,
    ) -> Result<(SelectedProfile, PreparedProfile), Error> {
        let failed = |error: ProfileError| Error::ConfigurationLoading(error.to_string());
        let profile = self.profiles.open(profile_identity);
        // The same strict read the GUI made when the Profile was selected, so
        // the run refuses exactly the Profiles the window reported.
        let settings = profile.load_settings().map_err(failed)?;
        let ignored_mods = profile.ignored_mods().map_err(failed)?;
        // A missing or unreadable list is empty, as in C++; finalization logs it.
        let files_to_not_pack = profile.files_to_not_pack();
        // The same holds here; the headpart scan logs it.
        let custom_headparts = profile.custom_headparts();
        // Every Archive fact of the run comes from this one value: the
        // Archive extension routing compiles from here, and the packer and
        // the headpart scan's plugin extensions later.
        let (archive_rules, capabilities) = rules_and_capabilities(&settings);
        let selected = SelectedProfile::new(capabilities)
            .with_conversion_preference(settings.textures_convert_tga)
            .with_ignored_mods(ignored_mods)
            .with_packing_exclusions(files_to_not_pack);
        Ok((
            selected,
            PreparedProfile {
                settings: Arc::new(settings),
                archive_rules,
                custom_headparts,
            },
        ))
    }
}

impl RunConfigurationProvider for ProfileConfigurationProvider {
    /// Reads the profile `request` names once and builds the run's Selected
    /// Profile and production Run Toolset from that read. Built only after
    /// every read succeeded, so work never sees a partial profile.
    ///
    /// The backend's Texture and Mesh settings come from the request's Work
    /// Choices, and the headpart scan walks the request's Mod Selection
    /// directory.
    fn load(&self, request: &RunRequest) -> Result<(SelectedProfile, RunToolset), Error> {
        let (selected, profile) = self.read_profile(request.profile_identity())?;
        let choices = request.work_choices();
        let work = RunWorkSettings {
            textures: texture_settings(&choices.textures),
            meshes: mesh_settings(&choices.meshes),
            selected_folder: request.mod_selection().directory().to_path_buf(),
            hkxcmd: self.hkxcmd.clone(),
        };
        // Core's Archive work reads its Archive choices from the request
        // itself, so none of them enters the Toolset. Its Execution Mode
        // does: every Capacity Check of the run is built with it.
        let toolset = work.toolset(Arc::new(profile), request.execution_mode());
        Ok((selected, toolset))
    }
}

/// One run's backend settings, built at load from its Run Request, that the
/// production Run Toolset is built with, ported from what C++
/// `ApplicationRunWork` held.
#[derive(Clone)]
struct RunWorkSettings {
    /// The backend's Texture settings, from the request's Texture choices.
    textures: TextureSettings,
    /// The backend's Mesh settings, from the request's Mesh choices.
    meshes: MeshSettings,
    /// The selected folder, which the headpart scan walks whole: C++ scanned
    /// `userPath`, so mods with a Mod Exclusion are scanned too.
    selected_folder: PathBuf,
    /// The app directory's `bin/hkxcmd.exe`; each run's backend gets a fresh
    /// copy, so each run checks for the exe once, as each C++ run did.
    hkxcmd: Hkxcmd,
}

impl RunWorkSettings {
    /// The production Run Toolset of one run in `mode`, the Run Request's
    /// Execution Mode, over the profile its Preparing read.
    fn toolset(&self, profile: Arc<PreparedProfile>, mode: ExecutionMode) -> RunToolset {
        // One reader serves discovery, extraction and finalization for the
        // whole run: the Toolset holds only this one.
        // C++ always wires Archive Finalization, so Apply executes the phase:
        // it packs only when the Routing Policy requests Archive creation, and
        // prunes empty directories either way.
        let seams = ArchiveSeams {
            reader: Arc::new(ArchiveFileReader::default()),
            volume_probe: Arc::new(VolumeProbes),
            packer: Arc::new(GameArchivePacker::new(profile.archive_rules)),
        };
        RunToolset::new(
            // The adapter creates the backend on the first Asset, as C++
            // creates its MainOptimizer, so a run with nothing to process
            // never sets up an optimizer.
            Box::new(AssetAttemptAdapter::new(OptimizerBackendFactory {
                work: self.clone(),
                profile,
            })),
            seams,
            mode,
        )
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
    /// The profile Preparing read, which the backend's `TextureProfile`, Mesh
    /// Target and custom Headpart Meshes come from.
    profile: Arc<PreparedProfile>,
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
        if !self.work.meshes.recognises_headparts() {
            return Ok((HeadpartList::default(), Vec::new()));
        }
        let profile = &self.profile;
        let extensions = profile.archive_rules.plugin_extensions;
        let scan = scan_headparts(
            &profile.custom_headparts,
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
        let settings = &self.profile.settings;
        let meshes =
            MeshOptimizer::new(self.work.meshes, mesh_target(settings)).with_headparts(headparts);
        // The adapter calls this on the Run Worker, the thread that must join
        // COM. C++'s texture optimizer threw when it could not, and its run
        // contained the exception as an unsafe Operation Failure; the adapter
        // contains this error the same way. The scan's diagnostics travel with
        // the error, so the run still retains the plugins it could not read.
        let backend = match OptimizerBackend::new(
            self.work.textures,
            texture_profile(settings),
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
    use cao_core::run::MeshLevel;

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
        let profile = Profiles::new(&app_dir).open("SSE");
        let selection = std::env::temp_dir().join("cao-optimizers-unit/backend-failure");
        // A missing directory is the expected case; anything else surfaces below.
        let _ = std::fs::remove_dir_all(&selection);
        std::fs::create_dir_all(&selection).unwrap();
        std::fs::write(selection.join("Broken.esp"), truncated_headpart_plugin()).unwrap();
        // The necessary Mesh level, so the headpart scan runs.
        let meshes = MeshChoices {
            level: MeshLevel::new(1).unwrap(),
            ..MeshChoices::default()
        };
        let mut factory = OptimizerBackendFactory {
            work: RunWorkSettings {
                textures: texture_settings(&TextureChoices::default()),
                meshes: mesh_settings(&meshes),
                selected_folder: selection.clone(),
                hkxcmd: Hkxcmd::in_app_dir(&app_dir),
            },
            profile: Arc::new({
                let settings = profile.load_settings().unwrap();
                PreparedProfile {
                    archive_rules: crate::profile::archive_rules(&settings),
                    settings: Arc::new(settings),
                    custom_headparts: Vec::new(),
                }
            }),
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
