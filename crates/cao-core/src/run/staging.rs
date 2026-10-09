//! The staging-ownership protocol's producer side, ported from the producing
//! half of C++ `StagingRecovery` (`src/Run/StagingRecovery.cpp`).
//!
//! See `docs/architecture/staging-ownership.md`. Each Mod Root reserves
//! `.cao-staging`, which holds a stable `owner.lock`, the `ownership.manifest`
//! and one run child, `run-<Run ID>-<nonce>`. Generated Asset siblings named
//! `.cao-staging-<kind>-<Run ID>-<nonce>.<ext>` sit beside their destinations
//! and are registered as `S` records. The writer emits v3 manifests and
//! publishes a complete snapshot before it exclusively creates any newly owned
//! entry, so a crash at any point leaves every temporary path named in the
//! manifest that owns it.
//!
//! [`StagingScope::prepare_root`] recovers a crashed run's leftover staging
//! (#492), v1 to v3 and C++-written alike; see the `recovery` module. It
//! reports `StagingActive` while another process holds the lock, and fails
//! closed with `StagingOwnershipUnverified` on staging it cannot prove it
//! owns, before deleting anything. Once deletion has begun, an entry whose
//! parent changed is still `StagingOwnershipUnverified`, and any other
//! removal failure is `StagingRecoveryFailed`.
//!
//! This scope belongs to one run on its Run Worker. Dropping it releases its
//! pins and the ownership lock without deleting either control file.

mod recovery;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{File, Metadata};
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use cao_winfs::{
    Access, FileFacts, Open, OwnerLock, OwnerLockError, Share, compare_ordinal_ignore_case,
    delete_by_handle, generic_utf8, is_reparse_point, move_file_write_through, msvc_canonical,
    msvc_weakly_canonical, random_nonce, volume_guid_path,
};

use crate::run::{CancellationToken, RunFailure, RunFailureCode, RunId, RunPhase, is_staging_name};
use recovery::Halt;

/// The reserved staging directory beneath every Mod Root.
pub(crate) const STAGING_DIRECTORY: &str = ".cao-staging";
const OWNER_LOCK: &str = "owner.lock";
const MANIFEST: &str = "ownership.manifest";
const MANIFEST_SCRATCH: &str = "ownership.manifest.next";
const CONFLICT_CONTROL: &str = "ownership.conflict";
/// The format bounds recovery enforces; the writer refuses to exceed them.
const MANIFEST_BYTE_LIMIT: usize = 8 * 1024 * 1024;
const REGISTRATION_LIMIT: usize = 100_000;

/// Why a staging operation failed.
///
/// Producers report these as staging failures; only [`StagingError::Ownership`]
/// carries a staging Run Failure code, because it describes a Mod Root's
/// reserved namespace rather than one attempt.
#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    /// The reserved namespace is active, unverifiable or could not be
    /// recovered. `path` is the affected entry.
    #[error("{detail}")]
    Ownership {
        code: RunFailureCode,
        path: PathBuf,
        detail: String,
    },
    /// A path could not be resolved. C++ threw a `filesystem_error` here,
    /// which producers treat as safe to continue.
    #[error("{context} `{}`: {source}", .path.display())]
    Lookup {
        context: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A filesystem operation on a resolved path failed.
    #[error("{context} `{}`: {source}", .path.display())]
    Io {
        context: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// An exclusive creation found the new staging name already occupied by
    /// an entry this run does not own.
    #[error("An unowned entry already occupies a new staging path: {}", .0.display())]
    Collision(PathBuf),
    /// The request breaks the staging contract, such as a destination
    /// outside its Mod Root or a changed destination.
    #[error("{0}")]
    Invalid(String),
}

impl StagingError {
    /// Whether C++ reported this as a `filesystem_error`, the one staging
    /// failure its producers treated as safe to continue.
    pub fn is_lookup(&self) -> bool {
        matches!(self, Self::Lookup { .. })
    }

    /// A fail-closed `StagingOwnershipUnverified` failure for `path`.
    fn unverified(path: &Path, detail: impl Into<String>) -> Self {
        Self::Ownership {
            code: RunFailureCode::StagingOwnershipUnverified,
            path: path.to_path_buf(),
            detail: detail.into(),
        }
    }

    /// A `map_err` adapter turning an I/O error on `path` into [`Self::Io`].
    pub(crate) fn io(context: &'static str, path: &Path) -> impl FnOnce(io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| Self::Io {
            context,
            path,
            source,
        }
    }

    /// A `map_err` adapter turning a failure to resolve `path` into
    /// [`Self::Lookup`], the one staging error producers may continue past.
    pub(crate) fn lookup(context: &'static str, path: &Path) -> impl FnOnce(io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| Self::Lookup {
            context,
            path,
            source,
        }
    }
}

