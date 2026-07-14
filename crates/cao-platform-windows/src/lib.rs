#![deny(unsafe_op_in_unsafe_fn)]
//! Safe Windows platform capabilities with private, audited unsafe boundaries.

pub(crate) mod process;
mod run_effects;
mod state_document;

pub use run_effects::WindowsRunEnvironmentFactory;

use cao_application::{
    ActiveProfileId, AssetPath, AtomicFilePublisher, FailureKind, GlobalStateRecovery,
    GlobalStateRecoveryAction, OperationId, PortFailure, PortId, PortableState,
    PortableStateFactory, ProfileOverlay, ProfileSelectionFallback, SetupLoadOutcome, SetupState,
    authenticated_built_in_profile_contract,
};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use state_document::{DocumentError, StateDocument};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const ERROR_SHARING_VIOLATION: i32 = 32;
const STARTUP_DEFAULTS_RELATIVE_PATH: &str = "resources/profiles/SSE/startup.state";
const BUILT_IN_PROFILES_RELATIVE_PATH: &str = "resources/profiles/built-ins.state";
const GLOBAL_STATE_RELATIVE_PATH: &str = "data/config/application.state";
const RECOVERY_REPORT_RELATIVE_PATH: &str = "data/config/recovery.report";
#[cfg(test)]
const OVERLAY_STATE_RELATIVE_PATH: &str = "data/profiles/builtin-sse/overlay.state";
const OWNERSHIP_LOCK_RELATIVE_PATH: &str = "data/state.lock";
const SSE_STARTUP_DEFAULTS: &[u8] = b"schema_version=1\nactive_profile=SSE\ndry_run=0\n";
const AUTHENTICATED_BUILT_IN_PROFILES: &[u8] =
    include_bytes!("../../../resources/profiles/built-ins.state");

/// Safe Windows implementation of atomic same-volume file publication.
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsAtomicFilePublisher;

impl AtomicFilePublisher for WindowsAtomicFilePublisher {
    /// Publishes a prepared sibling through one write-through Windows replacement.
    fn replace(&self, source: &Path, destination: &Path) -> Result<(), PortFailure> {
        if !source.is_absolute() || !destination.is_absolute() {
            let subject = if !source.is_absolute() {
                source
            } else {
                destination
            };
            return Err(PortFailure::new(
                PortId::RunStore,
                OperationId::CommitReplace,
                FailureKind::InvalidInput,
                "atomic replacement paths must be absolute",
            )
            .with_subject(subject.display().to_string()));
        }
        windows_api::replace_file(source, destination).map_err(|error| {
            let kind = match error.kind() {
                io::ErrorKind::NotFound => FailureKind::NotFound,
                io::ErrorKind::PermissionDenied => FailureKind::PermissionDenied,
                io::ErrorKind::AlreadyExists => FailureKind::Conflict,
                _ => FailureKind::Io,
            };
            PortFailure::new(
                PortId::RunStore,
                OperationId::CommitReplace,
                kind,
                error.to_string(),
            )
            .with_subject(destination.display().to_string())
        })
    }
}

/// Factory for executable-relative, exclusively owned Tracetide portable state.
#[derive(Clone, Debug)]
pub struct WindowsPortableStateFactory {
    executable_root: PathBuf,
}

impl WindowsPortableStateFactory {
    /// Captures the canonical root containing the currently running executable.
    ///
    /// The process working directory is deliberately never consulted or changed.
    ///
    /// # Errors
    ///
    /// Returns a stable portable-state failure when the executable path cannot be
    /// resolved to a regular file with a canonical parent directory.
    pub fn from_current_executable() -> Result<Self, PortFailure> {
        let executable = std::env::current_exe().map_err(|error| {
            open_failure(
                FailureKind::Unavailable,
                None,
                format!("current executable could not be located: {error}"),
            )
        })?;
        Self::for_executable(executable)
    }

    /// Establishes a canonical executable root from an explicit executable path.
    ///
    /// This constructor supports packaged-launch verification without introducing a
    /// separate application seam; production uses [`Self::from_current_executable`].
    ///
    /// # Errors
    ///
    /// Returns a stable portable-state failure when `executable` cannot be resolved
    /// to a regular file or has no parent directory.
    pub fn for_executable(executable: impl AsRef<Path>) -> Result<Self, PortFailure> {
        let requested = executable.as_ref();
        if !requested.is_absolute() {
            return Err(open_failure(
                FailureKind::InvalidInput,
                Some(requested),
                "executable path must be absolute so it cannot inherit working-directory meaning",
            ));
        }
        let executable = requested
            .canonicalize()
            .map_err(|error| io_port_failure(OperationId::Open, requested, error))?;
        let executable_is_regular = windows_api::validate_executable_path(&executable)
            .map_err(|error| io_port_failure(OperationId::Open, &executable, error))?;
        if !executable_is_regular {
            return Err(open_failure(
                FailureKind::InvalidInput,
                Some(&executable),
                "executable root must be established from a regular file",
            ));
        }
        let executable_root = executable.parent().ok_or_else(|| {
            open_failure(
                FailureKind::InvalidInput,
                Some(&executable),
                "executable path has no containing directory",
            )
        })?;
        Ok(Self {
            executable_root: executable_root.to_path_buf(),
        })
    }

    /// Returns the canonical directory containing the captured executable.
    #[must_use]
    pub fn executable_root(&self) -> &Path {
        &self.executable_root
    }

    /// Creates per-run effect factories from this startup-captured executable root.
    ///
    /// Sharing this captured root prevents state and run-log capabilities from
    /// independently re-resolving executable-relative authority.
    #[must_use]
    pub fn run_environment_factory(&self) -> WindowsRunEnvironmentFactory {
        WindowsRunEnvironmentFactory::for_executable_root(self.executable_root.clone())
    }

    /// Opens and exclusively owns the fixed portable state tree.
    fn open_session(&self) -> Result<WindowsPortableState, PortFailure> {
        let executable_root_handle = hold_managed_directory(&self.executable_root)?;
        let bootstrap_ownership = windows_api::NamedOwnership::acquire(&self.executable_root)
            .map_err(|error| match error {
                windows_api::OwnershipError::Conflict => open_failure(
                    FailureKind::Conflict,
                    Some(&self.executable_root),
                    "portable state tree is already owned by another Tracetide process",
                ),
                windows_api::OwnershipError::Io(error) => {
                    io_port_failure(OperationId::Open, &self.executable_root, error)
                }
            })?;

        let data_root = self.executable_root.join("data");
        create_managed_directory(&self.executable_root, &data_root)?;
        let data_root_handle = hold_managed_directory(&data_root)?;
        let ownership_path = self.executable_root.join(OWNERSHIP_LOCK_RELATIVE_PATH);
        let ownership = acquire_state_ownership(&ownership_path)?;

        let startup_defaults = self.executable_root.join(STARTUP_DEFAULTS_RELATIVE_PATH);
        let built_in_profiles = self.executable_root.join(BUILT_IN_PROFILES_RELATIVE_PATH);
        let startup_profile_root = startup_defaults.parent().ok_or_else(|| {
            containment_failure(
                OperationId::Open,
                &startup_defaults,
                "startup defaults path has no managed parent",
            )
        })?;
        let resources_root = self.executable_root.join("resources");
        validate_existing_managed_path(&self.executable_root, &resources_root, OperationId::Open)
            .map_err(|failure| bundled_resource_directory_failure(&resources_root, failure))?;
        let resources_root_handle = hold_managed_directory(&resources_root)
            .map_err(|failure| bundled_resource_directory_failure(&resources_root, failure))?;
        let resources_profiles_root = self.executable_root.join("resources/profiles");
        validate_existing_managed_path(
            &self.executable_root,
            &resources_profiles_root,
            OperationId::Open,
        )
        .map_err(|failure| bundled_resource_directory_failure(&resources_profiles_root, failure))?;
        let resources_profiles_root_handle = hold_managed_directory(&resources_profiles_root)
            .map_err(|failure| {
                bundled_resource_directory_failure(&resources_profiles_root, failure)
            })?;
        validate_existing_managed_path(
            &self.executable_root,
            startup_profile_root,
            OperationId::Open,
        )
        .map_err(|failure| bundled_resource_directory_failure(startup_profile_root, failure))?;
        let startup_profile_root_handle = hold_managed_directory(startup_profile_root)
            .map_err(|failure| bundled_resource_directory_failure(startup_profile_root, failure))?;

        let profiles_root = self.executable_root.join("data/profiles");
        create_managed_directory(&self.executable_root, &profiles_root)?;
        let profiles_root_handle = hold_managed_directory(&profiles_root)?;
        let mut profile_root_handles = Vec::new();
        let overlay_paths = ActiveProfileId::ALL.map(|profile| {
            self.executable_root
                .join("data/profiles")
                .join(profile.definition().state_directory())
                .join("overlay.state")
        });
        for overlay_path in &overlay_paths {
            let profile_root = overlay_path.parent().ok_or_else(|| {
                containment_failure(
                    OperationId::Open,
                    overlay_path,
                    "profile overlay path has no managed parent",
                )
            })?;
            create_managed_directory(&self.executable_root, profile_root)?;
            profile_root_handles.push(hold_managed_directory(profile_root)?);
        }
        let config_root = self.executable_root.join("data/config");
        create_managed_directory(&self.executable_root, &config_root)?;
        let config_root_handle = hold_managed_directory(&config_root)?;
        let global_state_path = self.executable_root.join(GLOBAL_STATE_RELATIVE_PATH);

        let mut managed_directory_handles = vec![
            executable_root_handle,
            resources_root_handle,
            resources_profiles_root_handle,
            startup_profile_root_handle,
            data_root_handle,
            config_root_handle,
            profiles_root_handle,
        ];
        managed_directory_handles.extend(profile_root_handles);

        Ok(WindowsPortableState {
            _bootstrap_ownership: bootstrap_ownership,
            _ownership: ownership,
            _managed_directory_handles: managed_directory_handles,
            executable_root: self.executable_root.clone(),
            startup_defaults,
            built_in_profiles,
            global_state_path,
            overlay_paths,
            global_document: None,
            overlay_documents: std::array::from_fn(|_| None),
        })
    }
}

impl PortableStateFactory for WindowsPortableStateFactory {
    fn open(&self) -> Result<Box<dyn PortableState>, PortFailure> {
        self.open_session()
            .map(|state| Box::new(state) as Box<dyn PortableState>)
    }
}

