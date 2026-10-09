//! Temporary Ownership: the run's registry of temporary artifacts, and staged
//! publication through one-use receipts.
//!
//! Ported from `src/Run/TemporaryArtifactRegistry.h`. A producer captures its
//! destination before loading the original bytes, stages a durable sibling,
//! writes it, and publishes it through a [`PublicationReceipt`]. Publication
//! revalidates the destination, flushes the staged bytes, renames them over
//! the destination natively on the same volume, and only then releases the
//! temporary name from the ownership manifest. Its [`PublicationResult`]
//! carries the mutation fact either way.
//!
//! In C++ the receipt was move-only and every `publish` consumed it, so a
//! second call reported `NotPublished`. Here [`PublicationReceipt::publish`]
//! takes the receipt by value: a second publication does not compile.
//!
//! Registrations name only temporary paths. Source Assets, committed
//! destinations, backups and retained evidence are never registered, so
//! Safety Cleanup never deletes them.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use cao_winfs::{
    Access, FileFacts, FileIdentity, Open, RenameMode, Share, compare_ordinal_ignore_case,
    msvc_canonical, msvc_weakly_canonical, rename_by_handle,
};

use crate::Error;
use crate::execution::MutationState;
use crate::run::staging::{StagingError, StagingScope, has_staging_component, pin_directory};
use crate::run::{
    CancellationToken, RunFailure, RunFailureCode, RunId, RunPhase, SafetyCleanupService,
};

/// The native rule for an occupied destination leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationPolicy {
    /// Replace an existing destination file: Texture, Mesh and Animation output.
    Replace,
    /// Fail when the destination is occupied, even after preflight: Archive
    /// entries, output Archives and Loading Plugins.
    NoReplace,
}

/// How far one publication got. The states also describe its crash windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationState {
    /// No destination mutation happened. Cleanup or a later recovery removes
    /// only the registered temporary path.
    NotPublished,
    /// The destination was committed, but the manifest snapshot releasing the
    /// temporary name failed. Ownership of that (now absent) name remains.
    PublishedStillOwned,
    /// The destination was committed and the temporary name released.
    PublishedAndReleased,
}

/// The outcome of one publication: its state and, unless it completed, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationResult {
    pub state: PublicationState,
    /// Empty after a completed publication.
    pub error_detail: String,
}

impl PublicationResult {
    /// The destination's mutation fact: None when nothing was published, and
    /// Committed for both published states, even when the release failed.
    pub fn mutation(&self) -> MutationState {
        match self.state {
            PublicationState::NotPublished => MutationState::None,
            _ => MutationState::Committed,
        }
    }

    /// Whether no unknown disk state makes continuing the run dangerous.
    ///
    /// A release failure is unsafe: it belongs to the run's Temporary
    /// Ownership scope, not the output, and the next staging operation would
    /// go through the same snapshot path that just failed. A phase may be
    /// stricter than this verdict, never looser.
    pub fn safe_to_continue(&self) -> bool {
        self.state != PublicationState::PublishedStillOwned
    }
}

/// One registered artifact, retained for cleanup unless committed.
struct Artifact {
    path: PathBuf,
    committed: bool,
    /// Owned by the staging manifest: only publication can release it.
    durable: bool,
}

/// The registry's state, shared with the receipts it issues.
struct Inner {
    id: u64,
    artifacts: Vec<Artifact>,
    /// Safety Cleanup has started: no further registration or publication.
    cleaned: bool,
    staging: StagingScope,
}

/// An opaque handle to one non-durable registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    registry: u64,
    index: usize,
}

/// A staged file and its registration, from [`TemporaryArtifactRegistry::stage_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedFile {
    pub path: PathBuf,
    pub registration: Registration,
}

/// What an Asset destination looked like before its producer loaded the
/// original bytes. Staging and publication reject a changed parent or leaf.
#[derive(Debug)]
#[must_use]
pub struct PublicationTarget {
    root: PathBuf,
    destination: PathBuf,
    parent: FileIdentity,
    leaf: Option<DestinationSnapshot>,
}

