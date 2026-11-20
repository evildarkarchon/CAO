//! Temporary Ownership: the run's registry of temporary artifacts, and staged
//! publication through one-use receipts.
//!
//! A producer captures its destination before loading the original bytes,
//! stages a durable sibling, writes it, and publishes it through a
//! [`PublicationReceipt`]. Publication revalidates the destination, flushes
//! the staged bytes, renames them over the destination natively on the same
//! volume, and only then releases the temporary name from the ownership
//! manifest. Its [`PublicationResult`] carries the mutation fact either way,
//! and the error when it did not complete.
//!
//! [`PublicationReceipt::publish`] takes the receipt by value, so a staged
//! file is published at most once: a second publication does not compile.
//!
//! Registrations name only temporary paths. Source Assets, committed
//! destinations, backups and retained evidence are never registered, so
//! Safety Cleanup never deletes them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
#[cfg(any(test, feature = "test-support"))]
use std::os::windows::fs::FileTypeExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

#[cfg(any(test, feature = "test-support"))]
use cao_winfs::weakly_canonical;
use cao_winfs::{
    Access, FileFacts, FileIdentity, Open, RenameMode, Share, canonical,
    compare_ordinal_ignore_case, rename_by_handle,
};

use crate::Error;
use crate::execution::MutationState;
use crate::run::path_safety::{DirectoryPins, pin_directory_chain, unsafe_windows_name};
use crate::run::staging::{StagingError, StagingScope, has_staging_component};
use crate::run::{CancellationToken, RunFailure, RunId, SafetyCleanupService};
#[cfg(any(test, feature = "test-support"))]
use crate::run::{RunFailureCode, RunPhase};

/// The native rule for an occupied destination leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationPolicy {
    /// Replace an existing destination file: Texture, Mesh and Animation
    /// output written back over the Asset it was made from.
    Replace,
    /// Fail when the destination is occupied, even after preflight: Archive
    /// entries, output Archives, Loading Plugins, and a Texture conversion,
    /// whose DDS output is a different file from its source.
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

/// The outcome of one publication: how far it got and, unless it completed,
/// the error that stopped it. A publication that did not complete always
/// carries its error, so no caller needs a fallback for a missing one.
#[derive(Debug)]
pub enum PublicationResult {
    /// No destination mutation happened, because of this error. Cleanup or a
    /// later recovery removes only the registered temporary path.
    NotPublished(StagingError),
    /// The destination was committed, but the manifest snapshot releasing the
    /// temporary name failed with this error. Ownership of that (now absent)
    /// name remains.
    PublishedStillOwned(StagingError),
    /// The destination was committed and the temporary name released.
    PublishedAndReleased,
}

impl PublicationResult {
    /// How far the publication got.
    pub fn state(&self) -> PublicationState {
        match self {
            Self::NotPublished(_) => PublicationState::NotPublished,
            Self::PublishedStillOwned(_) => PublicationState::PublishedStillOwned,
            Self::PublishedAndReleased => PublicationState::PublishedAndReleased,
        }
    }

    /// Whether the destination was committed and the temporary name released.
    pub fn completed(&self) -> bool {
        matches!(self, Self::PublishedAndReleased)
    }

    /// Why the publication did not complete, or `None` when it did.
    pub fn error(&self) -> Option<&StagingError> {
        match self {
            Self::NotPublished(error) | Self::PublishedStillOwned(error) => Some(error),
            Self::PublishedAndReleased => None,
        }
    }

    /// The destination's mutation fact: None when nothing was published, and
    /// Committed for both published states, even when the release failed.
    pub fn mutation(&self) -> MutationState {
        match self {
            Self::NotPublished(_) => MutationState::None,
            Self::PublishedStillOwned(_) | Self::PublishedAndReleased => MutationState::Committed,
        }
    }

    /// Whether no unknown disk state makes continuing the run dangerous.
    ///
    /// A release failure is unsafe: it belongs to the run's Temporary
    /// Ownership scope, not the output, and the next staging operation would
    /// go through the same snapshot path that just failed. A phase may be
    /// stricter than this verdict, never looser.
    pub fn safe_to_continue(&self) -> bool {
        !matches!(self, Self::PublishedStillOwned(_))
    }
}