/// One supervisor-owned portable-state session and its exclusive ownership lease.
struct WindowsPortableState {
    _bootstrap_ownership: windows_api::NamedOwnership,
    // The non-shareable handle is the lease; the file may remain after a crash and reopen safely.
    _ownership: File,
    // Directory handles deny delete sharing so validated roots cannot be swapped after startup.
    _managed_directory_handles: Vec<File>,
    executable_root: PathBuf,
    startup_defaults: PathBuf,
    built_in_profiles: PathBuf,
    global_state_path: PathBuf,
    overlay_paths: [PathBuf; 3],
    global_document: Option<StateDocument>,
    overlay_documents: [Option<StateDocument>; 3],
}

impl WindowsPortableState {
    /// Loads every built-in overlay independently so inactive choices cannot inherit state.
    fn load_profile_overlays(&mut self) -> Result<[ProfileOverlay; 3], PortFailure> {
        let mut overlays = std::array::from_fn(|_| ProfileOverlay::default());
        for profile in ActiveProfileId::ALL {
            let index = profile.storage_index();
            let path = self.overlay_paths[index].clone();
            let (overlay, document) = match read_optional_managed_file(
                &self.executable_root,
                &path,
                OperationId::LoadSetup,
            )? {
                None => {
                    let overlay = ProfileOverlay::default();
                    let document = default_overlay_document(&overlay);
                    (overlay, document)
                }
                Some(bytes) => decode_overlay_document(&self.executable_root, &bytes, &path)?,
            };
            overlays[index] = overlay;
            self.overlay_documents[index] = Some(document);
        }
        Ok(overlays)
    }
}

impl PortableState for WindowsPortableState {
    /// Loads authenticated setup state, migrating compatible documents transactionally.
    ///
    /// Corrupt global configuration retains this owned session and becomes a recovery
    /// projection; newer schemas, corrupt profile overlays, and I/O failures remain errors.
    fn load_setup(&mut self) -> Result<SetupLoadOutcome, PortFailure> {
        let defaults =
            read_required_bundled_resource(&self.executable_root, &self.startup_defaults)?;
        if defaults != SSE_STARTUP_DEFAULTS {
            return Err(PortFailure::new(
                PortId::PortableState,
                OperationId::LoadSetup,
                FailureKind::Integrity,
                "bundled SSE startup defaults do not match the authenticated contract",
            )
            .with_subject(self.startup_defaults.display().to_string()));
        }
        let built_ins =
            read_required_bundled_resource(&self.executable_root, &self.built_in_profiles)
                .map_err(|failure| {
                    PortFailure::new(
                        PortId::PortableState,
                        OperationId::LoadSetup,
                        FailureKind::Integrity,
                        format!(
                            "bundled built-in profile definitions could not be authenticated: {}",
                            failure.diagnostic().as_str()
                        ),
                    )
                    .with_subject(self.built_in_profiles.display().to_string())
                })?;
        let domain_contract = authenticated_built_in_profile_contract();
        if built_ins != AUTHENTICATED_BUILT_IN_PROFILES || built_ins != domain_contract.as_bytes() {
            return Err(PortFailure::new(
                PortId::PortableState,
                OperationId::LoadSetup,
                FailureKind::Integrity,
                "bundled FO4, SSE, and TES5 definitions do not match the authenticated contract",
            )
            .with_subject(self.built_in_profiles.display().to_string()));
        }

        let (active_profile, selection_fallback) = match read_optional_managed_file(
            &self.executable_root,
            &self.global_state_path,
            OperationId::LoadSetup,
        )? {
            None => {
                let (active_profile, document) = default_global_document();
                self.global_document = Some(document);
                (active_profile, None)
            }
            Some(bytes) => match decode_global_document(&bytes, &self.global_state_path) {
                Ok((active_profile, selection_fallback, document)) => {
                    if document.was_migrated() {
                        let migrated = document.encode();
                        atomic_replace(
                            &self.executable_root,
                            &self.global_state_path,
                            &migrated,
                            OperationId::MigrateState,
                        )?;
                    }
                    self.global_document = Some(document);
                    (active_profile, selection_fallback)
                }
                Err(failure) if failure.kind() == FailureKind::CorruptData => {
                    self.global_document = None;
                    let backup_available =
                        valid_global_backup(&self.executable_root, &self.global_state_path)?;
                    return Ok(SetupLoadOutcome::RecoveryRequired(
                        GlobalStateRecovery::new(failure, backup_available),
                    ));
                }
                Err(failure) => return Err(failure),
            },
        };

        let overlays = self.load_profile_overlays()?;
        let mut setup = SetupState::default().with_active_profile(active_profile);
        for profile in ActiveProfileId::ALL {
            setup =
                setup.with_profile_overlay_for(profile, overlays[profile.storage_index()].clone());
        }
        if let Some(reason) = selection_fallback {
            setup = setup.with_selection_fallback(reason);
        }
        Ok(SetupLoadOutcome::Ready(setup))
    }

    /// Atomically persists one complete profile overlay without touching global selection.
    fn persist_profile_overlay(
        &mut self,
        profile: ActiveProfileId,
        overlay: &ProfileOverlay,
    ) -> Result<(), PortFailure> {
        let index = profile.storage_index();
        let overlay_path = self.overlay_paths[index].clone();
        let _ = read_optional_managed_file(
            &self.executable_root,
            &overlay_path,
            OperationId::PersistSetup,
        )?;
        let mut document = self.overlay_documents[index]
            .clone()
            .unwrap_or_else(|| default_overlay_document(overlay));
        update_overlay_document(&mut document, overlay);
        let encoded = document.encode();
        atomic_replace(
            &self.executable_root,
            &overlay_path,
            &encoded,
            OperationId::PersistSetup,
        )?;
        self.overlay_documents[index] = Some(document);
        Ok(())
    }

    /// Atomically persists one stable selection without materializing an unchanged overlay.
    fn persist_active_profile(&mut self, profile: ActiveProfileId) -> Result<(), PortFailure> {
        let active_profile = profile.stable_id();
        let mut global_document = self
            .global_document
            .clone()
            .unwrap_or_else(|| default_global_document().1);
        if global_document.get("active_profile") != Some(active_profile) {
            global_document.set("active_profile", active_profile);
            atomic_replace(
                &self.executable_root,
                &self.global_state_path,
                &global_document.encode(),
                OperationId::PersistSetup,
            )?;
            self.global_document = Some(global_document);
        }
        Ok(())
    }

    /// Restores the valid global backup or explicitly resets only global configuration.
    ///
    /// Both actions preserve profile, import, provenance, and log trees. The corrupt global
    /// authority is retained at its diagnostic sibling when replacement commits.
    fn recover_global_state(
        &mut self,
        action: GlobalStateRecoveryAction,
    ) -> Result<SetupState, PortFailure> {
        let (operation, action_name) = match action {
            GlobalStateRecoveryAction::RestoreBackup => {
                (OperationId::RestoreGlobalState, "restore-backup")
            }
            GlobalStateRecoveryAction::Reset => (OperationId::ResetGlobalState, "reset"),
        };
        let recovery_report = self.executable_root.join(RECOVERY_REPORT_RELATIVE_PATH);
        let report = format!(
            "schema_version=1\naction={action_name}\nresult=validation-started\ncorrupt_copy=application.state.corrupt\n"
        );
        // Recording first guarantees that validation failures, commit failures, and
        // successful recovery all leave a durable diagnostic of the explicit attempt.
        atomic_replace(
            &self.executable_root,
            &recovery_report,
            report.as_bytes(),
            operation,
        )?;
        let (candidate, active_profile, global_document, operation) = match action {
            GlobalStateRecoveryAction::RestoreBackup => {
                let backup = sibling_with_suffix(
                    &self.global_state_path,
                    ".backup",
                    OperationId::RestoreGlobalState,
                )?;
                let bytes = read_optional_managed_file(
                    &self.executable_root,
                    &backup,
                    OperationId::RestoreGlobalState,
                )?
                .ok_or_else(|| {
                    PortFailure::new(
                        PortId::PortableState,
                        OperationId::RestoreGlobalState,
                        FailureKind::NotFound,
                        "no valid global configuration backup is available",
                    )
                    .with_subject(backup.display().to_string())
                })?;
                let (active_profile, _, document) = decode_global_document(&bytes, &backup)
                    .map_err(|failure| {
                        PortFailure::new(
                            PortId::PortableState,
                            OperationId::RestoreGlobalState,
                            failure.kind(),
                            failure.diagnostic().as_str(),
                        )
                        .with_subject(backup.display().to_string())
                    })?;
                (
                    document.encode(),
                    active_profile,
                    document,
                    OperationId::RestoreGlobalState,
                )
            }
            GlobalStateRecoveryAction::Reset => {
                let (active_profile, document) = default_global_document();
                let bytes = document.encode();
                (
                    bytes,
                    active_profile,
                    document,
                    OperationId::ResetGlobalState,
                )
            }
        };

        // Every remaining fallible setup read happens before the global authority commits.
        let overlays = self.load_profile_overlays()?;
        let corrupt_backup = sibling_with_suffix(&self.global_state_path, ".corrupt", operation)?;
        atomic_replace_with_backup(
            &self.executable_root,
            &self.global_state_path,
            &corrupt_backup,
            &candidate,
            operation,
        )?;
        self.global_document = Some(global_document);
        let mut setup = SetupState::default().with_active_profile(active_profile);
        for profile in ActiveProfileId::ALL {
            setup =
                setup.with_profile_overlay_for(profile, overlays[profile.storage_index()].clone());
        }
        Ok(setup)
    }
}

/// Acquires a Windows sharing lease that survives stale lock-file residue safely.
fn acquire_state_ownership(path: &Path) -> Result<File, PortFailure> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
                open_failure(
                    FailureKind::Conflict,
                    Some(path),
                    "portable state tree is already owned by another Tracetide process",
                )
            } else {
                io_port_failure(OperationId::Open, path, error)
            }
        })?;
    validate_regular_handle(&file, path, OperationId::Open)?;
    Ok(file)
}

/// Creates one fixed managed directory and rejects any reparse or root escape.
fn create_managed_directory(root: &Path, path: &Path) -> Result<(), PortFailure> {
    reject_reparse_if_present(path, OperationId::Open)?;
    std::fs::create_dir_all(path)
        .map_err(|error| io_port_failure(OperationId::Open, path, error))?;
    validate_existing_managed_path(root, path, OperationId::Open)
}