/// One-use authority to publish a durable staged file while its registry lives.
///
/// Dropping an unpublished receipt leaves its temporary path owned, so Safety
/// Cleanup or a later recovery removes it.
#[derive(Debug)]
#[must_use = "an unpublished staged file is removed by Safety Cleanup"]
pub struct PublicationReceipt {
    owner: Weak<Mutex<Inner>>,
    index: usize,
    root: PathBuf,
    temporary: PathBuf,
    /// Asset output is bound to the destination captured before staging.
    intended: Option<PathBuf>,
    staged_parent: Option<FileIdentity>,
    staged_leaf: Option<Option<DestinationSnapshot>>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Inner")
            .field("id", &self.id)
            .field("artifacts", &self.artifacts.len())
            .field("cleaned", &self.cleaned)
            .finish_non_exhaustive()
    }
}

impl PublicationReceipt {
    /// The writable staged path, while the owning registry is live.
    ///
    /// # Errors
    /// [`StagingError::Invalid`] after the registry has been dropped.
    pub fn path(&self) -> Result<&Path, StagingError> {
        if self.owner.strong_count() == 0 {
            return Err(StagingError::Invalid(
                "The publication receipt is no longer owned.".to_owned(),
            ));
        }
        Ok(&self.temporary)
    }

    /// Publishes the staged file at `destination` with `policy`, consuming
    /// the receipt whatever happens.
    ///
    /// It never fails outright: every error comes back as the state reached
    /// before it, so a Committed Mutation is never hidden. A dropped registry
    /// returns `NotPublished`.
    pub fn publish(self, destination: &Path, policy: PublicationPolicy) -> PublicationResult {
        let Some(owner) = self.owner.upgrade() else {
            return PublicationResult {
                state: PublicationState::NotPublished,
                error_detail: "The Temporary Ownership scope ended.".to_owned(),
            };
        };
        let mut inner = lock(&owner);
        let mut state = PublicationState::NotPublished;
        match publish(&mut inner, &self, destination, policy, &mut state) {
            Ok(()) => PublicationResult {
                state: PublicationState::PublishedAndReleased,
                error_detail: String::new(),
            },
            Err(error) => PublicationResult {
                state,
                error_detail: error.to_string(),
            },
        }
    }
}

/// Validates, flushes, publishes natively, then releases the temporary name.
/// `state` records how far it got before any error.
fn publish(
    inner: &mut Inner,
    receipt: &PublicationReceipt,
    destination: &Path,
    policy: PublicationPolicy,
    state: &mut PublicationState,
) -> Result<(), StagingError> {
    let owned = !inner.cleaned
        && inner.artifacts.get(receipt.index).is_some_and(|artifact| {
            !artifact.committed && same_artifact_path(&artifact.path, &receipt.temporary)
        });
    if !owned {
        return Err(StagingError::Invalid(
            "The publication receipt is no longer owned.".to_owned(),
        ));
    }
    if let Some(intended) = &receipt.intended
        && !same_artifact_path(destination, intended)
    {
        return Err(StagingError::Invalid(
            "Asset publication changed its staged destination.".to_owned(),
        ));
    }
    let pinned = pin_destination(&receipt.root, destination, receipt.staged_parent.as_ref())?;
    {
        let staged = flush_staged_file(&receipt.temporary, &pinned.parent)?;
        // The saved bytes came from the leaf captured before loading, not a
        // replacement or an in-place edit made while the backend ran.
        if let Some(leaf) = &receipt.staged_leaf
            && destination_snapshot(destination)? != *leaf
        {
            return Err(StagingError::Invalid(
                "Publication destination changed after input capture.".to_owned(),
            ));
        }
        let mode = match policy {
            PublicationPolicy::Replace => RenameMode::Replace,
            PublicationPolicy::NoReplace => RenameMode::NoReplace,
        };
        // Every ancestor stays pinned without delete sharing, so this absolute
        // name still reaches the validated parent.
        rename_by_handle(&staged, destination, mode)
            .map_err(StagingError::io("Publish staged file to", destination))?;
        *state = PublicationState::PublishedStillOwned;
    }
    // The destination is committed. A failed release must keep that fact and
    // leave the old temporary name under durable ownership.
    inner.staging.release_file(&receipt.temporary)?;
    inner.artifacts[receipt.index].committed = true;
    Ok(())
}