/// One registered artifact, retained for cleanup unless committed.
struct Artifact {
    path: PathBuf,
    committed: bool,
    /// Owned by the staging manifest: only publication can release it. Every
    /// artifact a run registers is; only tests register any other kind.
    #[cfg(any(test, feature = "test-support"))]
    durable: bool,
}

/// The registry's state, shared with the receipts it issues.
struct Inner {
    id: u64,
    artifacts: Vec<Artifact>,
    /// Safety Cleanup has started: no further registration or publication.
    cleaned: bool,
    /// Temporary names whose files a deferred publication already moved to
    /// their destinations, still registered until
    /// [`TemporaryArtifactRegistry::release_published`] releases them together.
    unreleased: Vec<PathBuf>,
    staging: StagingScope,
    /// The directories, keyed by canonical Mod Root, that this run deleted a
    /// file from; see [`TemporaryArtifactRegistry::record_vacated`].
    vacated: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
}

/// When a publication gives up its temporary name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Release {
    /// In its own manifest snapshot, straight after the destination commits.
    Now,
    /// Later, with every other deferred name, in one snapshot.
    Deferred,
}

/// An opaque handle to one registration made without a receipt. Only tests
/// register that way (audit FS-12), so it exists only with `test-support`.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    registry: u64,
    index: usize,
}

/// A staged file and its registration, from
/// [`TemporaryArtifactRegistry::stage_file`]; test-only, like
/// [`Registration`].
#[cfg(any(test, feature = "test-support"))]
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

impl PublicationTarget {
    /// Whether a file already stood at the destination when it was captured.
    ///
    /// A producer whose output is a different file from its input, such as a
    /// Texture conversion, asks this before loading anything: publishing would
    /// replace a file the producer never read. A directory at the destination
    /// does not count; the capture leaves it for the native rename to refuse.
    pub fn is_occupied(&self) -> bool {
        self.leaf.is_some()
    }
}

/// One-use authority to publish a durable staged file while its registry lives.
///
/// A producer that will not publish calls [`Self::abandon`], which removes the
/// staged file and its registration at once; a publication that fails before
/// its rename does the same. Dropping an unpublished receipt leaves its
/// temporary path owned, so Safety Cleanup or a later recovery removes it.
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
    /// It never fails outright: every error comes back inside the state
    /// reached before it, so a Committed Mutation is never hidden. A dropped
    /// registry returns `NotPublished`. A `NotPublished` failure abandons the
    /// staged file, as [`Self::abandon`] does, since nothing can publish it
    /// later.
    pub fn publish(self, destination: &Path, policy: PublicationPolicy) -> PublicationResult {
        let Some(owner) = self.owner.upgrade() else {
            return PublicationResult::NotPublished(StagingError::Invalid(
                "The Temporary Ownership scope ended.".to_owned(),
            ));
        };
        let mut inner = lock(&owner);
        let mut state = PublicationState::NotPublished;
        match publish(
            &mut inner,
            &self,
            destination,
            policy,
            Release::Now,
            &mut state,
        ) {
            Ok(()) => PublicationResult::PublishedAndReleased,
            // `publish` leaves `state` at the rename's outcome: a later error
            // can only be the release's.
            Err(error) if state == PublicationState::NotPublished => {
                PublicationResult::NotPublished(error)
            }
            Err(error) => PublicationResult::PublishedStillOwned(error),
        }
    }

    /// Publishes like [`Self::publish`], but leaves the temporary name
    /// registered for [`TemporaryArtifactRegistry::release_published`] to
    /// release together with the others, consuming the receipt whatever
    /// happens.
    ///
    /// For a producer that publishes many staged files in a row, such as an
    /// Archive's entries: one release snapshot per file rewrites the whole
    /// manifest each time. Between this call and the release, the name is
    /// registered and absent, exactly as after a crash between a publication
    /// and its release, and recovery skips it the same way.
    ///
    /// `Ok` means the destination is a Committed Mutation.
    ///
    /// # Errors
    /// Nothing was published: every failure here precedes the destination
    /// mutation, because no release follows it. The staged file is then
    /// abandoned, as [`Self::abandon`] does.
    pub fn publish_deferring_release(
        self,
        destination: &Path,
        policy: PublicationPolicy,
    ) -> Result<(), StagingError> {
        let Some(owner) = self.owner.upgrade() else {
            return Err(StagingError::Invalid(
                "The Temporary Ownership scope ended.".to_owned(),
            ));
        };
        let mut inner = lock(&owner);
        let mut state = PublicationState::NotPublished;
        publish(
            &mut inner,
            &self,
            destination,
            policy,
            Release::Deferred,
            &mut state,
        )
    }

    /// Deletes the staged file without publishing it and releases its
    /// registration, consuming the receipt.
    ///
    /// For a producer whose attempt failed, or found nothing to change,
    /// after staging: without it the empty sibling would stay beside the
    /// user's Asset, and its record in the growing ownership manifest, until
    /// Safety Cleanup.
    ///
    /// # Errors
    /// The registry was dropped or closed, or the deletion or release
    /// failed. Whatever remains stays owned, so Safety Cleanup or a later
    /// recovery removes it; the caller need not stop for it.
    pub fn abandon(self) -> Result<(), StagingError> {
        let Some(owner) = self.owner.upgrade() else {
            return Err(StagingError::Invalid(
                "The Temporary Ownership scope ended.".to_owned(),
            ));
        };
        let mut inner = lock(&owner);
        if inner.cleaned {
            return Err(StagingError::Invalid(
                "Temporary artifact registration is closed.".to_owned(),
            ));
        }
        inner.staging.abandon_file(&self.temporary)
    }
}

