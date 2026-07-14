#![deny(unsafe_op_in_unsafe_fn)]
//! Safe Windows platform capabilities with private, audited unsafe boundaries.

use cao_application::{
    FailureKind, OperationId, PortFailure, PortId, PortableState, PortableStateFactory,
    ProfileOverlay, SetupState,
};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const ERROR_SHARING_VIOLATION: i32 = 32;
const STARTUP_DEFAULTS_RELATIVE_PATH: &str = "resources/profiles/SSE/startup.state";
const OVERLAY_STATE_RELATIVE_PATH: &str = "data/profiles/SSE/overlay.state";
const OWNERSHIP_LOCK_RELATIVE_PATH: &str = "data/state.lock";
const SSE_STARTUP_DEFAULTS: &[u8] = b"schema_version=1\nactive_profile=SSE\ndry_run=0\n";

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
        let startup_profile_root = startup_defaults.parent().ok_or_else(|| {
            containment_failure(
                OperationId::Open,
                &startup_defaults,
                "startup defaults path has no managed parent",
            )
        })?;
        let resources_root = self.executable_root.join("resources");
        validate_existing_managed_path(&self.executable_root, &resources_root, OperationId::Open)?;
        let resources_root_handle = hold_managed_directory(&resources_root)?;
        let resources_profiles_root = self.executable_root.join("resources/profiles");
        validate_existing_managed_path(
            &self.executable_root,
            &resources_profiles_root,
            OperationId::Open,
        )?;
        let resources_profiles_root_handle = hold_managed_directory(&resources_profiles_root)?;
        validate_existing_managed_path(
            &self.executable_root,
            startup_profile_root,
            OperationId::Open,
        )?;
        let startup_profile_root_handle = hold_managed_directory(startup_profile_root)?;

        let profiles_root = self.executable_root.join("data/profiles");
        create_managed_directory(&self.executable_root, &profiles_root)?;
        let profiles_root_handle = hold_managed_directory(&profiles_root)?;
        let profile_root = self.executable_root.join("data/profiles/SSE");
        create_managed_directory(&self.executable_root, &profile_root)?;
        let profile_root_handle = hold_managed_directory(&profile_root)?;
        let overlay_path = self.executable_root.join(OVERLAY_STATE_RELATIVE_PATH);

        Ok(WindowsPortableState {
            _bootstrap_ownership: bootstrap_ownership,
            _ownership: ownership,
            _managed_directory_handles: vec![
                executable_root_handle,
                resources_root_handle,
                resources_profiles_root_handle,
                startup_profile_root_handle,
                data_root_handle,
                profiles_root_handle,
                profile_root_handle,
            ],
            executable_root: self.executable_root.clone(),
            startup_defaults,
            overlay_path,
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
    overlay_path: PathBuf,
}

impl PortableState for WindowsPortableState {
    fn load_setup(&mut self) -> Result<SetupState, PortFailure> {
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

        let Some(bytes) = read_optional_managed_file(
            &self.executable_root,
            &self.overlay_path,
            OperationId::LoadSetup,
        )?
        else {
            return Ok(SetupState::default());
        };
        decode_persisted_overlay(&bytes, &self.overlay_path)
    }

    fn persist_setup(&mut self, setup: &SetupState) -> Result<(), PortFailure> {
        let _ = read_optional_managed_file(
            &self.executable_root,
            &self.overlay_path,
            OperationId::PersistSetup,
        )?;
        let encoded = encode_overlay(setup);
        atomic_replace(&self.executable_root, &self.overlay_path, &encoded)
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
fn atomic_replace(root: &Path, destination: &Path, bytes: &[u8]) -> Result<(), PortFailure> {
    let parent = destination.parent().ok_or_else(|| {
        containment_failure(
            OperationId::PersistSetup,
            destination,
            "portable overlay has no parent directory",
        )
    })?;
    validate_existing_managed_path(root, parent, OperationId::PersistSetup)?;
    let temporary = parent.join("overlay.state.pending");
    reject_reparse_if_present(&temporary, OperationId::PersistSetup)?;
    if temporary.exists() {
        std::fs::remove_file(&temporary)
            .map_err(|error| io_port_failure(OperationId::PersistSetup, &temporary, error))?;
    }

    let write_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&temporary)?;
        validate_regular_handle(&file, &temporary, OperationId::PersistSetup)
            .map_err(|failure| io::Error::other(failure.diagnostic().as_str()))?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        // A failed candidate is inactive; cleanup must not replace its more useful write error.
        let _ = std::fs::remove_file(&temporary);
        return Err(io_port_failure(
            OperationId::PersistSetup,
            &temporary,
            error,
        ));
    }

    validate_existing_managed_path(root, parent, OperationId::PersistSetup)?;
    reject_reparse_if_present(destination, OperationId::PersistSetup)?;
    if let Err(error) = windows_api::replace_file(&temporary, destination) {
        // The prior authoritative overlay remains active when the atomic move fails.
        let _ = std::fs::remove_file(&temporary);
        return Err(io_port_failure(
            OperationId::PersistSetup,
            destination,
            error,
        ));
    }
    Ok(())
}

/// Encodes the deliberately small version-one SSE overlay owned by issue 26.
fn encode_overlay(setup: &SetupState) -> Vec<u8> {
    let dry_run = u8::from(setup.profile_overlay().dry_run());
    format!("schema_version=1\ndry_run={dry_run}\n").into_bytes()
}

/// Decodes only the minimal version-one SSE overlay assigned to issue 26.
fn decode_persisted_overlay(bytes: &[u8], path: &Path) -> Result<SetupState, PortFailure> {
    let dry_run = match bytes {
        b"schema_version=1\ndry_run=0\n" => false,
        b"schema_version=1\ndry_run=1\n" => true,
        _ => return Err(corrupt_overlay(path)),
    };
    Ok(SetupState::default().with_profile_overlay(ProfileOverlay::default().with_dry_run(dry_run)))
}

/// Creates the stable corruption result for a malformed portable overlay.
fn corrupt_overlay(path: &Path) -> PortFailure {
    PortFailure::new(
        PortId::PortableState,
        OperationId::LoadSetup,
        FailureKind::CorruptData,
        "portable SSE overlay is not a supported version-one document",
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
        // duration of the call. The flags request replacement on the same managed volume.
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
        ERROR_SHARING_VIOLATION, OVERLAY_STATE_RELATIVE_PATH, STARTUP_DEFAULTS_RELATIVE_PATH,
        WindowsPortableStateFactory,
    };
    use cao_application::{
        ActiveProfileId, ApplicationRuntime, Intent, IntentOutcome, ProfileOverlayEdit,
        SnapshotRevision, SnapshotSink, WorkbenchSnapshot,
    };
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    const WAIT_TIMEOUT: Duration = Duration::from_secs(5);
    const AUTHENTICATED_SSE_STARTUP_DEFAULTS: &[u8] =
        b"schema_version=1\nactive_profile=SSE\ndry_run=0\n";
    const COMMITTED_SSE_STARTUP_DEFAULTS: &[u8] =
        include_bytes!("../../../resources/profiles/SSE/startup.state");
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
    fn missing_sse_startup_resource_is_an_installation_integrity_failure() {
        let sandbox = PortableSandbox::create("missing-resource");
        std::fs::remove_file(sandbox.root().join(STARTUP_DEFAULTS_RELATIVE_PATH))
            .expect("the sandbox resource should be removed");
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
        let profile_root = sandbox.root().join("data/profiles/SSE");
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