/// Owns the explicitly registered temporary paths of one run.
///
/// Register each directory before its children; cleanup never recurses.
/// Commit non-durable retained artifacts; publish durable staged output
/// through a receipt. Dropping the registry releases its OS handles without
/// cleaning up: callers must perform Safety Cleanup explicitly.
#[derive(Debug)]
pub struct TemporaryArtifactRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl TemporaryArtifactRegistry {
    /// An empty registry whose staging names derive from `run_id`.
    pub fn new(run_id: RunId) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                artifacts: Vec::new(),
                cleaned: false,
                staging: StagingScope::new(run_id),
            })),
        }
    }

    /// Checks a canonical Mod Root during Apply Preparing and keeps it pinned.
    ///
    /// Absent staging is not created. Existing staging fails closed until
    /// recovery is ported (#492): `StagingActive` while another process owns
    /// it, `StagingOwnershipUnverified` otherwise. Cancellation stops the
    /// check without a failure.
    ///
    /// # Errors
    /// [`StagingError::Invalid`] after Safety Cleanup closed registration.
    pub fn prepare_root(
        &mut self,
        mod_root: &Path,
        stop: &CancellationToken,
    ) -> Result<Option<RunFailure>, StagingError> {
        let mut inner = self.open()?;
        Ok(inner.staging.prepare_root(mod_root, stop))
    }

    /// Flushes ownership, then creates an empty Asset sibling of
    /// `destination` without publication authority, for recoverable staging
    /// only. Use [`Self::stage_file_for_publication`] for durable output.
    ///
    /// # Errors
    /// Invalid ownership or confinement, an unavailable lock, an I/O failure,
    /// or closed registration.
    pub fn stage_file(
        &mut self,
        mod_root: &Path,
        destination: &Path,
    ) -> Result<StagedFile, StagingError> {
        let mut inner = self.open()?;
        let path = inner.staging.stage_file(mod_root, destination)?;
        Ok(push_durable(&mut inner, path))
    }

    /// Flushes ownership, then creates an empty Archive file beneath the run
    /// child without publication authority.
    ///
    /// # Errors
    /// As for [`Self::stage_file`].
    pub fn stage_archive_file(&mut self, mod_root: &Path) -> Result<StagedFile, StagingError> {
        let mut inner = self.open()?;
        let path = inner.staging.stage_archive_file(mod_root)?;
        Ok(push_durable(&mut inner, path))
    }

    /// Captures an Asset destination before its original bytes are loaded,
    /// without touching the disk. A later stage or publish rejects a changed
    /// parent or leaf.
    ///
    /// # Errors
    /// Invalid confinement, a linked or ambiguous path, an identity lookup
    /// failure, or closed registration.
    pub fn capture_publication_target(
        &self,
        mod_root: &Path,
        destination: &Path,
    ) -> Result<PublicationTarget, StagingError> {
        drop(self.open()?);
        if !mod_root.is_absolute() {
            return Err(StagingError::Invalid(
                "A Mod Root must be absolute.".to_owned(),
            ));
        }
        let root = msvc_canonical(mod_root).map_err(StagingError::lookup("Resolve", mod_root))?;
        let pinned = pin_destination(&root, destination, None)?;
        Ok(PublicationTarget {
            root,
            destination: destination.to_path_buf(),
            parent: pinned.parent,
            leaf: destination_snapshot(destination)?,
        })
    }

    /// Stages a durable Asset sibling for a captured destination.
    ///
    /// # Errors
    /// The destination's parent or leaf changed since capture, staging
    /// ownership cannot be established, or registration is closed.
    pub fn stage_file_for_publication(
        &mut self,
        target: PublicationTarget,
    ) -> Result<PublicationReceipt, StagingError> {
        let mut inner = self.open()?;
        drop(pin_destination(
            &target.root,
            &target.destination,
            Some(&target.parent),
        )?);
        if destination_snapshot(&target.destination)? != target.leaf {
            return Err(StagingError::Invalid(
                "Publication destination changed after input capture.".to_owned(),
            ));
        }
        let path = inner
            .staging
            .stage_file(&target.root, &target.destination)?;
        let index = push_durable(&mut inner, path.clone()).registration.index;
        Ok(PublicationReceipt {
            owner: Arc::downgrade(&self.inner),
            index,
            root: target.root,
            temporary: path,
            intended: Some(target.destination),
            staged_parent: Some(target.parent),
            staged_leaf: Some(target.leaf),
        })
    }

    /// Captures and stages an Asset sibling in one step, for producers that
    /// already hold their input; the receipt still detects a replaced leaf.
    ///
    /// # Errors
    /// As for [`Self::capture_publication_target`] and
    /// [`Self::stage_file_for_publication`].
    pub fn capture_and_stage_file(
        &mut self,
        mod_root: &Path,
        destination: &Path,
    ) -> Result<PublicationReceipt, StagingError> {
        let target = self.capture_publication_target(mod_root, destination)?;
        self.stage_file_for_publication(target)
    }

    /// Stages a durable Archive file whose destination is chosen at
    /// publication. The caller keeps any Archive-specific parent pins.
    ///
    /// # Errors
    /// As for [`Self::stage_archive_file`].
    pub fn stage_archive_file_for_publication(
        &mut self,
        mod_root: &Path,
    ) -> Result<PublicationReceipt, StagingError> {
        let mut inner = self.open()?;
        if !mod_root.is_absolute() {
            return Err(StagingError::Invalid(
                "A Mod Root must be absolute.".to_owned(),
            ));
        }
        let root = msvc_canonical(mod_root).map_err(StagingError::lookup("Resolve", mod_root))?;
        let path = inner.staging.stage_archive_file(&root)?;
        let index = push_durable(&mut inner, path.clone()).registration.index;
        Ok(PublicationReceipt {
            owner: Arc::downgrade(&self.inner),
            index,
            root,
            temporary: path,
            intended: None,
            staged_parent: None,
            staged_leaf: None,
        })
    }

    /// Records an absent absolute path before the caller creates it. Nothing
    /// on disk changes. Keep its parent stable through cleanup.
    ///
    /// # Errors
    /// An existing or already registered path, an ambiguous Windows name, a
    /// lookup failure, or closed registration.
    pub fn register_artifact(&mut self, path: &Path) -> Result<Registration, StagingError> {
        let mut inner = self.open()?;
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(StagingError::Invalid(
                "A temporary artifact needs an absolute entry path.".to_owned(),
            ));
        };
        if !path.is_absolute() || name == "." || name == ".." {
            return Err(StagingError::Invalid(
                "A temporary artifact needs an absolute entry path.".to_owned(),
            ));
        }
        // Resolve parent aliases once, so cleanup never depends on a later
        // working directory.
        let normalized = msvc_weakly_canonical(parent)
            .map_err(StagingError::lookup("Resolve", parent))?
            .join(name);
        // Win32 strips trailing dots and spaces and reads colons as streams, so
        // aliases could otherwise reacquire cleanup ownership of committed output.
        let ambiguous = normalized.components().any(|component| match component {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                name.ends_with('.') || name.ends_with(' ') || name.contains(':')
            }
            _ => false,
        });
        if ambiguous {
            return Err(StagingError::Invalid(
                "A temporary artifact needs an unambiguous Windows path.".to_owned(),
            ));
        }
        match std::fs::symlink_metadata(&normalized) {
            Ok(_) => {
                return Err(StagingError::Invalid(
                    "An existing entry cannot become a temporary artifact.".to_owned(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(StagingError::Io {
                    context: "Inspect temporary artifact",
                    path: normalized,
                    source,
                });
            }
        }
        if inner
            .artifacts
            .iter()
            .any(|artifact| same_artifact_path(&artifact.path, &normalized))
        {
            return Err(StagingError::Invalid(
                "The temporary artifact is already registered.".to_owned(),
            ));
        }
        inner.artifacts.push(Artifact {
            path: normalized,
            committed: false,
            durable: false,
        });
        Ok(Registration {
            registry: inner.id,
            index: inner.artifacts.len() - 1,
        })
    }

    /// Retains a non-durable registered artifact instead of deleting it.
    ///
    /// # Errors
    /// [`StagingError::Invalid`] for a durable, foreign, already committed or
    /// closed registration: only publication releases a durable claim.
    pub fn commit(&mut self, registration: Registration) -> Result<(), StagingError> {
        let mut inner = self.open()?;
        let id = inner.id;
        let Some(artifact) = inner
            .artifacts
            .get_mut(registration.index)
            .filter(|artifact| registration.registry == id && !artifact.committed)
        else {
            return Err(StagingError::Invalid(
                "The temporary artifact registration is no longer owned.".to_owned(),
            ));
        };
        if artifact.durable {
            return Err(StagingError::Invalid(
                "Durable staged files require a publication receipt.".to_owned(),
            ));
        }
        artifact.committed = true;
        Ok(())
    }

    /// Removes the remaining paths once, in reverse registration order and
    /// without recursing, collecting every failure. Later calls return none.
    pub fn cleanup(&mut self) -> Vec<RunFailure> {
        let mut inner = lock(&self.inner);
        if inner.cleaned {
            return Vec::new();
        }
        // Close ownership before touching the disk, so no second pass can
        // retry a failed deletion.
        inner.cleaned = true;
        let mut failures = Vec::new();
        for artifact in inner.artifacts.iter().rev() {
            // The staging scope also knows durable paths whose creation failed
            // before a receipt was returned.
            if artifact.committed || artifact.durable {
                continue;
            }
            if let Err(detail) = remove_registered(&artifact.path) {
                failures.push(
                    RunFailure::new(
                        RunFailureCode::TemporaryArtifactCleanupFailed,
                        RunPhase::SafetyCleanup,
                        detail,
                    )
                    .with_path(&artifact.path),
                );
            }
        }
        failures.extend(inner.staging.cleanup_artifacts());
        failures
    }

    /// Locks the state, refusing once Safety Cleanup has started.
    fn open(&self) -> Result<MutexGuard<'_, Inner>, StagingError> {
        let inner = lock(&self.inner);
        if inner.cleaned {
            return Err(StagingError::Invalid(
                "Temporary artifact registration is closed.".to_owned(),
            ));
        }
        Ok(inner)
    }
}