/// One manifest record. `relative` is generic (`/`-separated) UTF-8 text,
/// relative to the reserved directory, or to the Mod Root for an `S` record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Artifact {
    relative: String,
    directory: bool,
    /// An `S` record: a generated Asset sibling relative to the Mod Root.
    root_relative: bool,
}

/// One Mod Root's staging area, once bootstrap has claimed it.
#[derive(Default)]
struct Area {
    /// The run child's name, `run-<Run ID>-<nonce>`; empty until published.
    child: String,
    artifacts: Vec<Artifact>,
    /// Pins the run child until its staged Archive files have been removed.
    child_pin: Option<File>,
    /// Ordinary ancestors of staged siblings, pinned from staging through
    /// Safety Cleanup so none can be renamed into a link.
    parent_pins: BTreeMap<PathBuf, File>,
    /// Bootstrap finished: the initial manifest and the run child exist.
    ready: bool,
}

/// The manifest-owned temporary entries of one run, across its Mod Roots.
pub(crate) struct StagingScope {
    run_id: RunId,
    /// Root and reserved-directory pins plus ownership locks, held until drop.
    pins: Vec<File>,
    locks: Vec<OwnerLock>,
    areas: BTreeMap<PathBuf, Area>,
}

impl StagingScope {
    /// An empty scope whose staging names derive from `run_id`. It touches
    /// nothing until a root is prepared or a file is staged.
    pub(crate) fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            pins: Vec::new(),
            locks: Vec::new(),
            areas: BTreeMap::new(),
        }
    }

    /// Prepares a canonical Mod Root before any work, during Apply Preparing,
    /// and keeps it pinned against rename until this scope is dropped.
    ///
    /// Absent staging is not created. Existing staging is recovered: once its
    /// ownership is proven, every recorded entry still present is removed,
    /// and this scope keeps the recovered `owner.lock` and reuses the area.
    /// A failure names the affected path and says what to do:
    /// `StagingActive` while another process owns the area, and
    /// `StagingOwnershipUnverified` when ownership cannot be proven, which
    /// deletes nothing. After deletion began, the remaining entries stay
    /// owned: an entry whose parent changed is `StagingOwnershipUnverified`,
    /// and any other removal failure `StagingRecoveryFailed`. Cancellation
    /// returns no failure; the executor observes the same token.
    pub(crate) fn prepare_root(
        &mut self,
        mod_root: &Path,
        stop: &CancellationToken,
    ) -> Option<RunFailure> {
        const REMAINING: &str = " Leave remaining staging in place; check permissions and \
                                 ownership before retrying recovery.";
        let staging = mod_root.join(STAGING_DIRECTORY);
        match self.recover(mod_root, &staging, stop) {
            Ok(()) | Err(Halt::Cancelled) => None,
            Err(Halt::Failed(StagingError::Ownership { code, path, detail })) => {
                let guidance = if code == RunFailureCode::StagingActive {
                    " Wait for the owning CAO run to finish, then retry."
                } else {
                    " Leave the contents in place. Inspect ownership.manifest and move \
                     unrecognized material outside .cao-staging before retrying."
                };
                Some(
                    RunFailure::new(code, RunPhase::Preparing, format!("{detail}{guidance}"))
                        .with_path(path),
                )
            }
            // A filesystem error before deletion began still leaves ownership unproven.
            Err(Halt::Failed(error)) => Some(
                RunFailure::new(
                    RunFailureCode::StagingOwnershipUnverified,
                    RunPhase::Preparing,
                    format!("{error}{REMAINING}"),
                )
                .with_path(staging),
            ),
            Err(Halt::RecoveryFailed { path, error }) => Some(
                RunFailure::new(
                    RunFailureCode::StagingRecoveryFailed,
                    RunPhase::Preparing,
                    format!("{error}{REMAINING}"),
                )
                .with_path(path),
            ),
        }
    }

    /// Registers a unique sibling of `destination` durably, then exclusively
    /// creates it empty, returning its path.
    ///
    /// The sibling's extension is the destination's, lowercased, and only
    /// DDS, NIF, BTR, BTO and HKX destinations can be staged. Its parent and
    /// every ancestor below the Mod Root stay pinned until this scope drops.
    pub(crate) fn stage_file(
        &mut self,
        mod_root: &Path,
        destination: &Path,
    ) -> Result<PathBuf, StagingError> {
        if !mod_root.is_absolute() || !destination.is_absolute() {
            return Err(StagingError::Invalid(
                "A staged output must belong to its canonical Mod Root.".to_owned(),
            ));
        }
        let root = msvc_canonical(mod_root).map_err(StagingError::lookup("Resolve", mod_root))?;
        let parent_path = destination.parent().unwrap_or(destination);
        let parent =
            msvc_canonical(parent_path).map_err(StagingError::lookup("Resolve", parent_path))?;
        let Ok(destination_parent) = parent.strip_prefix(&root) else {
            return Err(StagingError::Invalid(
                "A staged output must belong to its canonical Mod Root.".to_owned(),
            ));
        };
        if has_staging_component(destination_parent) || destination.file_name().is_none() {
            return Err(StagingError::Invalid(
                "A staged output must belong to its canonical Mod Root.".to_owned(),
            ));
        }
        let root_volume = volume_guid_path(&root).map_err(StagingError::io("Resolve", &root))?;
        let parent_volume =
            volume_guid_path(&parent).map_err(StagingError::io("Resolve", &parent))?;
        if compare_ordinal_ignore_case(&root_volume, &parent_volume).is_ne() {
            return Err(StagingError::Invalid(
                "The staged output and Mod Root must be on the same volume.".to_owned(),
            ));
        }
        // The manifest grammar is canonical even though routed Asset extensions
        // are case-insensitive.
        let extension = destination
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        let Some(prefix) = staging_prefix(&extension) else {
            return Err(StagingError::Invalid(
                "Asset staging requires a DDS, NIF, BTR, BTO, or HKX destination.".to_owned(),
            ));
        };
        let destination_parent = generic(destination_parent)?;
        self.prepare_area(&root)?;
        let area = self
            .areas
            .get_mut(&root)
            .expect("prepare_area claimed the area");
        if !pin_parent_directories(&root, &parent, &mut area.parent_pins)? {
            return Err(StagingError::unverified(
                &parent,
                "A staged Asset parent disappeared before creation.",
            ));
        }
        let filename = format!("{prefix}{}-{}{extension}", self.run_id, nonce()?);
        let relative = if destination_parent.is_empty() {
            filename
        } else {
            format!("{destination_parent}/{filename}")
        };
        self.create_registered_file(&root, relative, true)
    }

    /// Durably registers and exclusively creates a unique empty file beneath
    /// the owned run child, for Archive entries, output Archives and Loading
    /// Plugins.
    pub(crate) fn stage_archive_file(&mut self, mod_root: &Path) -> Result<PathBuf, StagingError> {
        if !mod_root.is_absolute() {
            return Err(StagingError::Invalid(
                "Archive staging requires an absolute Mod Root.".to_owned(),
            ));
        }
        let root = msvc_canonical(mod_root).map_err(StagingError::lookup("Resolve", mod_root))?;
        self.prepare_area(&root)?;
        let child = &self.areas[&root].child;
        let relative = format!("{child}/archive-entry-{}", nonce()?);
        self.create_registered_file(&root, relative, false)
    }

    /// Removes a temporary registration after publication moved its file to
    /// the destination. The destination is never part of the protocol.
    ///
    /// # Errors
    /// [`StagingError::Invalid`] when the temporary still exists or is not
    /// registered; otherwise the manifest publication's error.
    pub(crate) fn release_file(&mut self, temporary: &Path) -> Result<(), StagingError> {
        for (root, area) in &mut self.areas {
            let Some(position) = area.artifacts.iter().position(|artifact| {
                !artifact.directory && artifact_path(root, artifact) == temporary
            }) else {
                continue;
            };
            if inspect(temporary)?.is_some() {
                return Err(StagingError::Invalid(
                    "A durable temporary file must be moved before releasing ownership.".to_owned(),
                ));
            }
            let mut retained = area.artifacts.clone();
            retained.remove(position);
            publish_manifest(root, &self.run_id, &area.child, &retained)?;
            area.artifacts = retained;
            return Ok(());
        }
        Err(StagingError::Invalid(
            "The durable temporary file is not registered.".to_owned(),
        ))
    }

    /// Removes every registered entry that is still present, in reverse
    /// registration order, attempting all of them despite failures.
    ///
    /// Directories are removed nonrecursively, so unregistered contents
    /// survive. The manifest is not rewritten: entries this pass could not
    /// remove stay owned for a later recovery. Locks stay held until drop.
    pub(crate) fn cleanup_artifacts(&mut self) -> Vec<RunFailure> {
        let mut failures = Vec::new();
        for (root, area) in &mut self.areas {
            if area.child.is_empty() {
                continue;
            }
            // Durable registrations predate creation, so they also cover native
            // write failures before a receipt could be returned.
            for artifact in area.artifacts.iter().rev() {
                let affected = artifact_path(root, artifact);
                let removed = remove_artifact(&affected, artifact, || {
                    // The run child pins Archive staging parents until their
                    // file removals end.
                    if !artifact.root_relative && artifact.relative == area.child {
                        area.child_pin = None;
                    }
                });
                if let Err(error) = removed {
                    failures.push(
                        RunFailure::new(
                            RunFailureCode::TemporaryArtifactCleanupFailed,
                            RunPhase::SafetyCleanup,
                            error.to_string(),
                        )
                        .with_path(affected),
                    );
                }
            }
        }
        failures
    }

    /// Claims the root's reserved directory and publishes its run child,
    /// before any staged bytes exist.
    ///
    /// Only a directory this call creates is claimed: an existing one, even
    /// empty, is never adopted. An interruption before the initial manifest
    /// leaves only control files, and no Asset has been touched.
    fn prepare_area(&mut self, root: &Path) -> Result<(), StagingError> {
        // The manifest grammar admits 1-128 ASCII letters, digits or hyphens.
        // C++ wrote a hex nonce here; a caller-supplied Run ID is checked, so
        // a malformed one can never produce staging recovery would reject.
        let valid_run_id = (1..=128).contains(&self.run_id.len())
            && self
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if !valid_run_id {
            return Err(StagingError::Invalid(format!(
                "The Run ID `{}` cannot name staging.",
                self.run_id
            )));
        }
        if let Some(failure) = self.prepare_root(root, &CancellationToken::new()) {
            return Err(StagingError::Ownership {
                code: failure.code,
                path: failure.path.clone(),
                detail: failure.detail,
            });
        }
        let staging = root.join(STAGING_DIRECTORY);
        if !self.areas.contains_key(root) {
            let root_pin = pin_directory(root)?;
            std::fs::create_dir(&staging).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    StagingError::Invalid(
                        "The reserved staging area appeared during initialization.".to_owned(),
                    )
                } else {
                    StagingError::io("Create", &staging)(error)
                }
            })?;
            let staging_pin = pin_directory(&staging)?;
            let lock = claim_lock(&staging.join(OWNER_LOCK), OwnerLock::create)?;
            self.pins.push(root_pin);
            self.pins.push(staging_pin);
            self.locks.push(lock);
            self.areas.insert(root.to_path_buf(), Area::default());
        }
        let area = self.areas.get_mut(root).expect("the area was just claimed");
        if !area.child.is_empty() && !area.ready {
            return Err(StagingError::Invalid(
                "The staging area initialization did not complete.".to_owned(),
            ));
        }
        if area.child.is_empty() {
            area.child = format!("run-{}-{}", self.run_id, nonce()?);
            area.artifacts = vec![Artifact {
                relative: area.child.clone(),
                directory: true,
                root_relative: false,
            }];
            // Bootstrap may leave only controls if interrupted here; no Asset bytes exist yet.
            publish_manifest(root, &self.run_id, &area.child, &area.artifacts)?;
            let child = staging.join(&area.child);
            std::fs::create_dir(&child).map_err(StagingError::io("Create", &child))?;
            area.child_pin = Some(pin_directory(&child)?);
            area.ready = true;
        }
        Ok(())
    }

    /// Publishes one file registration, then creates the file exclusively.
    ///
    /// A name rejected by `CREATE_NEW` was never acquired, so its registration
    /// is withdrawn again; the colliding entry is never deleted.
    fn create_registered_file(
        &mut self,
        root: &Path,
        relative: String,
        root_relative: bool,
    ) -> Result<PathBuf, StagingError> {
        let area = self
            .areas
            .get_mut(root)
            .expect("prepare_area claimed the area");
        let mut registered = area.artifacts.clone();
        registered.push(Artifact {
            relative,
            directory: false,
            root_relative,
        });
        publish_manifest(root, &self.run_id, &area.child, &registered)?;
        area.artifacts = registered;
        let path = artifact_path(root, area.artifacts.last().expect("just registered"));
        match write_new_file(&path, b"") {
            Ok(()) => Ok(path),
            Err(collision @ StagingError::Collision(_)) => {
                area.artifacts.pop();
                if let Err(error) =
                    publish_manifest(root, &self.run_id, &area.child, &area.artifacts)
                {
                    // An unregistered conflict control makes future recovery
                    // preserve the whole tree when the release could not be
                    // published. A failure to write it changes nothing: the
                    // publication error is what the caller must see.
                    let _ = write_new_file(
                        &root.join(STAGING_DIRECTORY).join(CONFLICT_CONTROL),
                        b"A staged name collided before creation.\n",
                    );
                    return Err(error);
                }
                Err(collision)
            }
            Err(error) => Err(error),
        }
    }
}