/// Validates, flushes, publishes natively, then releases the temporary name
/// or, for [`Release::Deferred`], records it for a later joint release.
/// `state` records how far it got before any error. A failure before the
/// rename abandons the staged file.
fn publish(
    inner: &mut Inner,
    receipt: &PublicationReceipt,
    destination: &Path,
    policy: PublicationPolicy,
    release: Release,
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
    if let Err(error) = rename_into_place(receipt, destination, policy, state) {
        // Nothing was published, and nothing can publish this staged file
        // later, so it is removed now rather than left to Safety Cleanup. A
        // failed removal leaves it owned, which is where it would have been.
        if let Err(abandon) = inner.staging.abandon_file(&receipt.temporary) {
            log::warn!(
                "{} is left for Safety Cleanup: {abandon}",
                receipt.temporary.display()
            );
        }
        return Err(error);
    }
    // The destination is committed. A failed release must keep that fact and
    // leave the old temporary name under durable ownership.
    match release {
        Release::Now => inner.staging.release_file(&receipt.temporary)?,
        // Nothing here can fail, so a deferred publication that returns an
        // error has not reached the rename in `rename_into_place`.
        Release::Deferred => inner.unreleased.push(receipt.temporary.clone()),
    }
    inner.artifacts[receipt.index].committed = true;
    Ok(())
}