impl SafetyCleanupService for TemporaryArtifactRegistry {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        Ok(self.cleanup())
    }
}

/// Removes one non-durable registered path without following its contents.
fn remove_registered(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or(path);
    // A link substituted after registration must not redirect a child
    // deletion outside its original parent.
    match msvc_weakly_canonical(parent) {
        Ok(resolved) if same_artifact_path(&resolved, parent) => {}
        Ok(_) => return Err("The temporary artifact parent changed.".to_owned()),
        Err(error) => return Err(error.to_string()),
    }
    let removed = match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
        // Never recurse: unregistered contents may be committed output.
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir(path),
        Ok(_) => std::fs::remove_file(path),
    };
    removed.map_err(|error| error.to_string())
}

/// Records a durable path the staging scope just created.
fn push_durable(inner: &mut Inner, path: PathBuf) -> StagedFile {
    inner.artifacts.push(Artifact {
        path: path.clone(),
        committed: false,
        durable: true,
    });
    StagedFile {
        path,
        registration: Registration {
            registry: inner.id,
            index: inner.artifacts.len() - 1,
        },
    }
}

/// Locks a mutex, recovering the data if a panicking holder poisoned it.
fn lock(mutex: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Compares ownership names conservatively, including Windows case aliases.
fn same_artifact_path(left: &Path, right: &Path) -> bool {
    compare_ordinal_ignore_case(left, right).is_eq()
}

/// A destination leaf: its identity, change metadata and content
/// fingerprint. File IDs and timestamps can survive a same-size rewrite on
/// coarse-timestamp file systems, so the bytes are hashed too.
#[derive(Debug, Clone, Copy)]
struct DestinationSnapshot {
    identity: FileIdentity,
    facts: FileFacts,
    fingerprint: u64,
}

impl PartialEq for DestinationSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.facts.unchanged_since(&other.facts)
            && self.fingerprint == other.fingerprint
    }
}