/// Confirms every component is ordinary and the canonical result remains under `root`.
fn validate_existing_managed_path(
    root: &Path,
    path: &Path,
    operation: OperationId,
) -> Result<(), PortFailure> {
    let relative = path.strip_prefix(root).map_err(|_| {
        containment_failure(
            operation,
            path,
            "managed path is outside the executable root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(name) => current.push(name),
            _ => {
                return Err(containment_failure(
                    operation,
                    path,
                    "managed path contains a non-normal component",
                ));
            }
        }
        reject_reparse_if_present(&current, operation)?;
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| io_port_failure(operation, path, error))?;
    if !canonical.starts_with(root) {
        return Err(containment_failure(
            operation,
            path,
            "managed path resolves outside the executable root",
        ));
    }
    Ok(())
}

/// Rejects a present Windows reparse point before it can redirect managed I/O.
fn reject_reparse_if_present(path: &Path, operation: OperationId) -> Result<(), PortFailure> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => Err(
            containment_failure(operation, path, "managed path is a Windows reparse point"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_port_failure(operation, path, error)),
    }
}

/// Reads a required built-in resource through the same no-follow handle that was inspected.
fn read_required_bundled_resource(root: &Path, path: &Path) -> Result<Vec<u8>, PortFailure> {
    match read_optional_managed_file(root, path, OperationId::LoadSetup) {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => Err(bundled_resource_failure(
            path,
            io::Error::from(io::ErrorKind::NotFound),
        )),
        Err(failure) if failure.kind() == FailureKind::NotFound => Err(bundled_resource_failure(
            path,
            io::Error::from(io::ErrorKind::NotFound),
        )),
        Err(failure) => Err(failure),
    }
}

/// Reads an optional managed file without following a leaf reparse point.
fn read_optional_managed_file(
    root: &Path,
    path: &Path,
    operation: OperationId,
) -> Result<Option<Vec<u8>>, PortFailure> {
    let parent = path.parent().ok_or_else(|| {
        containment_failure(operation, path, "managed leaf has no parent directory")
    })?;
    validate_existing_managed_path(root, parent, operation)?;
    let mut file = match OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_port_failure(operation, path, error)),
    };
    validate_regular_handle(&file, path, operation)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| io_port_failure(operation, path, error))?;
    Ok(Some(bytes))
}

/// Confirms an opened file handle names an ordinary non-reparse regular file.
fn validate_regular_handle(
    file: &File,
    path: &Path,
    operation: OperationId,
) -> Result<(), PortFailure> {
    let metadata = file
        .metadata()
        .map_err(|error| io_port_failure(operation, path, error))?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(containment_failure(
            operation,
            path,
            "managed file handle is not an ordinary contained file",
        ));
    }
    Ok(())
}

/// Confirms an opened directory handle names an ordinary non-reparse directory.
fn validate_directory_handle(
    directory: &File,
    path: &Path,
    operation: OperationId,
) -> Result<(), PortFailure> {
    let metadata = directory
        .metadata()
        .map_err(|error| io_port_failure(operation, path, error))?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(containment_failure(
            operation,
            path,
            "managed directory handle is not an ordinary contained directory",
        ));
    }
    Ok(())
}

/// Atomically replaces one contained state file after flushing its complete candidate.
///
/// When an authoritative file already exists, an ordered write-through replacement first
/// retains its complete prior bytes at `.backup`; authority changes only in the next move.
fn atomic_replace(
    root: &Path,
    destination: &Path,
    bytes: &[u8],
    operation: OperationId,
) -> Result<(), PortFailure> {
    let backup = sibling_with_suffix(destination, ".backup", operation)?;
    atomic_replace_with_backup(root, destination, &backup, bytes, operation)
}

/// Commits one durable replacement while retaining the old authority at `backup`.
fn atomic_replace_with_backup(
    root: &Path,
    destination: &Path,
    backup: &Path,
    bytes: &[u8],
    operation: OperationId,
) -> Result<(), PortFailure> {
    let parent = destination.parent().ok_or_else(|| {
        containment_failure(
            operation,
            destination,
            "portable state file has no parent directory",
        )
    })?;
    validate_existing_managed_path(root, parent, operation)?;
    let temporary = sibling_with_suffix(destination, ".pending", operation)?;
    reject_reparse_if_present(&temporary, operation)?;
    reject_reparse_if_present(backup, operation)?;
    if temporary.exists() {
        std::fs::remove_file(&temporary)
            .map_err(|error| io_port_failure(operation, &temporary, error))?;
    }

    let write_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&temporary)?;
        validate_regular_handle(&file, &temporary, operation)
            .map_err(|failure| io::Error::other(failure.diagnostic().as_str()))?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        // A failed candidate is inactive; cleanup must not replace its more useful write error.
        let _ = std::fs::remove_file(&temporary);
        return Err(io_port_failure(operation, &temporary, error));
    }

    validate_existing_managed_path(root, parent, operation)?;
    reject_reparse_if_present(destination, operation)?;
    reject_reparse_if_present(backup, operation)?;
    if let Some(prior) = read_optional_managed_file(root, destination, operation)? {
        let backup_temporary = sibling_with_suffix(backup, ".pending", operation)?;
        reject_reparse_if_present(&backup_temporary, operation)?;
        if backup_temporary.exists() {
            std::fs::remove_file(&backup_temporary)
                .map_err(|error| io_port_failure(operation, &backup_temporary, error))?;
        }
        let backup_result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .share_mode(0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&backup_temporary)?;
            validate_regular_handle(&file, &backup_temporary, operation)
                .map_err(|failure| io::Error::other(failure.diagnostic().as_str()))?;
            file.write_all(&prior)?;
            file.sync_all()
        })();
        if let Err(error) = backup_result {
            // Neither incomplete candidate may displace the still-authoritative state file.
            let _ = std::fs::remove_file(&backup_temporary);
            let _ = std::fs::remove_file(&temporary);
            return Err(io_port_failure(operation, &backup_temporary, error));
        }
        if let Err(error) = windows_api::replace_file(&backup_temporary, backup) {
            // A durable old authority remains live even when backup rotation cannot commit.
            let _ = std::fs::remove_file(&backup_temporary);
            let _ = std::fs::remove_file(&temporary);
            return Err(io_port_failure(operation, backup, error));
        }
    }
    if let Err(error) = windows_api::replace_file(&temporary, destination) {
        // The prior authoritative state file remains active when the atomic move fails.
        let _ = std::fs::remove_file(&temporary);
        return Err(io_port_failure(operation, destination, error));
    }
    Ok(())
}

/// Produces a contained sibling path without accepting caller-controlled components.
fn sibling_with_suffix(
    path: &Path,
    suffix: &str,
    operation: OperationId,
) -> Result<PathBuf, PortFailure> {
    let name = path.file_name().ok_or_else(|| {
        containment_failure(operation, path, "portable state file has no leaf name")
    })?;
    let mut sibling = OsString::from(name);
    sibling.push(suffix);
    Ok(path.with_file_name(sibling))
}

/// Constructs a current-schema document from one complete profile overlay.
fn default_overlay_document(overlay: &ProfileOverlay) -> StateDocument {
    let mut document = StateDocument::current([]);
    update_overlay_document(&mut document, overlay);
    document
}

/// Updates every supported overlay field while retaining unknown compatible fields.
fn update_overlay_document(document: &mut StateDocument, overlay: &ProfileOverlay) {
    document.set("mode", "single-mod");
    document.set(
        "asset_path",
        overlay.asset_path().map_or("", AssetPath::as_str),
    );
    set_bool(document, "dry_run", overlay.dry_run());
    set_bool(document, "debug_log", overlay.debug_log());
    let archives = overlay.archives();
    set_bool(document, "archives.extract", archives.extract());
    set_bool(document, "archives.create", archives.create());
    set_bool(
        document,
        "archives.delete_backups",
        archives.delete_backups(),
    );
    set_bool(
        document,
        "archives.merge_incompressible",
        archives.merge_incompressible(),
    );
    set_bool(
        document,
        "archives.merge_textures",
        archives.merge_textures(),
    );
    set_bool(
        document,
        "archives.process_content",
        archives.process_content(),
    );
    set_bool(
        document,
        "archives.create_dummies",
        archives.create_dummies(),
    );
    set_bool(document, "archives.compress", archives.compress());
    set_bool(document, "archives.delete_source", archives.delete_source());
    let textures = overlay.textures();
    set_bool(
        document,
        "textures.process_necessary",
        textures.process_necessary(),
    );
    set_bool(document, "textures.compress", textures.compress());
    set_bool(
        document,
        "textures.generate_mipmaps",
        textures.generate_mipmaps(),
    );
    set_bool(
        document,
        "textures.resize_to_fixed_size",
        textures.resize_to_fixed_size(),
    );
    document.set("textures.target_width", textures.target_width().to_string());
    document.set(
        "textures.target_height",
        textures.target_height().to_string(),
    );
    set_bool(
        document,
        "textures.resize_by_ratio",
        textures.resize_by_ratio(),
    );
    document.set("textures.width_ratio", textures.width_ratio().to_string());
    document.set("textures.height_ratio", textures.height_ratio().to_string());
    let meshes = overlay.meshes();
    document.set(
        "meshes.optimization_level",
        meshes.optimization_level().to_string(),
    );
    set_bool(
        document,
        "meshes.handle_headparts",
        meshes.handle_headparts(),
    );
    set_bool(document, "meshes.resave", meshes.resave());
    set_bool(
        document,
        "animations.optimize",
        overlay.animations().optimize(),
    );
}

/// Stores one canonical fork-owned boolean in a compatible state document.
fn set_bool(document: &mut StateDocument, key: &str, value: bool) {
    document.set(key, u8::from(value).to_string());
}