/// Removes one present registered entry, checking its parent and type first.
/// `before_directory` runs just before a directory's removal.
fn remove_artifact(
    affected: &Path,
    artifact: &Artifact,
    before_directory: impl FnOnce(),
) -> Result<(), StagingError> {
    let parent = affected.parent().unwrap_or(affected);
    let resolved =
        msvc_weakly_canonical(parent).map_err(StagingError::lookup("Resolve", parent))?;
    if resolved != parent {
        return Err(StagingError::unverified(
            affected,
            "A staging artifact parent changed during cleanup.",
        ));
    }
    let Some(metadata) = inspect(affected)? else {
        return Ok(());
    };
    if metadata.is_dir() != artifact.directory {
        return Err(StagingError::unverified(
            affected,
            "A staging artifact changed its recorded type.",
        ));
    }
    if artifact.directory {
        before_directory();
        // Nonrecursive: any unregistered contents are preserved.
        std::fs::remove_dir(affected).map_err(StagingError::io("Remove", affected))
    } else {
        let file = open_temporary_file(affected)?;
        delete_by_handle(&file).map_err(StagingError::io("Remove", affected))
    }
}

/// The full path of a manifest record, in the namespace its kind selects.
fn artifact_path(root: &Path, artifact: &Artifact) -> PathBuf {
    let base = if artifact.root_relative {
        root.to_path_buf()
    } else {
        root.join(STAGING_DIRECTORY)
    };
    base.join(artifact.relative.replace('/', "\\"))
}