/// The ordinary directories from a Mod Root down to a destination's parent,
/// pinned against rename, with the parent's identity.
struct PinnedDestination {
    _directories: Vec<File>,
    parent: FileIdentity,
}

/// Rechecks a destination's confinement and pins every ordinary directory
/// from the Mod Root to its parent. `staged_parent` must match the parent.
fn pin_destination(
    root: &Path,
    destination: &Path,
    staged_parent: Option<&FileIdentity>,
) -> Result<PinnedDestination, StagingError> {
    let normal = destination
        .components()
        .all(|component| !matches!(component, Component::CurDir | Component::ParentDir));
    let named = destination
        .file_name()
        .is_some_and(|name| name != "." && name != "..");
    if !root.is_absolute() || !destination.is_absolute() || !normal || !named {
        return Err(StagingError::Invalid(
            "Publication requires an absolute ordinary destination.".to_owned(),
        ));
    }
    let relative = match destination.strip_prefix(root) {
        Ok(relative) if !relative.as_os_str().is_empty() && !has_staging_component(relative) => {
            relative
        }
        _ => {
            return Err(StagingError::Invalid(
                "Publication destination is outside its Mod Root or reserved.".to_owned(),
            ));
        }
    };
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(StagingError::Invalid(
                "Publication destination is outside its Mod Root or reserved.".to_owned(),
            ));
        };
        if ambiguous_windows_name(&name.to_string_lossy()) {
            return Err(StagingError::Invalid(
                "Publication destination has an ambiguous Windows name.".to_owned(),
            ));
        }
    }
    let parent = destination.parent().unwrap_or(destination);
    let unchanged = |path: &Path| msvc_canonical(path).is_ok_and(|resolved| resolved == path);
    if !unchanged(root) || !unchanged(parent) {
        return Err(StagingError::Invalid(
            "Publication destination parent changed or is linked.".to_owned(),
        ));
    }
    let mut directories = vec![pin_directory(root)?];
    let mut current = root.to_path_buf();
    for component in relative.parent().into_iter().flat_map(Path::components) {
        current.push(component);
        directories.push(pin_directory(&current)?);
    }
    let parent_pin = directories.last().expect("the root is always pinned");
    let parent_identity =
        FileIdentity::of(parent_pin).map_err(StagingError::io("Identify", &current))?;
    if staged_parent.is_some_and(|staged| *staged != parent_identity) {
        return Err(StagingError::Invalid(
            "Publication destination parent changed after staging.".to_owned(),
        ));
    }
    Ok(PinnedDestination {
        _directories: directories,
        parent: parent_identity,
    })
}