/// Decodes and, when necessary, transactionally migrates one profile overlay.
fn decode_overlay_document(
    root: &Path,
    bytes: &[u8],
    path: &Path,
) -> Result<(ProfileOverlay, StateDocument), PortFailure> {
    let document = StateDocument::decode(bytes).map_err(|error| document_failure(path, error))?;
    let dry_run = match document.get("dry_run") {
        Some("0") => false,
        Some("1") => true,
        _ => {
            return Err(corrupt_document(
                path,
                "portable profile overlay has no boolean dry_run",
            ));
        }
    };
    if document
        .get("mode")
        .is_some_and(|mode| mode != "single-mod")
    {
        return Err(corrupt_document(
            path,
            "portable profile overlay has an unsupported mode",
        ));
    }
    let defaults = ProfileOverlay::default();
    let archives = defaults
        .archives()
        .with_extract(read_optional_bool(
            &document,
            "archives.extract",
            false,
            path,
        )?)
        .with_create(read_optional_bool(
            &document,
            "archives.create",
            false,
            path,
        )?)
        .with_delete_backups(read_optional_bool(
            &document,
            "archives.delete_backups",
            false,
            path,
        )?)
        .with_merge_incompressible(read_optional_bool(
            &document,
            "archives.merge_incompressible",
            true,
            path,
        )?)
        .with_merge_textures(read_optional_bool(
            &document,
            "archives.merge_textures",
            false,
            path,
        )?)
        .with_process_content(read_optional_bool(
            &document,
            "archives.process_content",
            false,
            path,
        )?)
        .with_create_dummies(read_optional_bool(
            &document,
            "archives.create_dummies",
            true,
            path,
        )?)
        .with_compress(read_optional_bool(
            &document,
            "archives.compress",
            true,
            path,
        )?)
        .with_delete_source(read_optional_bool(
            &document,
            "archives.delete_source",
            true,
            path,
        )?);
    let textures = defaults
        .textures()
        .with_process_necessary(read_optional_bool(
            &document,
            "textures.process_necessary",
            true,
            path,
        )?)
        .with_compress(read_optional_bool(
            &document,
            "textures.compress",
            false,
            path,
        )?)
        .with_generate_mipmaps(read_optional_bool(
            &document,
            "textures.generate_mipmaps",
            false,
            path,
        )?)
        .with_resize_to_fixed_size(read_optional_bool(
            &document,
            "textures.resize_to_fixed_size",
            false,
            path,
        )?)
        .with_resize_by_ratio(read_optional_bool(
            &document,
            "textures.resize_by_ratio",
            false,
            path,
        )?)
        .with_target_size(
            read_optional_u32(&document, "textures.target_width", 2048, path)?,
            read_optional_u32(&document, "textures.target_height", 2048, path)?,
        )
        .with_ratios(
            read_optional_u32(&document, "textures.width_ratio", 2, path)?,
            read_optional_u32(&document, "textures.height_ratio", 2, path)?,
        );
    let optimization_level = read_optional_u32(&document, "meshes.optimization_level", 0, path)?;
    let optimization_level = u8::try_from(optimization_level).map_err(|_| {
        corrupt_document(path, "portable profile overlay mesh level is out of range")
    })?;
    let meshes = defaults
        .meshes()
        .with_optimization_level(optimization_level)
        .with_handle_headparts(read_optional_bool(
            &document,
            "meshes.handle_headparts",
            true,
            path,
        )?)
        .with_resave(read_optional_bool(&document, "meshes.resave", false, path)?);
    let animation_optimize = read_optional_bool(
        &document,
        "animations.optimize",
        document.get("animation_optimize") == Some("1"),
        path,
    )?;
    if document.was_migrated() {
        let migrated = document.encode();
        atomic_replace(root, path, &migrated, OperationId::MigrateState)?;
    }
    let mut overlay = defaults
        .with_dry_run(dry_run)
        .with_debug_log(read_optional_bool(&document, "debug_log", false, path)?)
        .with_archives(archives)
        .with_textures(textures)
        .with_meshes(meshes)
        .with_animations(defaults.animations().with_optimize(animation_optimize));
    if let Some(asset_path) = document.get("asset_path").filter(|value| !value.is_empty()) {
        let asset_path = AssetPath::new(asset_path).map_err(|error| {
            corrupt_document(
                path,
                &format!("portable profile overlay asset_path is invalid: {error}"),
            )
        })?;
        overlay = overlay.with_asset_path(asset_path);
    }
    Ok((overlay, document))
}

/// Reads one optional canonical boolean, using the documented deterministic default.
fn read_optional_bool(
    document: &StateDocument,
    key: &str,
    default: bool,
    path: &Path,
) -> Result<bool, PortFailure> {
    match document.get(key) {
        None => Ok(default),
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(corrupt_document(
            path,
            &format!("portable profile overlay field {key} is not boolean"),
        )),
    }
}

/// Reads one optional non-zero-capable integer with a deterministic fallback.
fn read_optional_u32(
    document: &StateDocument,
    key: &str,
    default: u32,
    path: &Path,
) -> Result<u32, PortFailure> {
    match document.get(key) {
        None => Ok(default),
        Some(value) => value.parse().map_err(|_| {
            corrupt_document(
                path,
                &format!("portable profile overlay field {key} is not an unsigned integer"),
            )
        }),
    }
}

/// Constructs default global configuration without mutating the portable state tree.
fn default_global_document() -> (ActiveProfileId, StateDocument) {
    (
        ActiveProfileId::Sse,
        StateDocument::current([(
            "active_profile".to_owned(),
            ActiveProfileId::Sse.stable_id().to_owned(),
        )]),
    )
}

/// Decodes one global configuration document and validates its active profile identity.
fn decode_global_document(
    bytes: &[u8],
    path: &Path,
) -> Result<
    (
        ActiveProfileId,
        Option<ProfileSelectionFallback>,
        StateDocument,
    ),
    PortFailure,
> {
    let document = StateDocument::decode(bytes).map_err(|error| document_failure(path, error))?;
    let (active_profile, fallback) = match document.get("active_profile") {
        Some(value) => match ActiveProfileId::from_stable_id(value) {
            Some(profile) => (profile, None),
            None => (
                ActiveProfileId::Sse,
                Some(ProfileSelectionFallback::Invalid),
            ),
        },
        None => (
            ActiveProfileId::Sse,
            Some(ProfileSelectionFallback::Missing),
        ),
    };
    Ok((active_profile, fallback, document))
}

/// Reports whether the fixed newest backup is readable, compatible, and valid.
fn valid_global_backup(root: &Path, global_path: &Path) -> Result<bool, PortFailure> {
    let backup = sibling_with_suffix(global_path, ".backup", OperationId::LoadSetup)?;
    let Some(bytes) = read_optional_managed_file(root, &backup, OperationId::LoadSetup)? else {
        return Ok(false);
    };
    Ok(decode_global_document(&bytes, &backup).is_ok())
}

/// Maps a versioned-document error without conflating corruption with a newer schema.
fn document_failure(path: &Path, error: DocumentError) -> PortFailure {
    match error {
        DocumentError::Corrupt(diagnostic) => corrupt_document(path, diagnostic),
        DocumentError::NewerSchema { found, supported } => PortFailure::new(
            PortId::PortableState,
            OperationId::LoadSetup,
            FailureKind::Unsupported,
            format!(
                "portable state schema version {found} is newer than supported version {supported}"
            ),
        )
        .with_subject(path.display().to_string()),
    }
}

/// Creates the stable corruption result for malformed fork-owned state.
fn corrupt_document(path: &Path, diagnostic: &str) -> PortFailure {
    PortFailure::new(
        PortId::PortableState,
        OperationId::LoadSetup,
        FailureKind::CorruptData,
        diagnostic,
    )
    .with_subject(path.display().to_string())
}

/// Normalizes any bundled-profile directory failure as installation integrity loss.
fn bundled_resource_directory_failure(path: &Path, failure: PortFailure) -> PortFailure {
    PortFailure::new(
        PortId::PortableState,
        OperationId::LoadSetup,
        FailureKind::Integrity,
        format!(
            "bundled profile resources could not be loaded: {}",
            failure.diagnostic().as_str()
        ),
    )
    .with_subject(path.display().to_string())
}