/// The closed sibling namespace of each canonical Asset output extension.
fn staging_prefix(extension: &str) -> Option<&'static str> {
    match extension {
        ".dds" => Some(".cao-staging-texture-"),
        ".hkx" => Some(".cao-staging-animation-"),
        ".nif" | ".btr" | ".bto" => Some(".cao-staging-mesh-"),
        _ => None,
    }
}

/// Whether any component of `path` is in the reserved namespace.
pub(crate) fn has_staging_component(path: &Path) -> bool {
    path.components()
        .any(|component| is_staging_name(component.as_os_str()))
}

/// 32 unpredictable lowercase hexadecimal characters.
fn nonce() -> Result<String, StagingError> {
    let bytes = random_nonce().map_err(|source| StagingError::Io {
        context: "Generate a staging nonce for",
        path: PathBuf::new(),
        source,
    })?;
    let mut text = String::with_capacity(32);
    for byte in bytes {
        // Writing into a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    Ok(text)
}

/// Protocol paths are generic UTF-8, independent of the ANSI code page.
fn generic(path: &Path) -> Result<String, StagingError> {
    generic_utf8(path).map_err(StagingError::io("Encode", path))
}

/// Quotes a manifest string field as C++ `std::quoted` does: `"` and `\` are
/// escaped with `\`, and the result is wrapped in double quotes.
pub(crate) fn quoted(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 2);
    result.push('"');
    for character in text.chars() {
        if character == '"' || character == '\\' {
            result.push('\\');
        }
        result.push(character);
    }
    result.push('"');
    result
}