/// Whether Win32 would alias `name` to another entry, a stream or a device:
/// a trailing dot or space, a reserved character, a control character, or a
/// DOS device name.
fn ambiguous_windows_name(name: &str) -> bool {
    name.is_empty()
        || name.ends_with('.')
        || name.ends_with(' ')
        || name.contains(['<', '>', ':', '"', '|', '?', '*'])
        || name.chars().any(|character| (character as u32) < 32)
        || reserved_device_name(name)
}

/// Whether Win32 resolves `name` to a DOS device, which it does in any
/// directory and with any extension.
///
/// Deviation 15: trailing spaces and dots are trimmed from the stem, so
/// `NUL .pex` is recognized too.
pub(crate) fn reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default();
    let stem = stem.trim_end_matches([' ', '.']).to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut characters = stem.chars();
    let prefix: String = characters.by_ref().take(3).collect();
    let number = characters.next();
    (prefix == "COM" || prefix == "LPT")
        && characters.next().is_none()
        && number.is_some_and(|digit| matches!(digit, '1'..='9' | '¹' | '²' | '³'))
}

/// Reads a destination leaf without following links. `None` when it is
/// absent or a directory; an existing directory is left for the native
/// rename to reject.
///
/// Write and delete sharing are denied while the bytes are hashed, so one
/// snapshot cannot mix two revisions and a rename cannot detach the file.
fn destination_snapshot(path: &Path) -> Result<Option<DestinationSnapshot>, StagingError> {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    let io = |context: &'static str| StagingError::io(context, path);
    let mut leaf = match Open::new(Access::READ_DATA | Access::READ_ATTRIBUTES, Share::READ)
        .directory()
        .open(path)
    {
        Ok(leaf) => leaf,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => return Ok(None),
        Err(error) => return Err(io("Snapshot publication destination")(error)),
    };
    let facts = FileFacts::of(&leaf).map_err(io("Inspect"))?;
    if facts.is_reparse_point() {
        return Err(StagingError::Invalid(
            "Publication destination is linked.".to_owned(),
        ));
    }
    if facts.is_directory() {
        return Ok(None);
    }
    if facts.link_count() != 1 {
        return Err(StagingError::Invalid(
            "Publication requires an ordinary staged file and parent.".to_owned(),
        ));
    }
    let identity = FileIdentity::of(&leaf).map_err(io("Identify"))?;
    let (size, fingerprint) = fingerprint(&mut leaf).map_err(io("Read publication destination"))?;
    if size != facts.size() {
        return Err(StagingError::Invalid(
            "Publication destination changed during snapshot.".to_owned(),
        ));
    }
    Ok(Some(DestinationSnapshot {
        identity,
        facts,
        fingerprint,
    }))
}