/// Revalidates the destination, flushes the staged file and renames it over
/// the destination natively. `state` becomes `PublishedStillOwned` at the
/// rename, the last step, so an error always means nothing was published.
fn rename_into_place(
    receipt: &PublicationReceipt,
    destination: &Path,
    policy: PublicationPolicy,
    state: &mut PublicationState,
) -> Result<(), StagingError> {
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
                unreleased: Vec::new(),
                staging: StagingScope::new(run_id),
                vacated: BTreeMap::new(),
            })),
        }
    }

    /// Prepares a canonical Mod Root during Apply Preparing, recovering its
    /// leftover staging, and keeps it pinned.
    ///
    /// Absent staging is not created. Leftover staging from a crashed run, v1
    /// to v3 and C++-written alike, is recovered, and the run keeps its lock.
    /// The returned failure names the affected path: `StagingActive` while
    /// another process owns it, `StagingOwnershipUnverified` when ownership
    /// cannot be proven, and `StagingRecoveryFailed` when a removal fails
    /// after deletion began. Proving ownership deletes nothing; see
    /// `StagingScope::prepare_root` for failures during deletion.
    /// Cancellation stops preparation without a failure.
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
    /// only: tests use it to leave staging behind as a crashed run would.
    /// Production output goes through [`Self::stage_file_for_publication`].
    ///
    /// # Errors
    /// Invalid ownership or confinement, an unavailable lock, an I/O failure,
    /// or closed registration.
    #[cfg(any(test, feature = "test-support"))]
    pub fn stage_file(
        &mut self,
        mod_root: &Path,
        destination: &Path,
    ) -> Result<StagedFile, StagingError> {
        let mut inner = self.open()?;
        let path = inner.staging.stage_file(mod_root, destination)?;
        Ok(staged_file(&mut inner, path))
    }

    /// Flushes ownership, then creates an empty Archive file beneath the run
    /// child without publication authority; test-only, like
    /// [`Self::stage_file`].
    ///
    /// # Errors
    /// As for [`Self::stage_file`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn stage_archive_file(&mut self, mod_root: &Path) -> Result<StagedFile, StagingError> {
        let mut inner = self.open()?;
        let path = inner.staging.stage_archive_file(mod_root)?;
        Ok(staged_file(&mut inner, path))
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
        let root = canonical(mod_root).map_err(StagingError::lookup("Resolve", mod_root))?;
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
        // Identity and change metadata only: the publication compares the
        // whole snapshot, bytes included, before it renames anything, so a
        // second hash here would only fail earlier (audit CORE-4).
        let unchanged = match (destination_leaf(&target.destination)?, &target.leaf) {
            (None, None) => true,
            (Some((identity, facts)), Some(leaf)) => {
                identity == leaf.identity && facts.unchanged_since(&leaf.facts)
            }
            _ => false,
        };
        if !unchanged {
            return Err(StagingError::Invalid(
                "Publication destination changed after input capture.".to_owned(),
            ));
        }
        let path = inner
            .staging
            .stage_file(&target.root, &target.destination)?;
        let index = push_durable(&mut inner, path.clone());
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
        let root = canonical(mod_root).map_err(StagingError::lookup("Resolve", mod_root))?;
        let path = inner.staging.stage_archive_file(&root)?;
        let index = push_durable(&mut inner, path.clone());
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

    /// Stages `count` durable Archive files with one ownership snapshot,
    /// returning one receipt each, in creation order. Their destinations are
    /// chosen at publication, as for
    /// [`Self::stage_archive_file_for_publication`].
    ///
    /// Use it for an Archive's entries: staging them one call at a time
    /// rewrites the growing ownership manifest once per entry. A `count` of
    /// zero touches nothing.
    ///
    /// # Errors
    /// As for [`Self::stage_archive_file`]; the manifest's format limit is
    /// checked before any file is created. Files created before a failure
    /// stay owned, for Safety Cleanup.
    pub fn stage_archive_files_for_publication(
        &mut self,
        mod_root: &Path,
        count: usize,
    ) -> Result<Vec<PublicationReceipt>, StagingError> {
        let mut inner = self.open()?;
        if !mod_root.is_absolute() {
            return Err(StagingError::Invalid(
                "A Mod Root must be absolute.".to_owned(),
            ));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let root = canonical(mod_root).map_err(StagingError::lookup("Resolve", mod_root))?;
        let paths = inner.staging.stage_archive_files(&root, count)?;
        Ok(paths
            .into_iter()
            .map(|path| {
                let index = push_durable(&mut inner, path.clone());
                PublicationReceipt {
                    owner: Arc::downgrade(&self.inner),
                    index,
                    root: root.clone(),
                    temporary: path,
                    intended: None,
                    staged_parent: None,
                    staged_leaf: None,
                }
            })
            .collect())
    }

    /// Releases, in one ownership snapshot per Mod Root, every temporary name
    /// whose file [`PublicationReceipt::publish_deferring_release`] has moved
    /// to its destination. With nothing deferred it does nothing.
    ///
    /// # Errors
    /// The release failed and the names stay owned, absent, for a later call
    /// or a later recovery. Like a single publication's failed release, this
    /// belongs to the run's Temporary Ownership scope: the destinations are
    /// already Committed Mutations, and the next staging operation would go
    /// through the same snapshot path that just failed, so the caller must
    /// treat it as unsafe to continue. Also an error once Safety Cleanup has
    /// closed registration.
    pub fn release_published(&mut self) -> Result<(), StagingError> {
        let mut inner = self.open()?;
        if inner.unreleased.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut inner.unreleased);
        let released = inner.staging.release_files(&pending);
        if released.is_err() {
            inner.unreleased = pending;
        }
        released
    }

    /// Records that this run deleted a file directly in `directory`, beneath
    /// the canonical `mod_root`, so Archive Finalization may prune the
    /// directory if that left it empty. Nothing on disk changes.
    ///
    /// Pruning removes only directories a run emptied, never one a mod ships
    /// empty (audit FS-7). The registry keeps the record because it is the
    /// one run-scoped object every phase that deletes a file in a Mod Root
    /// already holds: Archive extraction's source removal and Archive
    /// Finalization's packed-source removal.
    pub(crate) fn record_vacated(&mut self, mod_root: &Path, directory: &Path) {
        lock(&self.inner)
            .vacated
            .entry(mod_root.to_path_buf())
            .or_default()
            .insert(directory.to_path_buf());
    }

    /// The directories beneath the canonical `mod_root` this run recorded
    /// with [`Self::record_vacated`], in path order.
    pub(crate) fn vacated_directories(&self, mod_root: &Path) -> Vec<PathBuf> {
        lock(&self.inner)
            .vacated
            .get(mod_root)
            .map(|directories| directories.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Records an absent absolute path before the caller creates it. Nothing
    /// on disk changes. Keep its parent stable through cleanup.
    ///
    /// Only tests register a path without a receipt, to give Safety Cleanup
    /// something to remove or fail on, so this exists only with
    /// `test-support` (audit FS-12).
    ///
    /// # Errors
    /// An existing or already registered path, an ambiguous Windows name, a
    /// lookup failure, or closed registration.
    #[cfg(any(test, feature = "test-support"))]
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
        let normalized = weakly_canonical(parent)
            .map_err(StagingError::lookup("Resolve", parent))?
            .join(name);
        // Win32 strips trailing dots and spaces, reads colons as streams and
        // names as devices, so aliases could otherwise reacquire cleanup
        // ownership of committed output.
        let ambiguous = normalized.components().any(|component| match component {
            Component::Normal(name) => unsafe_windows_name(&name.to_string_lossy()),
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

    /// Retains a non-durable registered artifact instead of deleting it;
    /// test-only, like [`Self::register_artifact`].
    ///
    /// # Errors
    /// [`StagingError::Invalid`] for a durable, foreign, already committed or
    /// closed registration: only publication releases a durable claim.
    #[cfg(any(test, feature = "test-support"))]
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
        // Only tests register artifacts without a receipt; every one a run
        // stages is durable and removed by the staging scope below.
        #[cfg(any(test, feature = "test-support"))]
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
#[cfg(any(test, feature = "test-support"))]
fn remove_registered(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or(path);
    // A link substituted after registration must not redirect a child
    // deletion outside its original parent.
    match weakly_canonical(parent) {
        Ok(resolved) if same_artifact_path(&resolved, parent) => {}
        Ok(_) => return Err("The temporary artifact parent changed.".to_owned()),
        Err(error) => return Err(error.to_string()),
    }
    let removed = match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
        // Never recurse: unregistered contents may be committed output. A
        // directory link substituted for the registration is removed as the
        // link itself; Windows refuses to delete one as a file.
        Ok(metadata) if metadata.is_dir() || metadata.file_type().is_symlink_dir() => {
            std::fs::remove_dir(path)
        }
        Ok(_) => std::fs::remove_file(path),
    };
    removed.map_err(|error| error.to_string())
}

/// Records a durable path the staging scope just created, returning its
/// index, which only the receipt for it holds.
fn push_durable(inner: &mut Inner, path: PathBuf) -> usize {
    inner.artifacts.push(Artifact {
        path,
        committed: false,
        #[cfg(any(test, feature = "test-support"))]
        durable: true,
    });
    inner.artifacts.len() - 1
}

/// Records a durable path as [`push_durable`] does, for the test-only
/// staging calls that hand back a [`StagedFile`] instead of a receipt.
#[cfg(any(test, feature = "test-support"))]
fn staged_file(inner: &mut Inner, path: PathBuf) -> StagedFile {
    let index = push_durable(inner, path.clone());
    StagedFile {
        path,
        registration: Registration {
            registry: inner.id,
            index,
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
    _directories: DirectoryPins,
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
        if unsafe_windows_name(&name.to_string_lossy()) {
            return Err(StagingError::Invalid(
                "Publication destination has an ambiguous Windows name.".to_owned(),
            ));
        }
    }
    let parent = destination.parent().unwrap_or(destination);
    let unchanged = |path: &Path| canonical(path).is_ok_and(|resolved| resolved == path);
    if !unchanged(root) || !unchanged(parent) {
        return Err(StagingError::Invalid(
            "Publication destination parent changed or is linked.".to_owned(),
        ));
    }
    let mut directories = DirectoryPins::new();
    pin_directory_chain(root, parent, Share::READ | Share::WRITE, &mut directories)?;
    let parent_pin = directories
        .get(parent)
        .expect("the chain ends at the parent it pinned");
    let parent_identity =
        FileIdentity::of(parent_pin).map_err(StagingError::io("Identify", parent))?;
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

/// Why CAO will not replace the Asset whose facts are `facts`, as the
/// [`StagingError::Ineligible`] for `path`, or `None` when it may.
///
/// A link or reparse point, and a file with more than one hard link, are
/// never replaced: a hardlink-deploying mod manager deploys every file as a
/// hard link, and renaming output over one would leave the other names on
/// the old bytes.
pub(crate) fn ineligibility(path: &Path, facts: &FileFacts) -> Option<StagingError> {
    let reason = if facts.is_reparse_point() {
        "The Asset is a link, which CAO does not replace"
    } else if !facts.is_directory() && facts.link_count() != 1 {
        "The Asset has more than one hard link, so CAO does not replace it"
    } else {
        return None;
    };
    Some(StagingError::Ineligible {
        path: path.to_path_buf(),
        reason,
    })
}

/// The [`ineligibility`] of whatever stands at `path` now, read without
/// following a link and without capturing anything: a Dry Run's way to
/// report what Apply's capture would refuse. An absent or unreadable entry
/// is `None`; the attempt meets that later, where Apply would too.
pub(crate) fn probe_ineligibility(path: &Path) -> Option<StagingError> {
    let entry = Open::new(
        Access::READ_ATTRIBUTES,
        Share::READ | Share::WRITE | Share::DELETE,
    )
    .directory()
    .open(path)
    .ok()?;
    let facts = FileFacts::of(&entry).ok()?;
    ineligibility(path, &facts)
}

/// Reads a destination leaf without following links. `None` when it is
/// absent or a directory; an existing directory is left for the native
/// rename to reject.
///
/// Write and delete sharing are denied while the bytes are hashed, so one
/// snapshot cannot mix two revisions and a rename cannot detach the file.
///
/// The leaf is opened without backup semantics. With them, an elevated
/// administrator's open ignores other handles' share modes, so a file another
/// program holds without sharing would be hashed and captured instead of
/// refusing the capture (found on the CI runner; see [`Open::directory`]). A
/// directory cannot be opened that way, so an access-denied leaf is looked at
/// once more through an attribute-only open, which share modes never govern.
fn destination_snapshot(path: &Path) -> Result<Option<DestinationSnapshot>, StagingError> {
    let Some((mut leaf, identity, facts)) = open_destination_leaf(path)? else {
        return Ok(None);
    };
    let io = |context: &'static str| StagingError::io(context, path);
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

/// The identity and change metadata of a destination leaf, opened and judged
/// exactly as [`destination_snapshot`] opens and judges it, without reading
/// its bytes. `None` when it is absent or a directory.
fn destination_leaf(path: &Path) -> Result<Option<(FileIdentity, FileFacts)>, StagingError> {
    Ok(open_destination_leaf(path)?.map(|(_, identity, facts)| (identity, facts)))
}

/// Opens a destination leaf for [`destination_snapshot`] and
/// [`destination_leaf`]: the open file with its identity and facts, `None`
/// when it is absent or a directory, and the leaf's ineligibility as an
/// error. The open is described on [`destination_snapshot`].
fn open_destination_leaf(
    path: &Path,
) -> Result<Option<(File, FileIdentity, FileFacts)>, StagingError> {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    let io = |context: &'static str| StagingError::io(context, path);
    let leaf = match Open::new(Access::READ_DATA | Access::READ_ATTRIBUTES, Share::READ).open(path)
    {
        Ok(leaf) => leaf,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => return Ok(None),
        Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
            return match directory_leaf(path) {
                // An eligible directory has no leaf to open.
                Some(outcome) => outcome.map(|()| None),
                // A file this process may not read: the refusal stands.
                None => Err(io("Snapshot publication destination")(error)),
            };
        }
        Err(error) => return Err(io("Snapshot publication destination")(error)),
    };
    let facts = FileFacts::of(&leaf).map_err(io("Inspect"))?;
    if let Some(ineligible) = ineligibility(path, &facts) {
        return Err(ineligible);
    }
    if facts.is_directory() {
        return Ok(None);
    }
    let identity = FileIdentity::of(&leaf).map_err(io("Identify"))?;
    Ok(Some((leaf, identity, facts)))
}

/// What [`open_destination_leaf`] makes of a leaf its data open was denied:
/// `Some(Ok(()))` for an eligible directory, `Some(Err(..))` for a link or
/// reparse point (which a directory-like leaf such as a junction is), and
/// `None` when the leaf is a file or cannot be inspected, so the denial is the
/// answer.
fn directory_leaf(path: &Path) -> Option<Result<(), StagingError>> {
    let entry = Open::new(
        Access::READ_ATTRIBUTES,
        Share::READ | Share::WRITE | Share::DELETE,
    )
    .directory()
    .open(path)
    .ok()?;
    let facts = FileFacts::of(&entry).ok()?;
    if let Some(ineligible) = ineligibility(path, &facts) {
        return Some(Err(ineligible));
    }
    facts.is_directory().then_some(Ok(()))
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
///
/// The open waits out a sharing violation (FS-5): the backend has only just
/// closed the file, which is when a real-time scanner is most likely to hold
/// it, and refusing then would fail the Asset for nothing.
fn flush_staged_file(temporary: &Path, parent: &FileIdentity) -> Result<File, StagingError> {
    let io = |context: &'static str| StagingError::io(context, temporary);
    let staged = Open::new(
        Access::WRITE | Access::DELETE | Access::READ_ATTRIBUTES,
        Share::NONE,
    )
    .write_through()
    .open_retrying(temporary)
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
    use std::os::windows::fs::OpenOptionsExt;

    use super::*;

    /// A fresh, empty directory under the unit tests' scratch root in the
    /// target directory: `CARGO_TARGET_TMPDIR` exists only for integration
    /// tests.
    fn scratch_dir(name: &str) -> PathBuf {
        crate::test_support::unit_scratch_dir(&format!("artifacts/{name}"))
    }

    /// A file another program holds without sharing refuses the snapshot,
    /// on any host: an elevated administrator's backup-semantics open would
    /// have read it (audit CORE-2's capture relies on the refusal).
    #[test]
    fn an_exclusively_held_destination_refuses_the_snapshot() {
        let dir = scratch_dir("snapshot-held");
        let path = dir.join("held.nif");
        std::fs::write(&path, b"held").unwrap();
        let _holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        let error = destination_snapshot(&path).unwrap_err();

        assert!(
            matches!(&error, StagingError::Io { source, .. } if cao_winfs::is_sharing_violation(source)),
            "{error:?}"
        );
    }

    #[test]
    fn a_directory_destination_has_no_snapshot() {
        let dir = scratch_dir("snapshot-directory");
        let leaf = dir.join("textures");
        std::fs::create_dir(&leaf).unwrap();

        assert!(destination_snapshot(&leaf).unwrap().is_none());
    }

    /// A junction is directory-like, so its data open is denied as a
    /// directory's is; it is still refused as a link, not taken for one.
    #[test]
    fn a_junction_destination_is_ineligible() {
        let dir = scratch_dir("snapshot-junction");
        let target = dir.join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.join("link");
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");

        let error = destination_snapshot(&link).unwrap_err();

        assert!(
            matches!(error, StagingError::Ineligible { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_file_destination_is_snapshotted() {
        let dir = scratch_dir("snapshot-file");
        let path = dir.join("plain.dds");
        std::fs::write(&path, b"plain").unwrap();

        let snapshot = destination_snapshot(&path).unwrap().unwrap();

        assert_eq!(snapshot.facts.size(), 5);
    }
}