/// Publishes a complete v3 snapshot: writes and flushes the fixed scratch
/// file exclusively, then replaces the manifest with it on the same volume.
fn publish_manifest(
    root: &Path,
    run_id: &str,
    child: &str,
    artifacts: &[Artifact],
) -> Result<(), StagingError> {
    let staging = root.join(STAGING_DIRECTORY);
    let mut text = format!(
        "CAO-STAGING 3\n{}\n{} {}\n{}\n",
        quoted(&generic(root)?),
        quoted(run_id),
        quoted(child),
        artifacts.len()
    );
    for artifact in artifacts {
        let kind = if artifact.root_relative {
            'S'
        } else if artifact.directory {
            'D'
        } else {
            'F'
        };
        text.push(kind);
        text.push(' ');
        text.push_str(&quoted(&artifact.relative));
        text.push('\n');
    }
    if text.len() > MANIFEST_BYTE_LIMIT || artifacts.len() > REGISTRATION_LIMIT {
        return Err(StagingError::Invalid(
            "The staging ownership manifest exceeds its format limit.".to_owned(),
        ));
    }
    let scratch = staging.join(MANIFEST_SCRATCH);
    let manifest = staging.join(MANIFEST);
    write_new_file(&scratch, text.as_bytes())?;
    move_file_write_through(&scratch, &manifest).map_err(StagingError::io("Publish", &manifest))
}