/// Narrow safe wrappers over Windows atomic file-replacement primitives.
mod windows_api {
    use std::ffi::OsStr;
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x80;
    const WAIT_TIMEOUT: u32 = 0x102;
    const WAIT_FAILED: u32 = u32::MAX;
    const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    #[link(name = "Kernel32")]
    // SAFETY: These declarations exactly match their Win32 ABI signatures, and every call
    // remains inside the safe wrappers below with handle and pointer lifetimes checked.
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
        fn CreateMutexW(
            mutex_attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn ReleaseMutex(handle: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetFileAttributesW(file_name: *const u16) -> u32;
    }

    /// Failure to establish the process-level bootstrap ownership lease.
    pub(super) enum OwnershipError {
        /// Another process in this Windows session owns the same executable root.
        Conflict,
        /// Windows could not create or wait on the ownership object.
        Io(io::Error),
    }

    /// Session-local bootstrap lease used before the durable state lock can be created.
    pub(super) struct NamedOwnership {
        handle: isize,
    }

    impl NamedOwnership {
        /// Acquires a non-blocking named lease derived from the canonical executable root.
        pub(super) fn acquire(executable_root: &Path) -> Result<Self, OwnershipError> {
            let name = ownership_name(executable_root);
            // SAFETY: `name` is a live NUL-terminated UTF-16 buffer, the security
            // attributes pointer is null, and ownership is requested only by the wait.
            let handle = unsafe { self::CreateMutexW(ptr::null(), 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(OwnershipError::Io(io::Error::last_os_error()));
            }
            // SAFETY: `handle` was returned by CreateMutexW and remains live here.
            let wait_result = unsafe { self::WaitForSingleObject(handle, 0) };
            match wait_result {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self {
                    handle: handle as isize,
                }),
                WAIT_TIMEOUT => {
                    // SAFETY: The valid handle is not owned and is closed exactly once.
                    let _ = unsafe { self::CloseHandle(handle) };
                    Err(OwnershipError::Conflict)
                }
                WAIT_FAILED => {
                    let error = io::Error::last_os_error();
                    // SAFETY: The failed wait did not consume the valid handle.
                    let _ = unsafe { self::CloseHandle(handle) };
                    Err(OwnershipError::Io(error))
                }
                unexpected => {
                    // SAFETY: The unexpected wait result did not consume the valid handle.
                    let _ = unsafe { self::CloseHandle(handle) };
                    Err(OwnershipError::Io(io::Error::other(format!(
                        "unexpected ownership wait result {unexpected}"
                    ))))
                }
            }
        }
    }

    impl Drop for NamedOwnership {
        fn drop(&mut self) {
            let handle = self.handle as *mut c_void;
            // SAFETY: Successful construction owns the mutex and this is its only Drop.
            let _ = unsafe { self::ReleaseMutex(handle) };
            // SAFETY: The handle remains valid until it is closed exactly once here.
            let _ = unsafe { self::CloseHandle(handle) };
        }
    }

    /// Replaces `destination` with `source` in one write-through filesystem operation.
    pub(super) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
        let source = null_terminated(source.as_os_str());
        let destination = null_terminated(destination.as_os_str());
        // SAFETY: Both buffers are live, immutable, NUL-terminated UTF-16 strings for the
        // duration of the call. The flags commit the same-volume replacement durably.
        let succeeded = unsafe {
            self::MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if succeeded == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Reports whether a canonical executable path names an ordinary file.
    pub(super) fn validate_executable_path(path: &Path) -> io::Result<bool> {
        let path = null_terminated(path.as_os_str());
        // SAFETY: `path` is a live NUL-terminated UTF-16 buffer for this synchronous call.
        let attributes = unsafe { self::GetFileAttributesW(path.as_ptr()) };
        if attributes == INVALID_FILE_ATTRIBUTES {
            return Err(io::Error::last_os_error());
        }
        Ok(attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) == 0)
    }

    /// Encodes one Windows path for a single synchronous Win32 call.
    fn null_terminated(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    /// Derives a bounded mutex name from the canonical root without exposing its path.
    fn ownership_name(executable_root: &Path) -> Vec<u16> {
        let units: Vec<u16> = executable_root.as_os_str().encode_wide().collect();
        let forward = fnv1a(units.iter().copied(), 0xcbf2_9ce4_8422_2325);
        let reverse = fnv1a(units.iter().rev().copied(), 0x8422_2325_cbf2_9ce4);
        null_terminated(OsStr::new(&format!(
            "Local\\Tracetide-State-{:016X}-{:016X}-{}",
            forward,
            reverse,
            units.len()
        )))
    }

    /// Computes one stable 64-bit identifier over canonical UTF-16 path units.
    fn fnv1a(units: impl Iterator<Item = u16>, offset: u64) -> u64 {
        units.fold(offset, |hash, unit| {
            let low = u64::from(unit & 0xff);
            let high = u64::from(unit >> 8);
            let hash = (hash ^ low).wrapping_mul(0x100_0000_01b3);
            (hash ^ high).wrapping_mul(0x100_0000_01b3)
        })
    }
}

/// Creates a stable containment failure without leaking platform error types.
fn containment_failure(operation: OperationId, path: &Path, diagnostic: &str) -> PortFailure {
    PortFailure::new(
        PortId::PortableState,
        operation,
        FailureKind::Integrity,
        diagnostic,
    )
    .with_subject(path.display().to_string())
}

/// Maps one platform I/O failure to the application-owned portable-state vocabulary.
fn io_port_failure(operation: OperationId, path: &Path, error: io::Error) -> PortFailure {
    let kind = match error.kind() {
        io::ErrorKind::NotFound => FailureKind::NotFound,
        io::ErrorKind::PermissionDenied => FailureKind::PermissionDenied,
        io::ErrorKind::AlreadyExists => FailureKind::Conflict,
        _ => FailureKind::Io,
    };
    PortFailure::new(PortId::PortableState, operation, kind, error.to_string())
        .with_subject(path.display().to_string())
}

/// Holds a managed directory without delete sharing so its identity cannot be swapped.
fn hold_managed_directory(path: &Path) -> Result<File, PortFailure> {
    let directory = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| io_port_failure(OperationId::Open, path, error))?;
    validate_directory_handle(&directory, path, OperationId::Open)?;
    Ok(directory)
}

/// Maps an unreadable required built-in resource to fatal installation integrity.
fn bundled_resource_failure(path: &Path, error: io::Error) -> PortFailure {
    PortFailure::new(
        PortId::PortableState,
        OperationId::LoadSetup,
        FailureKind::Integrity,
        format!("bundled SSE startup defaults could not be loaded: {error}"),
    )
    .with_subject(path.display().to_string())
}

/// Builds an `Open` failure with an optional affected path.
fn open_failure(
    kind: FailureKind,
    path: Option<&Path>,
    diagnostic: impl Into<String>,
) -> PortFailure {
    let failure = PortFailure::new(PortId::PortableState, OperationId::Open, kind, diagnostic);
    match path {
        Some(path) => failure.with_subject(path.display().to_string()),
        None => failure,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BUILT_IN_PROFILES_RELATIVE_PATH, ERROR_SHARING_VIOLATION, FILE_SHARE_READ,
        GLOBAL_STATE_RELATIVE_PATH, OVERLAY_STATE_RELATIVE_PATH, RECOVERY_REPORT_RELATIVE_PATH,
        STARTUP_DEFAULTS_RELATIVE_PATH, WindowsPortableStateFactory,
        validate_existing_managed_path,
    };
    use cao_application::{
        ActiveProfileId, ApplicationRuntime, AssetPath, FailureKind, GlobalStateRecoveryAction,
        Intent, IntentOutcome, PortableStateFactory, ProfileOverlay, ProfileOverlayEdit,
        ProfileSelectionFallback, SetupLoadOutcome, SnapshotRevision, SnapshotSink,
        WorkbenchSnapshot,
    };
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    const WAIT_TIMEOUT: Duration = Duration::from_secs(5);
    const AUTHENTICATED_SSE_STARTUP_DEFAULTS: &[u8] =
        b"schema_version=1\nactive_profile=SSE\ndry_run=0\n";
    const COMMITTED_SSE_STARTUP_DEFAULTS: &[u8] =
        include_bytes!("../../../resources/profiles/SSE/startup.state");
    const COMMITTED_BUILT_IN_PROFILES: &[u8] =
        include_bytes!("../../../resources/profiles/built-ins.state");
    const CHILD_EXECUTABLE_ENV: &str = "TRACETIDE_PORTABLE_STATE_CHILD_EXECUTABLE";

    #[derive(Default)]
    struct RecordingSink {
        snapshots: Mutex<Vec<Arc<WorkbenchSnapshot>>>,
        changed: Condvar,
    }

    impl RecordingSink {
        /// Waits for the public snapshot at `index`, failing instead of hanging indefinitely.
        fn wait_for(&self, index: usize) -> Arc<WorkbenchSnapshot> {
            let deadline = Instant::now() + WAIT_TIMEOUT;
            let mut snapshots = self.snapshots.lock().expect("snapshot lock was poisoned");
            while snapshots.len() <= index {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .expect("snapshot was not published before the timeout");
                (snapshots, _) = self
                    .changed
                    .wait_timeout(snapshots, remaining)
                    .expect("snapshot lock was poisoned while waiting");
            }
            Arc::clone(&snapshots[index])
        }
    }

    impl SnapshotSink for RecordingSink {
        fn publish(&self, snapshot: Arc<WorkbenchSnapshot>) {
            self.snapshots
                .lock()
                .expect("snapshot lock was poisoned")
                .push(snapshot);
            self.changed.notify_all();
        }
    }

    struct PortableSandbox {
        root: PathBuf,
        executable: PathBuf,
    }

    impl PortableSandbox {
        /// Creates an executable tree with the authenticated startup-default resource.
        fn create(name: &str) -> Self {
            assert_eq!(
                COMMITTED_SSE_STARTUP_DEFAULTS, AUTHENTICATED_SSE_STARTUP_DEFAULTS,
                "the committed resource must retain the reviewed startup contract"
            );
            let root = std::env::temp_dir().join(format!(
                "tracetide-portable-{name}-{}-{}",
                std::process::id(),
                unique_test_id()
            ));
            let resource = root.join("resources/profiles/SSE/startup.state");
            std::fs::create_dir_all(resource.parent().expect("resource should have a parent"))
                .expect("sandbox resources should be created");
            std::fs::write(&resource, COMMITTED_SSE_STARTUP_DEFAULTS)
                .expect("startup defaults should be written");
            std::fs::write(
                root.join(BUILT_IN_PROFILES_RELATIVE_PATH),
                COMMITTED_BUILT_IN_PROFILES,
            )
            .expect("authenticated built-in definitions should be written");
            let executable = root.join("tracetide.exe");
            std::fs::write(&executable, b"test executable")
                .expect("sandbox executable should be written");
            Self { root, executable }
        }

        /// Returns the fake executable used to establish the canonical executable root.
        fn executable(&self) -> &Path {
            &self.executable
        }

        /// Returns the executable-relative sandbox root.
        fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for PortableSandbox {
        fn drop(&mut self) {
            // Test cleanup is best-effort because a prior assertion may leave a file open.
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Returns a process-local identifier so parallel tests never share portable state.
    fn unique_test_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn executable_relative_start_persists_sse_overlay_through_public_seam() {
        let sandbox = PortableSandbox::create("restart");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("fresh portable state should start");
        let initial = sink.wait_for(0);

        assert_eq!(initial.revision(), SnapshotRevision::INITIAL);
        assert_eq!(initial.setup().active_profile(), ActiveProfileId::Sse);
        assert!(!initial.setup().profile_overlay().dry_run());

        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the overlay edit should enter the application queue");
        let applied = sink.wait_for(1);
        assert!(applied.setup().profile_overlay().dry_run());
        runtime
            .shutdown()
            .expect("the first runtime should release portable state");

        let restarted_sink = Arc::new(RecordingSink::default());
        let (_handle, restarted_runtime) =
            ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&restarted_sink))
                .expect("portable state should reopen after shutdown");
        let restarted = restarted_sink.wait_for(0);
        assert_eq!(restarted.setup().active_profile(), ActiveProfileId::Sse);
        assert!(restarted.setup().profile_overlay().dry_run());
        restarted_runtime
            .shutdown()
            .expect("the restarted runtime should shut down cleanly");
    }

    #[test]
    fn built_in_selection_and_overlays_survive_restart_without_cross_profile_leakage() {
        let sandbox = PortableSandbox::create("profile-isolation");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("fresh portable state should start");
        let initial = sink.wait_for(0);

        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the SSE edit should enter the queue");
        let sse_edited = sink.wait_for(1);
        handle
            .submit(Intent::SelectProfile {
                expected_revision: sse_edited.revision(),
                profile_id: ActiveProfileId::Fo4,
            })
            .expect("the FO4 selection should enter the queue");
        let fo4_selected = sink.wait_for(2);
        assert!(!fo4_selected.setup().profile_overlay().dry_run());
        runtime
            .shutdown()
            .expect("the first runtime should release portable state");

        let restarted_sink = Arc::new(RecordingSink::default());
        let (restarted_handle, restarted_runtime) =
            ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&restarted_sink))
                .expect("portable state should reopen after shutdown");
        let restarted = restarted_sink.wait_for(0);
        assert_eq!(restarted.setup().active_profile(), ActiveProfileId::Fo4);
        assert!(!restarted.setup().profile_overlay().dry_run());
        restarted_handle
            .submit(Intent::SelectProfile {
                expected_revision: restarted.revision(),
                profile_id: ActiveProfileId::Sse,
            })
            .expect("the SSE reselection should enter the queue");
        let sse_reselected = restarted_sink.wait_for(1);
        assert!(sse_reselected.setup().profile_overlay().dry_run());
        restarted_runtime
            .shutdown()
            .expect("the restarted runtime should shut down cleanly");
    }