/// FNV-1a over every byte `reader` yields, with the byte count.
pub(crate) fn fingerprint(reader: &mut impl Read) -> io::Result<(u64, u64)> {
    let mut buffer = vec![0u8; 65536];
    let mut hash: u64 = 14_695_981_039_346_656_037;
    let mut size = 0u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => return Ok((size, hash)),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        for byte in &buffer[..count] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
        size += count as u64;
    }
}

/// Opens the staged file exclusively and flushes it before any destination
/// mutation, requiring it to share the destination parent's volume: the
/// native rename never falls back to a copy.
fn flush_staged_file(temporary: &Path, parent: &FileIdentity) -> Result<File, StagingError> {
    let io = |context: &'static str| StagingError::io(context, temporary);
    let staged = Open::new(
        Access::WRITE | Access::DELETE | Access::READ_ATTRIBUTES,
        Share::NONE,
    )
    .write_through()
    .open(temporary)
    .map_err(io("Open staged file"))?;
    let facts = FileFacts::of(&staged).map_err(io("Inspect"))?;
    if !facts.is_ordinary_file() || facts.link_count() != 1 {
        return Err(StagingError::Invalid(
            "Publication requires an ordinary staged file and parent.".to_owned(),
        ));
    }
    let identity = FileIdentity::of(&staged).map_err(io("Identify"))?;
    if !identity.same_volume(parent) {
        return Err(StagingError::Invalid(
            "Staged bytes and destination are on different volumes.".to_owned(),
        ));
    }
    staged.sync_all().map_err(io("Flush"))?;
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_recognized_with_any_extension_and_trailing_spaces() {
        for name in [
            "NUL", "nul.pex", "Con.txt", "COM1.esp", "lpt9", "COM¹.x", "NUL .pex",
        ] {
            assert!(reserved_device_name(name), "{name}");
        }
        for name in [
            "NULL.pex",
            "COM0",
            "COM10",
            "console",
            "LPT",
            "aux_file.dds",
        ] {
            assert!(!reserved_device_name(name), "{name}");
        }
    }
}