/// Creates `path` exclusively, never truncating, and flushes `bytes` before
/// returning. A failure after creation keeps the file.
fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), StagingError> {
    const ERROR_FILE_EXISTS: i32 = 80;
    const ERROR_ALREADY_EXISTS: i32 = 183;
    let mut file = Open::new(Access::WRITE, Share::NONE)
        .create_new()
        .open(path)
        .map_err(|error| match error.raw_os_error() {
            Some(ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS) => {
                StagingError::Collision(path.to_path_buf())
            }
            _ => StagingError::io("Create", path)(error),
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(StagingError::io("Write", path))
}

/// Claims `owner.lock` with `claim`, classifying a held lock as active staging.
fn claim_lock(
    path: &Path,
    claim: fn(&Path) -> Result<OwnerLock, OwnerLockError>,
) -> Result<OwnerLock, StagingError> {
    claim(path).map_err(|error| {
        let code = if matches!(error, OwnerLockError::Active(_)) {
            RunFailureCode::StagingActive
        } else {
            RunFailureCode::StagingOwnershipUnverified
        };
        let detail = match &error {
            OwnerLockError::Active(source) | OwnerLockError::Open(source) => {
                format!("{error}: {source}.")
            }
            OwnerLockError::NotOrdinary => format!("{error}."),
        };
        StagingError::Ownership {
            code,
            path: path.to_path_buf(),
            detail,
        }
    })
}

/// Opens an ordinary directory so Windows denies its rename and deletion
/// while the handle lives.
pub(crate) fn pin_directory(path: &Path) -> Result<File, StagingError> {
    let pin = Open::new(
        Access::LIST_DIRECTORY | Access::READ_ATTRIBUTES,
        Share::READ | Share::WRITE,
    )
    .directory()
    .open(path)
    .map_err(StagingError::io("Pin", path))?;
    let facts = FileFacts::of(&pin).map_err(StagingError::io("Inspect", path))?;
    if !facts.is_directory() || facts.is_reparse_point() {
        return Err(StagingError::unverified(
            path,
            "The opened staging entry does not match its expected identity type.",
        ));
    }
    Ok(pin)
}

/// Opens a single-link temporary file with delete access, pinning that
/// identity so deletion can never reach a replacement at its name.
fn open_temporary_file(path: &Path) -> Result<File, StagingError> {
    let file = Open::new(Access::DELETE | Access::READ_ATTRIBUTES, Share::READ)
        .open(path)
        .map_err(StagingError::io("Open", path))?;
    let facts = FileFacts::of(&file).map_err(StagingError::io("Inspect", path))?;
    if !facts.is_ordinary_file() || facts.link_count() != 1 {
        return Err(StagingError::unverified(
            path,
            "The opened staging entry does not match its expected identity type.",
        ));
    }
    Ok(file)
}

/// Inspects an entry without following links: `None` when absent, and an
/// unverified-ownership error for any link, reparse point, unsupported type
/// or hard-linked file.
fn inspect(path: &Path) -> Result<Option<Metadata>, StagingError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StagingError::io("Inspect", path)(error)),
    };
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(StagingError::unverified(
            path,
            "Staging contains a linked entry or reparse point.",
        ));
    }
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(StagingError::unverified(
            path,
            "Staging contains an unsupported entry type.",
        ));
    }
    if metadata.is_file() {
        let file = Open::new(
            Access::READ_ATTRIBUTES,
            Share::READ | Share::WRITE | Share::DELETE,
        )
        .open(path)
        .map_err(StagingError::io("Inspect", path))?;
        let facts = FileFacts::of(&file).map_err(StagingError::io("Inspect", path))?;
        if facts.link_count() != 1 {
            return Err(StagingError::unverified(
                path,
                "Staging contains a hard-linked file.",
            ));
        }
    }
    Ok(Some(metadata))
}

/// Pins each ordinary directory from the pinned Mod Root down to `parent`.
///
/// Returns `false` when a component is absent. Holding every ancestor denies
/// Windows renames and junction substitution while later names are opened.
fn pin_parent_directories(
    root: &Path,
    parent: &Path,
    pins: &mut BTreeMap<PathBuf, File>,
) -> Result<bool, StagingError> {
    let Ok(relative) = parent.strip_prefix(root) else {
        return Err(StagingError::unverified(
            parent,
            "A staging artifact parent left its Mod Root.",
        ));
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(StagingError::unverified(
                parent,
                "A staging artifact parent contains traversal.",
            ));
        };
        current.push(name);
        // The root and each previous component are still pinned while this
        // name is opened.
        if pins.contains_key(&current) {
            continue;
        }
        match inspect(&current)? {
            None => return Ok(false),
            Some(metadata) if !metadata.is_dir() => {
                return Err(StagingError::unverified(
                    &current,
                    "A staging artifact parent is not an ordinary directory.",
                ));
            }
            Some(_) => {}
        }
        pins.insert(current.clone(), pin_directory(&current)?);
    }
    Ok(true)
}