    #[test]
    fn complete_overlay_round_trips_through_the_portable_state_port() {
        let sandbox = PortableSandbox::create("complete-overlay");
        let factory = WindowsPortableStateFactory::for_executable(sandbox.executable())
            .expect("the executable root should resolve");
        let mut state = factory.open().expect("portable state should open");
        let initial = match state.load_setup().expect("setup should load") {
            SetupLoadOutcome::Ready(setup) => setup,
            SetupLoadOutcome::RecoveryRequired(_) => {
                panic!("fresh setup unexpectedly needs recovery")
            }
        };
        let defaults = ProfileOverlay::default();
        let overlay = defaults
            .with_asset_path(
                AssetPath::new(r"E:\Mods\A=Named Mod")
                    .expect("the absolute asset path should validate"),
            )
            .with_dry_run(true)
            .with_debug_log(true)
            .with_archives(
                defaults
                    .archives()
                    .with_extract(true)
                    .with_create(true)
                    .with_delete_backups(true)
                    .with_merge_incompressible(false)
                    .with_merge_textures(true)
                    .with_process_content(true)
                    .with_create_dummies(false)
                    .with_compress(false)
                    .with_delete_source(false),
            )
            .with_textures(
                defaults
                    .textures()
                    .with_process_necessary(false)
                    .with_compress(true)
                    .with_generate_mipmaps(true)
                    .with_resize_to_fixed_size(true)
                    .with_resize_by_ratio(true)
                    .with_target_size(1024, 512)
                    .with_ratios(4, 8),
            )
            .with_meshes(
                defaults
                    .meshes()
                    .with_optimization_level(3)
                    .with_handle_headparts(false)
                    .with_resave(true),
            )
            .with_animations(defaults.animations().with_optimize(true));
        state
            .persist_profile_overlay(initial.active_profile(), &overlay)
            .expect("complete overlay should persist");
        drop(state);

        let mut reopened = factory.open().expect("portable state should reopen");
        let reloaded = match reopened.load_setup().expect("persisted setup should load") {
            SetupLoadOutcome::Ready(setup) => setup,
            SetupLoadOutcome::RecoveryRequired(_) => {
                panic!("persisted setup unexpectedly needs recovery")
            }
        };
        assert_eq!(reloaded.profile_overlay(), &overlay);
    }

    #[test]
    fn profile_selection_commits_only_global_authority() {
        let sandbox = PortableSandbox::create("selection-authority");
        let factory = WindowsPortableStateFactory::for_executable(sandbox.executable())
            .expect("the executable root should resolve");
        let mut state = factory.open().expect("portable state should open");
        state.load_setup().expect("fresh setup should load");

        state
            .persist_active_profile(ActiveProfileId::Fo4)
            .expect("the stable selection should persist");

        let global = std::fs::read_to_string(sandbox.root().join(GLOBAL_STATE_RELATIVE_PATH))
            .expect("global selection should be materialized");
        assert!(global.contains("active_profile=FO4"));
        assert!(
            !sandbox
                .root()
                .join("data/profiles/builtin-fo4/overlay.state")
                .exists(),
            "selection must not create a second persistence authority"
        );
    }

    #[test]
    fn relative_persisted_asset_path_is_rejected_as_corrupt_overlay_state() {
        let sandbox = PortableSandbox::create("relative-asset-path");
        let factory = WindowsPortableStateFactory::for_executable(sandbox.executable())
            .expect("the executable root should resolve");
        let mut state = factory.open().expect("portable state should open");
        std::fs::write(
            sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH),
            b"schema_version=2\ndry_run=0\nasset_path=relative\\assets\n",
        )
        .expect("invalid overlay fixture should be written");

        let failure = state
            .load_setup()
            .expect_err("a relative remembered path must not enter the domain model");
        assert_eq!(failure.kind(), FailureKind::CorruptData);
        assert!(
            failure.diagnostic().as_str().contains("not absolute"),
            "unexpected diagnostic: {}",
            failure.diagnostic().as_str()
        );
    }

    #[test]
    fn missing_or_invalid_selection_visibly_falls_back_to_sse_and_can_be_corrected() {
        for (name, document, expected_reason) in [
            (
                "missing-selection",
                b"schema_version=2\nfuture=kept\n".as_slice(),
                ProfileSelectionFallback::Missing,
            ),
            (
                "invalid-selection",
                b"schema_version=2\nactive_profile=display-name-skyrim\nfuture=kept\n".as_slice(),
                ProfileSelectionFallback::Invalid,
            ),
        ] {
            let sandbox = PortableSandbox::create(name);
            let global = sandbox.root().join(GLOBAL_STATE_RELATIVE_PATH);
            std::fs::create_dir_all(global.parent().expect("global state should have a parent"))
                .expect("configuration directory should be created");
            std::fs::write(&global, document).expect("global state should be written");
            let factory = Arc::new(
                WindowsPortableStateFactory::for_executable(sandbox.executable())
                    .expect("the executable root should resolve"),
            );
            let sink = Arc::new(RecordingSink::default());
            let (handle, runtime) =
                ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
                    .expect("invalid selection should not become corruption recovery");
            let fallback = sink.wait_for(0);

            assert_eq!(fallback.setup().active_profile(), ActiveProfileId::Sse);
            assert_eq!(fallback.setup().selection_fallback(), Some(expected_reason));
            assert_eq!(fallback.global_state_recovery(), None);
            handle
                .submit(Intent::SelectProfile {
                    expected_revision: fallback.revision(),
                    profile_id: ActiveProfileId::Tes5,
                })
                .expect("an explicit identity should correct the fallback");
            let corrected = sink.wait_for(1);
            assert_eq!(corrected.setup().active_profile(), ActiveProfileId::Tes5);
            assert_eq!(corrected.setup().selection_fallback(), None);
            runtime
                .shutdown()
                .expect("the corrected runtime should shut down cleanly");

            let saved = std::fs::read_to_string(&global)
                .expect("corrected global state should remain readable");
            assert!(saved.contains("active_profile=TES5"));
            assert!(saved.contains("future=kept"));
        }
    }

    #[test]
    fn version_one_overlay_migrates_atomically_and_preserves_unknown_fields() {
        let sandbox = PortableSandbox::create("overlay-migration");
        let overlay = sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(overlay.parent().expect("overlay should have a parent"))
            .expect("profile directory should be created");
        let original = b"schema_version=1\ndry_run=1\nfuture_option=keep\n";
        std::fs::write(&overlay, original).expect("version-one overlay should be written");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());

        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("the compatible overlay should migrate during startup");

        let initial = sink.wait_for(0);
        assert!(initial.setup().profile_overlay().dry_run());
        let migrated = std::fs::read_to_string(&overlay).expect("migrated overlay should be UTF-8");
        assert!(migrated.contains("schema_version=2"));
        assert!(migrated.contains("future_option=keep"));
        assert_eq!(
            std::fs::read(overlay.with_extension("state.backup"))
                .expect("migration should retain a restorable backup"),
            original
        );
        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(false),
            })
            .expect("the post-migration edit should enter the queue");
        assert!(!sink.wait_for(1).setup().profile_overlay().dry_run());
        let edited = std::fs::read_to_string(&overlay).expect("edited overlay should be UTF-8");
        assert!(edited.contains("future_option=keep"));
        assert!(edited.contains("dry_run=0"));
        assert_eq!(
            std::fs::read_to_string(overlay.with_extension("state.backup"))
                .expect("the migrated authority should become the next backup"),
            migrated
        );
        runtime
            .shutdown()
            .expect("the migrated runtime should shut down cleanly");
    }

    #[test]
    fn newer_overlay_schema_is_rejected_without_mutating_its_bytes() {
        let sandbox = PortableSandbox::create("newer-overlay");
        let overlay = sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(overlay.parent().expect("overlay should have a parent"))
            .expect("profile directory should be created");
        let newer = b"schema_version=999\ndry_run=1\nfuture.kept=1\n";
        std::fs::write(&overlay, newer).expect("newer overlay should be written");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());

        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("newer overlay schema unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.kind(), cao_application::FailureKind::Unsupported);
        assert_eq!(
            std::fs::read(&overlay).expect("overlay should remain"),
            newer
        );
        assert!(!overlay.with_extension("state.backup").exists());
        assert!(!overlay.with_extension("state.pending").exists());
    }

    #[test]
    fn corrupt_global_state_can_restore_backup_without_touching_other_state_trees() {
        let sandbox = PortableSandbox::create("global-restore");
        let global = sandbox.root().join(GLOBAL_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(global.parent().expect("global state should have a parent"))
            .expect("configuration directory should be created");
        let corrupt = b"not a global configuration\n";
        let backup = b"schema_version=2\nactive_profile=SSE\nfuture.kept=true\n";
        std::fs::write(&global, corrupt).expect("corrupt global state should be written");
        std::fs::write(global.with_extension("state.backup"), backup)
            .expect("valid backup should be written");
        let protected = [
            (
                "data/profiles/custom-a/manifest.state",
                b"profile".as_slice(),
            ),
            (
                "data/imports/import-a/source/profile.ini",
                b"provenance".as_slice(),
            ),
            ("data/imports/import-a/report.txt", b"report".as_slice()),
            ("data/logs/run-a.log", b"log".as_slice()),
        ];
        for (relative, bytes) in protected {
            let path = sandbox.root().join(relative);
            std::fs::create_dir_all(path.parent().expect("sentinel should have a parent"))
                .expect("sentinel parent should be created");
            std::fs::write(path, bytes).expect("sentinel should be written");
        }
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("corrupt global state should publish a recovery snapshot");
        let recovery = sink.wait_for(0);

        assert!(
            recovery
                .global_state_recovery()
                .is_some_and(|state| state.backup_available())
        );
        assert_eq!(
            std::fs::read(&global).expect("corrupt state should remain"),
            corrupt
        );
        let receipt = handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: recovery.revision(),
                action: GlobalStateRecoveryAction::RestoreBackup,
            })
            .expect("restore choice should enter the application queue");
        let restored = sink.wait_for(1);
        assert!(restored.global_state_recovery().is_none());
        assert!(matches!(
            restored.last_intent(),
            Some(IntentOutcome::Applied(actual)) if *actual == receipt
        ));
        assert_eq!(
            std::fs::read(&global).expect("global state should restore"),
            backup
        );
        assert_eq!(
            std::fs::read(global.with_extension("state.corrupt"))
                .expect("the corrupt authority should remain available for diagnosis"),
            corrupt
        );
        let report = std::fs::read_to_string(sandbox.root().join(RECOVERY_REPORT_RELATIVE_PATH))
            .expect("restore should record a durable diagnostic report");
        assert!(report.contains("action=restore-backup"));
        for (relative, bytes) in protected {
            assert_eq!(
                std::fs::read(sandbox.root().join(relative)).expect("sentinel should remain"),
                bytes
            );
        }
        runtime
            .shutdown()
            .expect("the recovered runtime should shut down cleanly");
    }

    #[test]
    fn corrupt_global_state_can_reset_without_deleting_profiles_imports_or_logs() {
        let sandbox = PortableSandbox::create("global-reset");
        let global = sandbox.root().join(GLOBAL_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(global.parent().expect("global state should have a parent"))
            .expect("configuration directory should be created");
        let corrupt = b"schema_version=2\nactive_profile\n";
        std::fs::write(&global, corrupt).expect("corrupt global state should be written");
        let protected = [
            (
                "data/profiles/custom-b/manifest.state",
                b"profile".as_slice(),
            ),
            (
                "data/imports/import-b/provenance/source.ini",
                b"import".as_slice(),
            ),
            ("data/logs/run-b.log", b"log".as_slice()),
        ];
        for (relative, bytes) in protected {
            let path = sandbox.root().join(relative);
            std::fs::create_dir_all(path.parent().expect("sentinel should have a parent"))
                .expect("sentinel parent should be created");
            std::fs::write(path, bytes).expect("sentinel should be written");
        }
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("corrupt global state should publish a recovery snapshot");
        let recovery = sink.wait_for(0);
        assert!(
            recovery
                .global_state_recovery()
                .is_some_and(|state| !state.backup_available())
        );

        handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: recovery.revision(),
                action: GlobalStateRecoveryAction::Reset,
            })
            .expect("reset choice should enter the application queue");
        let reset = sink.wait_for(1);

        assert!(reset.global_state_recovery().is_none());
        assert_eq!(reset.setup().active_profile(), ActiveProfileId::Sse);
        let reset_bytes = std::fs::read_to_string(&global).expect("reset state should be UTF-8");
        assert!(reset_bytes.contains("schema_version=2"));
        assert!(reset_bytes.contains("active_profile=SSE"));
        let report = std::fs::read_to_string(sandbox.root().join(RECOVERY_REPORT_RELATIVE_PATH))
            .expect("reset should record a durable diagnostic report");
        assert!(report.contains("action=reset"));
        for (relative, bytes) in protected {
            assert_eq!(
                std::fs::read(sandbox.root().join(relative)).expect("sentinel should remain"),
                bytes
            );
        }
        runtime
            .shutdown()
            .expect("the reset runtime should shut down cleanly");
    }

    #[test]
    fn failed_restore_validation_never_commits_or_rotates_the_corrupt_authority() {
        let sandbox = PortableSandbox::create("restore-validation-failure");
        let global = sandbox.root().join(GLOBAL_STATE_RELATIVE_PATH);
        let overlay = sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(global.parent().expect("global state should have a parent"))
            .expect("configuration directory should be created");
        std::fs::create_dir_all(overlay.parent().expect("overlay should have a parent"))
            .expect("profile directory should be created");
        let corrupt = b"corrupt global state\n";
        let backup = b"schema_version=2\nactive_profile=SSE\n";
        std::fs::write(&global, corrupt).expect("corrupt global state should be written");
        std::fs::write(global.with_extension("state.backup"), backup)
            .expect("valid global backup should be written");
        std::fs::write(&overlay, b"corrupt overlay\n").expect("corrupt overlay should be written");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("global corruption should project recovery before reading the overlay");
        let recovery = sink.wait_for(0);

        handle
            .submit(Intent::RecoverGlobalState {
                expected_revision: recovery.revision(),
                action: GlobalStateRecoveryAction::RestoreBackup,
            })
            .expect("restore choice should enter the queue");
        let failed = sink.wait_for(1);

        assert!(failed.global_state_recovery().is_some());
        assert!(matches!(
            failed.last_intent(),
            Some(IntentOutcome::Failed { .. })
        ));
        assert_eq!(
            std::fs::read(&global).expect("corrupt state should remain"),
            corrupt
        );
        assert_eq!(
            std::fs::read(global.with_extension("state.backup"))
                .expect("valid backup should remain"),
            backup
        );
        assert!(!global.with_extension("state.corrupt").exists());
        let report = std::fs::read_to_string(sandbox.root().join(RECOVERY_REPORT_RELATIVE_PATH))
            .expect("failed validation should still record its recovery attempt");
        assert!(report.contains("action=restore-backup"));
        runtime
            .shutdown()
            .expect("runtime should shut down after failed recovery validation");
    }

    #[test]
    fn version_one_global_configuration_migrates_with_a_restorable_backup() {
        let sandbox = PortableSandbox::create("global-migration");
        let global = sandbox.root().join(GLOBAL_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(global.parent().expect("global state should have a parent"))
            .expect("configuration directory should be created");
        let original = b"schema_version=1\nactive_profile=SSE\nfuture_global=keep\n";
        std::fs::write(&global, original).expect("version-one global state should be written");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());

        let (_handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("compatible global state should migrate during startup");

        assert_eq!(
            sink.wait_for(0).setup().active_profile(),
            ActiveProfileId::Sse
        );
        let migrated = std::fs::read_to_string(&global).expect("migrated global state is UTF-8");
        assert!(migrated.contains("schema_version=2"));
        assert!(migrated.contains("future_global=keep"));
        assert_eq!(
            std::fs::read(global.with_extension("state.backup"))
                .expect("migration should preserve its original"),
            original
        );
        runtime
            .shutdown()
            .expect("the migrated runtime should shut down cleanly");
    }

    #[test]
    fn newer_global_schema_is_rejected_without_offering_destructive_recovery() {
        let sandbox = PortableSandbox::create("newer-global");
        let global = sandbox.root().join(GLOBAL_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(global.parent().expect("global state should have a parent"))
            .expect("configuration directory should be created");
        let newer = b"schema_version=999\nactive_profile=SSE\nfuture=true\n";
        std::fs::write(&global, newer).expect("newer global state should be written");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());

        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("newer global schema unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.kind(), cao_application::FailureKind::Unsupported);
        assert_eq!(
            std::fs::read(&global).expect("global state should remain"),
            newer
        );
        assert!(!global.with_extension("state.backup").exists());
    }

    #[test]
    fn injected_replace_failure_preserves_authoritative_overlay_and_in_memory_setup() {
        let sandbox = PortableSandbox::create("replace-failure");
        let overlay = sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(overlay.parent().expect("overlay should have a parent"))
            .expect("profile directory should be created");
        let original = b"schema_version=2\ndry_run=0\nfuture=keep\n";
        std::fs::write(&overlay, original).expect("authoritative overlay should be written");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("portable state should start before fault injection");
        let initial = sink.wait_for(0);
        let _replace_blocker = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&overlay)
            .expect("the fixture should deny replacement sharing");

        let receipt = handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the faulted edit should enter the queue");
        let failed = sink.wait_for(1);

        assert!(!failed.setup().profile_overlay().dry_run());
        assert!(matches!(
            failed.last_intent(),
            Some(IntentOutcome::Failed { receipt: actual, .. }) if *actual == receipt
        ));
        assert_eq!(
            std::fs::read(&overlay).expect("overlay should remain"),
            original
        );
        assert!(!overlay.with_extension("state.pending").exists());
        runtime
            .shutdown()
            .expect("the runtime should shut down after the injected failure");
    }

    #[test]
    fn backup_leaf_reparse_point_cannot_redirect_a_durable_write() {
        let sandbox = PortableSandbox::create("backup-reparse");
        let outside = PortableSandbox::create("backup-reparse-outside");
        let overlay = sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(overlay.parent().expect("overlay should have a parent"))
            .expect("profile directory should be created");
        std::fs::write(&overlay, b"schema_version=2\ndry_run=0\n")
            .expect("authoritative overlay should be written");
        let outside_file = outside.root().join("outside-backup.state");
        std::fs::write(&outside_file, b"outside remains unchanged\n")
            .expect("outside sentinel should be written");
        std::os::windows::fs::symlink_file(&outside_file, overlay.with_extension("state.backup"))
            .expect("the backup leaf should become a reparse point");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("portable state should start before backup substitution");
        let initial = sink.wait_for(0);

        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the redirected edit should enter the queue");
        let failed = sink.wait_for(1);

        assert!(matches!(
            failed.last_intent(),
            Some(IntentOutcome::Failed { failure, .. })
                if failure.kind() == cao_application::FailureKind::Integrity
        ));
        assert_eq!(
            std::fs::read(&outside_file).expect("outside sentinel should remain readable"),
            b"outside remains unchanged\n"
        );
        runtime
            .shutdown()
            .expect("runtime should shut down after rejecting backup escape");
    }

    #[test]
    fn failed_overlay_migration_preserves_the_exact_original_document() {
        let sandbox = PortableSandbox::create("migration-failure");
        let overlay = sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH);
        std::fs::create_dir_all(overlay.parent().expect("overlay should have a parent"))
            .expect("profile directory should be created");
        let original = b"schema_version=1\ndry_run=1\nunknown=preserved\n";
        std::fs::write(&overlay, original).expect("version-one overlay should be written");
        let _replace_blocker = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&overlay)
            .expect("the fixture should deny migration replacement");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());

        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("faulted migration unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(
            failure.operation(),
            cao_application::OperationId::MigrateState
        );
        assert_eq!(
            std::fs::read(&overlay).expect("original should remain"),
            original
        );
        assert!(!overlay.with_extension("state.pending").exists());
        assert_eq!(
            std::fs::read(overlay.with_extension("state.backup"))
                .expect("failed migration should still retain a restorable backup"),
            original
        );
    }

    #[test]
    fn lexical_parent_traversal_is_rejected_before_canonical_resolution() {
        let sandbox = PortableSandbox::create("traversal");
        let outside = sandbox.root().join("outside");
        std::fs::create_dir(&outside).expect("outside fixture should be created");
        let traversing = sandbox.root().join("data").join("..").join("outside");

        let failure = validate_existing_managed_path(
            sandbox.root(),
            &traversing,
            cao_application::OperationId::Open,
        )
        .expect_err("a parent component must never acquire managed-path meaning");

        assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
    }

    #[test]
    fn junction_mount_point_cannot_redirect_managed_state() {
        let sandbox = PortableSandbox::create("junction");
        let outside = PortableSandbox::create("junction-outside");
        // Directory junctions use IO_REPARSE_TAG_MOUNT_POINT, the same redirecting tag as
        // volume mount points, so this unprivileged fixture exercises tag-agnostic rejection.
        let status = Command::new("cmd")
            .arg("/d")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(sandbox.root().join("data"))
            .arg(outside.root())
            .status()
            .expect("junction fixture command should launch");
        assert!(status.success(), "junction fixture should be created");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());

        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("junction-backed portable state unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
        assert!(!outside.root().join("config/application.state").exists());
    }

    #[test]
    fn relative_executable_path_is_rejected_without_consulting_working_directory() {
        let failure = WindowsPortableStateFactory::for_executable("tracetide.exe")
            .expect_err("a relative executable path must not acquire current-directory meaning");

        assert_eq!(failure.kind(), cao_application::FailureKind::InvalidInput);
        assert_eq!(failure.operation(), cao_application::OperationId::Open);
    }

    #[test]
    fn second_process_cannot_share_portable_state_from_an_unrelated_working_directory() {
        let sandbox = PortableSandbox::create("ownership");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (_handle, runtime) = ApplicationRuntime::start(factory, sink)
            .expect("the first process should own portable state");
        let unrelated_working_directory = sandbox.root().join("unrelated-working-directory");
        std::fs::create_dir(&unrelated_working_directory)
            .expect("the unrelated working directory should be created");

        let status = Command::new(std::env::current_exe().expect("test executable should resolve"))
            .arg("--exact")
            .arg("tests::state_lock_child_probe")
            .arg("--nocapture")
            .env(CHILD_EXECUTABLE_ENV, sandbox.executable())
            .current_dir(unrelated_working_directory)
            .status()
            .expect("the ownership probe process should launch");

        assert!(
            status.success(),
            "the child must observe the typed conflict"
        );
        runtime
            .shutdown()
            .expect("the owning runtime should release portable state");
    }

    #[test]
    fn state_lock_child_probe() {
        let Some(executable) = std::env::var_os(CHILD_EXECUTABLE_ENV) else {
            // The probe only performs work when launched by the cross-process ownership test.
            return;
        };
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(executable)
                .expect("the child should resolve the explicit executable root"),
        );
        let sink = Arc::new(RecordingSink::default());
        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected child runtime should still shut down cleanly");
                panic!("the child unexpectedly shared the owned portable state tree");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.port(), cao_application::PortId::PortableState);
        assert_eq!(failure.operation(), cao_application::OperationId::Open);
        assert_eq!(failure.kind(), cao_application::FailureKind::Conflict);
    }

    #[test]
    fn modified_sse_startup_resource_is_an_installation_integrity_failure() {
        let sandbox = PortableSandbox::create("integrity");
        std::fs::write(
            sandbox.root().join(STARTUP_DEFAULTS_RELATIVE_PATH),
            b"schema_version=1\nactive_profile=SSE\ndry_run=1\n",
        )
        .expect("the sandbox resource should be modified");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("modified bundled defaults unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.operation(), cao_application::OperationId::LoadSetup);
        assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
    }

    #[test]
    fn missing_or_modified_built_in_definition_inventory_is_an_integrity_failure() {
        for (name, replacement) in [
            ("missing-built-ins", None),
            (
                "modified-built-ins",
                Some(b"schema_version=1\nSSE.identity=FO4\n".as_slice()),
            ),
        ] {
            let sandbox = PortableSandbox::create(name);
            let manifest = sandbox.root().join(BUILT_IN_PROFILES_RELATIVE_PATH);
            match replacement {
                Some(bytes) => std::fs::write(&manifest, bytes)
                    .expect("modified built-in inventory should be written"),
                None => {
                    std::fs::remove_file(&manifest).expect("built-in inventory should be removed")
                }
            }
            let factory = Arc::new(
                WindowsPortableStateFactory::for_executable(sandbox.executable())
                    .expect("the executable root should resolve"),
            );

            let failure =
                match ApplicationRuntime::start(factory, Arc::new(RecordingSink::default())) {
                    Ok((_handle, runtime)) => {
                        runtime
                            .shutdown()
                            .expect("an unexpected runtime should still shut down cleanly");
                        panic!("unauthenticated built-ins unexpectedly started");
                    }
                    Err(failure) => failure,
                };

            assert_eq!(failure.port(), cao_application::PortId::PortableState);
            assert_eq!(failure.operation(), cao_application::OperationId::LoadSetup);
            assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
            assert!(failure.subject().is_some_and(|subject| {
                subject
                    .as_str()
                    .ends_with("resources\\profiles\\built-ins.state")
            }));
        }
    }

    #[test]
    fn missing_sse_startup_resource_is_an_installation_integrity_failure() {
        let sandbox = PortableSandbox::create("missing-resource");
        std::fs::remove_dir_all(
            sandbox
                .root()
                .join(STARTUP_DEFAULTS_RELATIVE_PATH)
                .parent()
                .expect("startup resource should have a parent"),
        )
        .expect("the sandbox SSE resource directory should be removed");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("missing bundled defaults unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.operation(), cao_application::OperationId::LoadSetup);
        assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
    }

    #[test]
    fn startup_resource_leaf_reparse_point_is_rejected_without_following_it() {
        let sandbox = PortableSandbox::create("resource-reparse");
        let outside = PortableSandbox::create("resource-reparse-outside");
        let startup_resource = sandbox.root().join(STARTUP_DEFAULTS_RELATIVE_PATH);
        std::fs::remove_file(&startup_resource).expect("the real resource should be removed");
        std::os::windows::fs::symlink_file(outside.executable(), &startup_resource)
            .expect("the resource leaf should become a reparse point");

        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("the resource reparse point unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.operation(), cao_application::OperationId::LoadSetup);
        assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
    }

    #[test]
    fn persisted_overlay_does_not_bypass_startup_resource_authentication() {
        let sandbox = PortableSandbox::create("restart-integrity");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(Arc::clone(&factory), Arc::clone(&sink))
            .expect("portable state should start before resource tampering");
        let initial = sink.wait_for(0);
        handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the overlay edit should enter the queue");
        assert!(sink.wait_for(1).setup().profile_overlay().dry_run());
        runtime
            .shutdown()
            .expect("the first runtime should release portable state");
        std::fs::write(
            sandbox.root().join(STARTUP_DEFAULTS_RELATIVE_PATH),
            b"tampered defaults\n",
        )
        .expect("the sandbox resource should be modified");

        let restarted_sink = Arc::new(RecordingSink::default());
        let failure = match ApplicationRuntime::start(factory, restarted_sink) {
            Ok((_handle, restarted_runtime)) => {
                restarted_runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("persisted state unexpectedly bypassed resource integrity");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.operation(), cao_application::OperationId::LoadSetup);
        assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
    }

    #[test]
    fn reparse_point_cannot_redirect_managed_state_outside_executable_root() {
        let sandbox = PortableSandbox::create("contained");
        let outside = PortableSandbox::create("outside");
        std::os::windows::fs::symlink_dir(outside.root(), sandbox.root().join("data"))
            .expect("the containment fixture should create a directory reparse point");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let failure = match ApplicationRuntime::start(factory, sink) {
            Ok((_handle, runtime)) => {
                runtime
                    .shutdown()
                    .expect("an unexpected runtime should still shut down cleanly");
                panic!("the redirected portable state tree unexpectedly started");
            }
            Err(failure) => failure,
        };

        assert_eq!(failure.operation(), cao_application::OperationId::Open);
        assert_eq!(failure.kind(), cao_application::FailureKind::Integrity);
    }

    #[test]
    fn managed_profile_root_cannot_be_swapped_after_start() {
        let sandbox = PortableSandbox::create("late-reparse");
        let outside = PortableSandbox::create("late-reparse-outside");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("portable state should start before the reparse swap");
        let initial = sink.wait_for(0);
        let profile_root = sandbox.root().join("data/profiles/builtin-sse");
        let removal_error = std::fs::remove_dir(&profile_root)
            .expect_err("the held profile root must deny a swap after startup");
        assert_eq!(removal_error.raw_os_error(), Some(ERROR_SHARING_VIOLATION));

        let receipt = handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the redirected edit should enter the application queue");
        let applied = sink.wait_for(1);

        assert!(matches!(
            applied.last_intent(),
            Some(IntentOutcome::Applied(actual_receipt)) if *actual_receipt == receipt
        ));
        assert!(!outside.root().join("overlay.state").exists());
        runtime
            .shutdown()
            .expect("the runtime should release state after rejecting the escape");
    }

    #[test]
    fn overlay_leaf_reparse_point_is_rejected_without_following_it() {
        let sandbox = PortableSandbox::create("overlay-reparse");
        let outside = PortableSandbox::create("overlay-reparse-outside");
        let factory = Arc::new(
            WindowsPortableStateFactory::for_executable(sandbox.executable())
                .expect("the executable root should resolve"),
        );
        let sink = Arc::new(RecordingSink::default());
        let (handle, runtime) = ApplicationRuntime::start(factory, Arc::clone(&sink))
            .expect("portable state should start before the leaf substitution");
        let initial = sink.wait_for(0);
        let outside_file = outside.root().join("outside-overlay.state");
        std::fs::write(&outside_file, b"outside remains unchanged\n")
            .expect("the outside file should be written");
        let overlay = sandbox.root().join(OVERLAY_STATE_RELATIVE_PATH);
        std::os::windows::fs::symlink_file(&outside_file, &overlay)
            .expect("the overlay leaf should become a reparse point");

        let receipt = handle
            .submit(Intent::EditProfileOverlay {
                expected_revision: initial.revision(),
                edit: ProfileOverlayEdit::SetDryRun(true),
            })
            .expect("the edit should enter the application queue");
        let failed = sink.wait_for(1);

        assert!(matches!(
            failed.last_intent(),
            Some(IntentOutcome::Failed {
                receipt: actual_receipt,
                failure,
            }) if *actual_receipt == receipt && failure.kind() == cao_application::FailureKind::Integrity
        ));
        assert_eq!(
            std::fs::read(&outside_file).expect("the outside file should remain readable"),
            b"outside remains unchanged\n"
        );
        runtime
            .shutdown()
            .expect("the runtime should release state after rejecting the leaf escape");
    }
}
