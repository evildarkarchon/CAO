//! Archive discovery's preflight and Archive extraction.
//!
//! Ported from `src/Run/ArchiveFirstAssetDiscovery.cpp`,
//! `src/Run/ArchiveExtraction.cpp` and `src/Run/ArchiveCapacity.h`. Archives
//! are read only through the [`ArchiveReader`] seam, and capacity and volume
//! identity only through their probes, so none of this needs `ba2`.
//!
//! The preflight runs before any extraction and is all-or-nothing: every
//! selected Archive's manifest is read, every entry is checked for an Unsafe
//! Game Path (shadowed entries included), Archive Precedence decides each game
//! path's winner, and the Capacity Check runs per volume. Any failure is a Run
//! Failure and no Archive is extracted. Extraction then stages every entry of
//! one Archive and publishes only the entries that Archive won, never
//! replacing an existing file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs::File;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};

use cao_winfs::{
    OrdinalIgnoreCase, compare_ordinal_ignore_case, is_reparse_point, msvc_canonical,
    msvc_weakly_canonical,
};

use crate::execution::MutationState;
use crate::run::artifacts::reserved_device_name;
use crate::run::source_pin::SourceFilePin;
use crate::run::staging::{has_staging_component, pin_directory};
use crate::run::{
    ArchiveAdapters, ArchiveEntry, ArchivePrecedence, ArchiveReader, CapacityProbe,
    PublicationPolicy, PublicationReceipt, PublicationState, RunFailure, RunFailureCode, RunPhase,
    TemporaryArtifactRegistry, VolumeIdentityProbe, is_staging_name, take_panic_message,
};

/// A game path compared as a case-insensitive Windows volume compares names.
///
/// A `BTreeMap` keeps the first key inserted among equal ones, so a map keyed
/// by this retains the first spelling seen.
type GamePathKey = OrdinalIgnoreCase<String>;

/// One game path that two or more Archives in a Mod Root provide.
///
/// It is reported before any extraction, so the user knows which Archive won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveCollision {
    /// The canonical Mod Root the collision belongs to; collisions never span roots.
    pub mod_root: PathBuf,
    /// The game path relative to the Mod Root, `/`-separated, in the winning
    /// Archive's spelling.
    pub game_path: PathBuf,
    /// The Archive with the highest Archive Precedence.
    pub winning_archive: PathBuf,
    /// The losing Archives, from high to low Archive Precedence.
    pub shadowed_archives: Vec<PathBuf>,
    /// Whether a Loose Asset at the game path outranks every Archive.
    pub loose_asset_wins: bool,
}

/// One Archive's manifest and precedence winners, frozen before any Archive
/// is extracted.
///
/// A winner failing later never promotes a shadowed Archive, and a Loose
/// Asset removed later keeps its preflight precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveExtractionPlan {
    pub archive_path: PathBuf,
    /// The canonical Mod Root, independent of the Archive's own directory.
    pub mod_root: PathBuf,
    /// Every manifest entry, canonical and `/`-separated, relative to the
    /// Archive's directory, in manifest order.
    pub entries: Vec<String>,
    /// The entries this Archive wins: no Loose Asset and no Archive of higher
    /// precedence provides them. Only these are published.
    pub merge_entries: Vec<String>,
    /// The staging bytes extraction is estimated to need, overhead included.
    pub estimated_capacity_bytes: u64,
}

/// Why one Archive extraction attempt failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveExtractionFailure {
    /// Reading or staging failed before anything was published.
    ExtractionFailed,
    /// Publishing a winning entry failed.
    MergeFailed,
    /// The source Archive could not be removed or backed up after extraction.
    SourceCleanupFailed,
    /// The Capacity Check failed when rechecked just before staging.
    InsufficientCapacity,
}

/// One Archive extraction attempt's durable evidence. Staged bytes are not a
/// mutation; only published entries are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveExtractionResult {
    pub archive_path: PathBuf,
    /// The frozen preflight scope; the Asset Run sets it from the plan.
    pub mod_root: PathBuf,
    pub mutation: MutationState,
    pub failure: Option<ArchiveExtractionFailure>,
    /// Whether the run may continue. An attempt with `PartialOrUnknown`
    /// mutation stops the run whatever this says.
    pub safe_to_continue: bool,
    pub detail: String,
}

impl ArchiveExtractionResult {
    /// A successful attempt for `plan` that has mutated nothing yet.
    pub fn new(plan: &ArchiveExtractionPlan) -> Self {
        Self {
            archive_path: plan.archive_path.clone(),
            mod_root: plan.mod_root.clone(),
            mutation: MutationState::None,
            failure: None,
            safe_to_continue: true,
            detail: String::new(),
        }
    }

    /// Whether extraction, merge and any requested source cleanup completed.
    pub fn succeeded(&self) -> bool {
        self.failure.is_none()
    }

    /// Whether this attempt must stop the run: it said so, or it left
    /// unknown mutation, whatever it claimed about continuing.
    pub fn is_unsafe(&self) -> bool {
        !self.safe_to_continue || self.mutation == MutationState::PartialOrUnknown
    }
}

/// Stages a whole Archive, then publishes only the entries it won.
///
/// The source Archive is never removed here: backing it up or deleting it
/// belongs to the caller (C++ `BSAOptimizer::extract`).
pub struct ArchiveExtractor<'a> {
    reader: &'a dyn ArchiveReader,
    capacity: &'a dyn CapacityProbe,
}

impl<'a> ArchiveExtractor<'a> {
    /// An extractor reading through `reader` and rechecking capacity through `capacity`.
    pub fn new(reader: &'a dyn ArchiveReader, capacity: &'a dyn CapacityProbe) -> Self {
        Self { reader, capacity }
    }

    /// Extracts one Archive by its frozen plan, never failing outright.
    ///
    /// The manifest and free space are rechecked first, since either can
    /// change after preflight; a shortage stops the attempt before any
    /// staging. A failure before the first published entry reports no
    /// mutation and stays safe to continue; one after it reports
    /// `PartialOrUnknown` and is unsafe. Publication never replaces an
    /// existing entry, so a Loose Asset that appeared after preflight wins.
    /// Staged entries that are not published stay registered, so Safety
    /// Cleanup removes them.
    pub fn extract(
        &self,
        plan: &ArchiveExtractionPlan,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> ArchiveExtractionResult {
        let mut result = ArchiveExtractionResult::new(plan);
        let mut progress = ExtractionProgress::default();
        // A panicking reader is classified like any other failure, by how far
        // the attempt got: `progress` survives the unwind, so a panic while
        // staging stays safe and one after a commit stays partial (C++
        // `catch (...)`).
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.try_extract(plan, artifacts, &mut result, &mut progress)
        }))
        .unwrap_or_else(|payload| Err(take_panic_message(payload)));
        // Whatever happened, nothing reads this Archive again in the attempt,
        // and the caller may remove or rename it next. A panicking release is
        // ignored: the attempt's own outcome is the fact worth reporting, and
        // source cleanup still fails safely on a source left mapped.
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.reader.release())) {
            take_panic_message(payload);
        }
        match outcome {
            Ok(()) => result,
            Err(detail) => {
                result.detail = detail;
                result.failure = Some(if progress.merging {
                    ArchiveExtractionFailure::MergeFailed
                } else {
                    ArchiveExtractionFailure::ExtractionFailed
                });
                // A half-merged Effective Asset Tree must never be optimized
                // as if it were complete.
                result.mutation = if progress.committed {
                    MutationState::PartialOrUnknown
                } else {
                    MutationState::None
                };
                result.safe_to_continue = !progress.committed;
                result
            }
        }
    }

    /// Performs the attempt, recording how far it got in `progress` so the
    /// caller can classify an `Err` detail as an extraction or merge failure.
    ///
    /// A failed capacity recheck is not an `Err`: it returns `Ok` with
    /// `result.failure` already set, because it is never a mutation.
    fn try_extract(
        &self,
        plan: &ArchiveExtractionPlan,
        artifacts: &mut TemporaryArtifactRegistry,
        result: &mut ArchiveExtractionResult,
        progress: &mut ExtractionProgress,
    ) -> Result<(), String> {
        let root = msvc_canonical(&plan.mod_root).map_err(|error| error.to_string())?;
        let source = std::path::absolute(&plan.archive_path).map_err(|error| error.to_string())?;
        let contained = source.strip_prefix(&root).is_ok_and(|relative| {
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
                && !has_staging_component(relative)
        });
        if !contained {
            return Err("Archive source is outside its Mod Root.".to_owned());
        }
        let directory = source.parent().unwrap_or(&root).to_path_buf();

        let entries = self
            .reader
            .list_entries(&source)
            .map_err(|error| error.to_string())?;
        let required = plan
            .estimated_capacity_bytes
            .max(estimated_capacity_bytes(&entries));
        if let Some(available) = self.capacity.available_bytes(&root)
            && available < required
        {
            result.failure = Some(ArchiveExtractionFailure::InsufficientCapacity);
            result.detail = capacity_detail(required, available);
            return Ok(());
        }

        let expected: BTreeSet<&str> = plan.entries.iter().map(String::as_str).collect();
        if expected.len() != plan.entries.len() {
            return Err("Archive manifest contains duplicate canonical entries.".to_owned());
        }
        let mut keys = BTreeSet::new();
        for entry in &expected {
            if canonical_archive_entry_path(entry).as_deref() != Ok(*entry) {
                return Err("Archive plan contains a noncanonical entry.".to_owned());
            }
            if !keys.insert(OrdinalIgnoreCase(*entry)) {
                return Err("Archive manifest contains aliased entries.".to_owned());
            }
        }
        if plan
            .merge_entries
            .iter()
            .any(|entry| !expected.contains(entry.as_str()))
        {
            return Err("Archive merge entry is absent from its manifest.".to_owned());
        }

        // Every entry is staged, shadowed ones too, before anything is published.
        let changed = || "Archive manifest changed after preflight.".to_owned();
        let mut staged: HashMap<String, PublicationReceipt> = HashMap::new();
        for entry in &entries {
            let canonical = canonical_archive_entry_path(&entry.name).map_err(|_| changed())?;
            if !expected.contains(canonical.as_str()) || staged.contains_key(&canonical) {
                return Err(changed());
            }
            let receipt = artifacts
                .stage_archive_file_for_publication(&root)
                .map_err(|error| error.to_string())?;
            let path = receipt
                .path()
                .map_err(|error| error.to_string())?
                .to_path_buf();
            self.reader
                .extract_entry(&source, &entry.name, &path)
                .map_err(|error| error.to_string())?;
            staged.insert(canonical, receipt);
        }
        if staged.len() != expected.len() {
            return Err(changed());
        }

        progress.merging = true;
        for entry in &plan.merge_entries {
            let receipt = staged.remove(entry).ok_or_else(changed)?;
            let target = prepare_merge_target(&root, &join_game_path(&directory, entry))?;
            let publication = receipt.publish(&target.path, PublicationPolicy::NoReplace);
            // Record the commit before inspecting the state, so a release
            // failure on this entry already counts as a partial merge.
            if publication.mutation() == MutationState::Committed {
                progress.committed = true;
            }
            if publication.state != PublicationState::PublishedAndReleased {
                return Err(if publication.error_detail.is_empty() {
                    "Archive publication did not complete.".to_owned()
                } else {
                    publication.error_detail
                });
            }
            result.mutation = MutationState::Committed;
        }
        Ok(())
    }
}

/// What becomes of a source Archive once every entry it won is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCleanup {
    /// Rename it to the first unoccupied `<name>.bak`, `<name>.bak.bak`, ...
    Backup,
    /// Delete it (the "delete backup" option).
    Remove,
}

impl ArchiveExtractor<'_> {
    /// Extracts one Archive, then backs up or removes its source, as C++
    /// `BSAOptimizer::extract` did.
    ///
    /// The source and its ancestors are pinned before the first read and
    /// until cleanup ends, so no read follows a substituted file or parent,
    /// and cleanup acts only on the very file object that was read, unchanged.
    /// The source keeps its name and bytes until the merge has succeeded, so a
    /// failed attempt leaves it recoverable. The reader is released before
    /// cleanup, since a memory-mapped source cannot be deleted.
    ///
    /// A pin that cannot be taken fails the attempt before anything is read.
    /// A failed cleanup is [`ArchiveExtractionFailure::SourceCleanupFailed`]
    /// and unsafe to continue, unless entries were committed and the source is
    /// still the same readable Archive: then the run may go on, because the
    /// retained source is still recovery material for what was extracted.
    /// Otherwise the mutation becomes unknown.
    pub fn extract_with_source_cleanup(
        &self,
        plan: &ArchiveExtractionPlan,
        cleanup: SourceCleanup,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> ArchiveExtractionResult {
        let mut pin = match SourceFilePin::new(&plan.archive_path, &plan.mod_root) {
            Ok(pin) => pin,
            Err(error) => {
                return ArchiveExtractionResult {
                    failure: Some(ArchiveExtractionFailure::ExtractionFailed),
                    detail: error.to_string(),
                    ..ArchiveExtractionResult::new(plan)
                };
            }
        };
        // `extract` releases the reader before it returns.
        let mut result = self.extract(plan, artifacts);
        if !result.succeeded() {
            return result;
        }
        let cleaned = match cleanup {
            SourceCleanup::Remove => pin.remove_if_unchanged(),
            SourceCleanup::Backup => pin.backup_if_unchanged().map(|_| ()),
        };
        match cleaned {
            Ok(()) => result.mutation = MutationState::Committed,
            Err(error) => {
                result.failure = Some(ArchiveExtractionFailure::SourceCleanupFailed);
                result.safe_to_continue = false;
                result.detail = error.to_string();
                // Existence alone cannot prove the retained source is still
                // usable: reopen its manifest, through the same pin, before
                // letting later phases proceed.
                if result.mutation == MutationState::Committed
                    && pin.pin_unchanged_for_recovery().is_ok()
                {
                    let readable = catch_unwind(AssertUnwindSafe(|| {
                        let listed = self.reader.list_entries(pin.path()).is_ok();
                        self.reader.release();
                        listed
                    }));
                    result.safe_to_continue = readable.unwrap_or_else(|payload| {
                        take_panic_message(payload);
                        false
                    });
                }
                if !result.safe_to_continue {
                    result.mutation = MutationState::PartialOrUnknown;
                }
            }
        }
        result
    }
}

/// How far one extraction attempt got, for classifying its failure.
#[derive(Default)]
struct ExtractionProgress {
    /// Staging finished and publication began.
    merging: bool,
    /// At least one entry was published.
    committed: bool,
}

/// A merge destination, with every directory from the Mod Root to its parent
/// pinned against rename until publication finishes.
struct MergeTarget {
    path: PathBuf,
    _pins: Vec<File>,
}

/// Resolves a destination in the existing game-path casing without following
/// directory links, creating only missing parents.
///
/// # Errors
/// An ambiguous name, a destination that appeared since preflight, a linked
/// or non-directory parent, or a path outside the Mod Root.
fn prepare_merge_target(root: &Path, destination: &Path) -> Result<MergeTarget, String> {
    let parts = contained_parts(root, destination)
        .ok_or_else(|| "Archive destination is outside its Mod Root.".to_owned())?;
    let mut current = root.to_path_buf();
    let mut pins = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let existing = matching_child(&current, part)?;
        let found = existing.is_some();
        current = existing.unwrap_or_else(|| current.join(part));
        if index + 1 == parts.len() {
            if found {
                return Err("Archive destination appeared after preflight.".to_owned());
            }
            break;
        }
        if !found {
            std::fs::create_dir(&current).map_err(|error| error.to_string())?;
        }
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err("Archive destination parent is not an ordinary directory.".to_owned());
        }
        pins.push(pin_directory(&current).map_err(|error| error.to_string())?);
    }
    Ok(MergeTarget {
        path: current,
        _pins: pins,
    })
}

/// Rejects a destination already occupied by anything but a Loose Asset at
/// its leaf, or reached through a non-directory or linked parent.
///
/// A Loose Asset at the leaf is authoritative, so its entry needs no merge.
fn validate_existing_destination(
    root: &Path,
    destination: &Path,
    has_loose_file: bool,
) -> Result<(), String> {
    let parts = contained_parts(root, destination)
        .ok_or_else(|| "Archive destination is outside its Mod Root.".to_owned())?;
    let mut current = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        let Some(existing) = matching_child(&current, part)? else {
            return Ok(());
        };
        if index + 1 == parts.len() {
            // A contained file link is a Loose Asset; only the leaf is followed.
            if !has_loose_file || !existing.is_file() {
                return Err("Archive destination is already occupied.".to_owned());
            }
        } else {
            let metadata =
                std::fs::symlink_metadata(&existing).map_err(|error| error.to_string())?;
            if is_reparse_point(&metadata) {
                return Err("Archive destination parent is a reparse point.".to_owned());
            }
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("Archive destination parent is not an ordinary directory.".to_owned());
            }
            current = existing;
        }
    }
    Ok(())
}

/// The one entry of `directory` whose name is Windows-equivalent to `name`.
///
/// # Errors
/// An unreadable directory, or two entries that differ only in case (a
/// case-sensitive directory).
fn matching_child(directory: &Path, name: &OsStr) -> Result<Option<PathBuf>, String> {
    let mut found = None;
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if compare_ordinal_ignore_case(entry.file_name(), name).is_eq() {
            if found.is_some() {
                return Err("Archive destination has ambiguous casing.".to_owned());
            }
            found = Some(entry.path());
        }
    }
    Ok(found)
}

/// The ordinary components of `path` beneath `root`, or `None` when it is the
/// root itself, escapes it, or names reserved staging.
fn contained_parts<'p>(root: &Path, path: &'p Path) -> Option<Vec<&'p OsStr>> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<_>> = relative
        .components()
        .map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    parts.filter(|parts| !parts.is_empty() && !has_staging_component(relative))
}

/// Joins a `/`-separated game path onto a directory, one component at a time.
fn join_game_path(directory: &Path, game_path: &str) -> PathBuf {
    game_path
        .split('/')
        .fold(directory.to_path_buf(), |path, part| path.join(part))
}

/// The `/`-separated game path of a path beneath a Mod Root.
fn game_path_text(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Normalizes a raw manifest name into a contained game path in its original
/// spelling, or rejects it as an Unsafe Game Path.
///
/// Rejected: empty or absolute names, C0 control characters, `: * ? " < > |`,
/// `.`/`..` escapes, a trailing separator, components ending in `.` or a
/// space, staging-name components, and DOS device stems. Deviation 15: a
/// device stem is compared with trailing spaces and dots trimmed, so
/// `NUL .txt` is rejected too.
pub(crate) fn canonical_archive_entry_path(name: &str) -> Result<String, &'static str> {
    let name = name.replace('\\', "/");
    if name.is_empty()
        || name.starts_with('/')
        || name.chars().any(|character| (character as u32) < 32)
        || name.contains([':', '*', '?', '"', '<', '>', '|'])
    {
        return Err("Archive entry has an invalid game path.");
    }
    let segments: Vec<&str> = name.split('/').collect();
    // C++ `lexically_normal` leaves a trailing separator as an empty file
    // name, which names a directory rather than an entry.
    if matches!(segments.last(), Some(&("" | "." | ".."))) {
        return Err("Archive entry escapes its extraction directory.");
    }
    let mut parts: Vec<&str> = Vec::new();
    for segment in segments {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("Archive entry escapes its extraction directory.");
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err("Archive entry escapes its extraction directory.");
    }
    for part in &parts {
        if part.ends_with(['.', ' '])
            || is_staging_name(OsStr::new(part))
            || reserved_device_name(part)
        {
            return Err("Archive entry aliases an unsafe or reserved path.");
        }
    }
    Ok(parts.join("/"))
}

/// The staging bytes an Archive's extraction is estimated to need.
///
/// Each staged payload and its ownership record consume space even when the
/// entry is shadowed, so every entry counts, with a per-entry allowance for
/// metadata and path overhead. The formula is internal; only conservatism is
/// contract. Saturates rather than wrapping a huge Archive into a small one.
pub(crate) fn estimated_capacity_bytes(entries: &[ArchiveEntry]) -> u64 {
    entries.iter().fold(65_536, |total: u64, entry| {
        let overhead = (entry.name.len() as u64)
            .saturating_mul(8)
            .saturating_add(65_536);
        total.saturating_add(entry.decompressed_size.saturating_add(overhead))
    })
}

/// Explains a failed Capacity Check without promising a reservation.
pub(crate) fn capacity_detail(required: u64, available: u64) -> String {
    format!(
        "Insufficient Archive staging capacity: estimated {required} bytes, available \
         {available} bytes. Estimates include staging overhead allowances but do not reserve \
         space or guarantee filesystem metadata, quotas, or concurrent writes."
    )
}

/// Accumulates staging estimates by volume, so each Mod Root is checked only
/// against the work sharing its volume.
///
/// A root with positive work and an unknown volume could share any volume,
/// so it makes every root carry the whole batch's total.
pub(crate) struct CapacityRequirements<'p> {
    probe: &'p dyn VolumeIdentityProbe,
    volumes: BTreeMap<PathBuf, Option<String>>,
    required_by_volume: HashMap<String, u64>,
    total: u64,
    unknown_volume: bool,
}

impl<'p> CapacityRequirements<'p> {
    pub(crate) fn new(probe: &'p dyn VolumeIdentityProbe) -> Self {
        Self {
            probe,
            volumes: BTreeMap::new(),
            required_by_volume: HashMap::new(),
            total: 0,
            unknown_volume: false,
        }
    }

    /// Adds an estimate to the root's volume, probing each root once.
    pub(crate) fn add(&mut self, root: &Path, bytes: u64) {
        let probe = self.probe;
        let volume = self
            .volumes
            .entry(root.to_path_buf())
            .or_insert_with(|| probe.volume_identity(root))
            .clone();
        // A root needing no bytes cannot consume another volume's capacity,
        // even if its identity is unknown.
        if volume.is_none() && bytes != 0 {
            self.unknown_volume = true;
        }
        self.total = self.total.saturating_add(bytes);
        if let Some(volume) = volume {
            let required = self.required_by_volume.entry(volume).or_default();
            *required = required.saturating_add(bytes);
        }
    }

    /// The estimate sharing this previously added root's volume.
    pub(crate) fn required_at(&self, root: &Path) -> u64 {
        match self.volumes.get(root) {
            Some(Some(volume)) if !self.unknown_volume => self.required_by_volume[volume],
            _ => self.total,
        }
    }
}

/// One Mod Root's Archive pass, as the preflight receives it.
pub(crate) struct RootArchives {
    /// The canonical Mod Root.
    pub root: PathBuf,
    /// The selected Archives in default Archive Precedence, high to low:
    /// sorted by relative name, never by filesystem enumeration.
    pub archives: Vec<PathBuf>,
    /// The game paths of the Loose Assets the Archive pass found.
    pub loose: BTreeSet<GamePathKey>,
}

/// What the preflight decided.
pub(crate) enum Preflight {
    /// Every check passed: the frozen plans in extraction order, and the
    /// collisions in Mod Root and game-path order.
    Planned {
        plans: Vec<ArchiveExtractionPlan>,
        collisions: Vec<ArchiveCollision>,
    },
    /// A check failed; no Archive may be extracted.
    Failed(RunFailure),
    /// Cancellation was observed between Archives or entries.
    Cancelled,
}

/// A Run Failure of Archive discovery, attributed to `path`.
fn discovery_failure(code: RunFailureCode, path: &Path, detail: impl Into<String>) -> RunFailure {
    RunFailure::new(code, RunPhase::DiscoveringArchives, detail).with_path(path)
}

/// Plans the extraction of every selected Archive, or fails the whole batch.
///
/// Explicit Archive Precedence is validated per Mod Root first. Then every
/// manifest is read and each entry checked; winners are frozen; collisions
/// are calculated; and the Capacity Check runs. Nothing on disk changes.
/// Only the reader and the probes of `seams` are used; extraction is the
/// caller's. Without `seams`, the first selected Archive fails the run as
/// requested work this build cannot perform.
pub(crate) fn preflight(
    roots: Vec<RootArchives>,
    precedence: &ArchivePrecedence,
    seams: Option<&ArchiveAdapters<'_>>,
    cancelled: &dyn Fn() -> bool,
) -> Preflight {
    let mut roots = roots;
    if let ArchivePrecedence::ExplicitOrder(order) = precedence {
        for root in &mut roots {
            match validate_archive_order(&root.root, &root.archives, order) {
                Ok(ordered) => root.archives = ordered,
                Err(failure) => return Preflight::Failed(failure),
            }
        }
    }

    // Every game path an Archive provides, with its participants from high
    // to low precedence, per Mod Root.
    let mut participants: Vec<BTreeMap<GamePathKey, Vec<PathBuf>>> = Vec::new();
    // Each plan with its root's index and each entry's game path, in entry order.
    let mut plans: Vec<(usize, ArchiveExtractionPlan, Vec<String>)> = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        let mut provided: BTreeMap<GamePathKey, Vec<PathBuf>> = BTreeMap::new();
        for archive in &root.archives {
            if cancelled() {
                return Preflight::Cancelled;
            }
            let Some(seams) = seams else {
                return Preflight::Failed(discovery_failure(
                    RunFailureCode::RequestedWorkUnavailable,
                    archive,
                    "Archive extraction is not available in this build",
                ));
            };
            let listed = catch_unwind(AssertUnwindSafe(|| seams.reader.list_entries(archive)));
            let entries = match listed {
                Ok(Ok(entries)) => entries,
                Ok(Err(error)) => {
                    return Preflight::Failed(discovery_failure(
                        RunFailureCode::ArchiveUnreadable,
                        archive,
                        error.to_string(),
                    ));
                }
                // Nothing has changed on disk, so a panicking reader is just
                // an unreadable Archive.
                Err(payload) => {
                    return Preflight::Failed(discovery_failure(
                        RunFailureCode::ArchiveUnreadable,
                        archive,
                        take_panic_message(payload),
                    ));
                }
            };
            let mut plan = ArchiveExtractionPlan {
                archive_path: archive.clone(),
                mod_root: root.root.clone(),
                entries: Vec::new(),
                merge_entries: Vec::new(),
                estimated_capacity_bytes: estimated_capacity_bytes(&entries),
            };
            let directory = archive.parent().unwrap_or(&root.root);
            let mut own: BTreeSet<GamePathKey> = BTreeSet::new();
            let mut game_paths = Vec::with_capacity(entries.len());
            for entry in &entries {
                if cancelled() {
                    return Preflight::Cancelled;
                }
                let invalid = |detail: &str| {
                    Preflight::Failed(discovery_failure(
                        RunFailureCode::ArchiveEntryInvalid,
                        archive,
                        detail,
                    ))
                };
                let local = match canonical_archive_entry_path(&entry.name) {
                    Ok(local) => local,
                    Err(detail) => return invalid(detail),
                };
                // Extraction writes beside the Archive, so the destination,
                // not the raw name, is what must stay inside the Mod Root.
                let destination = join_game_path(directory, &local);
                let contained = msvc_weakly_canonical(&destination)
                    .is_ok_and(|resolved| resolved.starts_with(&root.root));
                let Some(relative) = destination
                    .strip_prefix(&root.root)
                    .ok()
                    .filter(|_| contained)
                else {
                    return invalid(
                        "Archive entry resolves outside the Mod Root or cannot be resolved.",
                    );
                };
                let game_path = OrdinalIgnoreCase(game_path_text(relative));
                if !own.insert(OrdinalIgnoreCase(game_path.0.clone())) {
                    return invalid("Archive manifest contains aliased entries.");
                }
                if let Err(detail) = validate_existing_destination(
                    &root.root,
                    &destination,
                    root.loose.contains(&game_path),
                ) {
                    return invalid(&detail);
                }
                plan.entries.push(local);
                game_paths.push(game_path.0.clone());
                provided.entry(game_path).or_default().push(archive.clone());
            }
            plans.push((index, plan, game_paths));
        }
        participants.push(provided);
    }

    // Freeze winners: an entry merges only when its Archive is the highest
    // participant and no Loose Asset holds the game path.
    let mut planned: Vec<BTreeSet<GamePathKey>> = vec![BTreeSet::new(); roots.len()];
    for (index, plan, game_paths) in &mut plans {
        let root = &roots[*index];
        for (entry, game_path) in plan.entries.iter().zip(game_paths.iter()) {
            let key = OrdinalIgnoreCase(game_path.clone());
            let wins = participants[*index][&key].first() == Some(&plan.archive_path);
            if !wins || root.loose.contains(&key) {
                continue;
            }
            if conflicts_with_planned(&planned[*index], game_path) {
                return Preflight::Failed(discovery_failure(
                    RunFailureCode::ArchiveEntryInvalid,
                    &plan.archive_path,
                    "Archive entries require a file and directory at the same game path.",
                ));
            }
            planned[*index].insert(key);
            plan.merge_entries.push(entry.clone());
        }
    }

    let mut collisions = Vec::new();
    for (root, provided) in roots.iter().zip(&participants) {
        for (game_path, archives) in provided {
            if archives.len() < 2 {
                continue;
            }
            collisions.push(ArchiveCollision {
                mod_root: root.root.clone(),
                game_path: PathBuf::from(&game_path.0),
                winning_archive: archives[0].clone(),
                shadowed_archives: archives[1..].to_vec(),
                loose_asset_wins: root.loose.contains(game_path),
            });
        }
    }

    // Every Archive read above needed the seams, so no plan exists without them.
    let Some(seams) = seams.filter(|_| !plans.is_empty()) else {
        return Preflight::Planned {
            plans: Vec::new(),
            collisions,
        };
    };
    // A volume must fit all of its roots before extraction begins. No credit
    // is taken for source deletion or for cleanup of shadowed staging.
    let mut requirements = CapacityRequirements::new(seams.volume_identity);
    for (_, plan, _) in &plans {
        requirements.add(&plan.mod_root, plan.estimated_capacity_bytes);
    }
    for (_, plan, _) in &plans {
        let required = requirements.required_at(&plan.mod_root);
        if let Some(available) = seams.capacity.available_bytes(&plan.mod_root)
            && available < required
        {
            return Preflight::Failed(discovery_failure(
                RunFailureCode::ArchiveInsufficientCapacity,
                &plan.mod_root,
                capacity_detail(required, available),
            ));
        }
    }

    Preflight::Planned {
        plans: plans.into_iter().map(|(_, plan, _)| plan).collect(),
        collisions,
    }
}

/// Whether a planned output is this game path's ancestor or descendant: a
/// file and a directory cannot share one game path.
fn conflicts_with_planned(planned: &BTreeSet<GamePathKey>, game_path: &str) -> bool {
    let has_planned_ancestor = game_path
        .match_indices('/')
        .any(|(slash, _)| planned.contains(&OrdinalIgnoreCase(game_path[..slash].to_owned())));
    // Descendants sort contiguously from `game_path/`, so the first one at or
    // after it is the only candidate.
    let has_planned_descendant = planned
        .range(OrdinalIgnoreCase(format!("{game_path}/"))..)
        .next()
        .is_some_and(|child| has_game_path_ancestor(&child.0, game_path));
    has_planned_ancestor || has_planned_descendant
}

/// Whether `ancestor` is a component-boundary prefix of `path`.
///
/// Windows-equivalent names may differ in UTF-8 length, so each prefix is
/// compared rather than slicing by the ancestor's byte count.
fn has_game_path_ancestor(path: &str, ancestor: &str) -> bool {
    path.match_indices('/')
        .any(|(slash, _)| compare_ordinal_ignore_case(&path[..slash], ancestor).is_eq())
}

/// Validates complete high-to-low intent within one Mod Root and applies it.
///
/// Every enabled Archive must be named exactly once, relative to the root.
fn validate_archive_order(
    root: &Path,
    archives: &[PathBuf],
    order: &[PathBuf],
) -> Result<Vec<PathBuf>, RunFailure> {
    let mut ordered: Vec<PathBuf> = Vec::new();
    for requested in order {
        let Some(normalized) = contained_relative(requested) else {
            return Err(discovery_failure(
                RunFailureCode::ArchiveOrderOutsideRoot,
                requested,
                "Archive Precedence paths must be relative and contained in the Mod Root.",
            ));
        };
        let candidate = root.join(normalized);
        if let Ok(resolved) = msvc_canonical(&candidate)
            && !resolved.starts_with(root)
        {
            return Err(discovery_failure(
                RunFailureCode::ArchiveOrderOutsideRoot,
                &candidate,
                "Required Archive resolves outside the Mod Root.",
            ));
        }
        // Match discovered names, not target identity: two contained hard
        // links stay two enabled Archives.
        let Some(found) = archives
            .iter()
            .find(|archive| compare_ordinal_ignore_case(archive, &candidate).is_eq())
        else {
            return Err(discovery_failure(
                RunFailureCode::ArchiveOrderExtra,
                &candidate,
                "Archive Precedence names an Archive that is not enabled in this Mod Root.",
            ));
        };
        if ordered.contains(found) {
            return Err(discovery_failure(
                RunFailureCode::ArchiveOrderDuplicate,
                &candidate,
                "Archive Precedence names the same enabled Archive more than once.",
            ));
        }
        ordered.push(found.clone());
    }
    if ordered.len() != archives.len() {
        return Err(discovery_failure(
            RunFailureCode::ArchiveOrderMissing,
            root,
            "Archive Precedence must include every enabled Archive in this Mod Root.",
        ));
    }
    Ok(ordered)
}

/// Lexically normalizes a relative path, or `None` when it is empty,
/// absolute, or escapes upward.
///
/// A path that normalizes to the root itself, such as `textures/..`, is
/// contained: C++ `lexically_normal` made it `.`, which names no Archive.
fn contained_relative(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let mut parts: Vec<&OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    Some(parts.iter().collect())
}

/// The key ordering Archives within one Mod Root: case-folded first, with the
/// original spelling breaking ties, never filesystem enumeration order.
///
/// C++ folded with utf8proc's full case folding. Lowercasing plus `ß` to `ss`
/// matches it for every ASCII name and the common Latin cases; the corpus
/// keeps game paths ASCII.
pub(crate) fn archive_order_key(relative_name: &str) -> (String, String) {
    let folded: String = relative_name
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace('ß', "ss");
    (folded, relative_name.to_owned())
}

/// The `/`-separated game path of a discovered file beneath its canonical
/// Mod Root. Discovery only visits paths it joined onto that root.
pub(crate) fn relative_game_path(root: &Path, path: &Path) -> String {
    game_path_text(path.strip_prefix(root).unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_names_normalize_to_contained_game_paths() {
        for (name, expected) in [
            ("Textures/Shared.dds", "Textures/Shared.dds"),
            ("TEXTURES\\folder\\..\\.\\SHARED.DDS", "TEXTURES/SHARED.DDS"),
            ("a//b.dds", "a/b.dds"),
        ] {
            assert_eq!(canonical_archive_entry_path(name), Ok(expected.to_owned()));
        }
        for name in [
            "",
            "/a.dds",
            "..\\escaped.dds",
            "a/../../b.dds",
            "textures/",
            "textures/.",
            ".",
            "a:b.dds",
            "bad\u{1}.dds",
            "textures/trailing.",
            "textures/trailing ",
            ".cao-staging/a.dds",
            "aux/file.dds",
        ] {
            assert!(canonical_archive_entry_path(name).is_err(), "{name:?}");
        }
    }

    /// The formula is internal; only conservatism is contract: every entry's
    /// decompressed bytes count, shadowed or not, and a huge Archive never
    /// wraps into a small estimate.
    #[test]
    fn the_capacity_estimate_is_conservative_and_saturates() {
        let entry = |name: &str, size| ArchiveEntry {
            name: name.to_owned(),
            decompressed_size: size,
        };
        let one = [entry("textures/a.dds", 1_000)];
        let two = [
            entry("textures/a.dds", 1_000),
            entry("textures/b.dds", 2_000),
        ];
        assert!(estimated_capacity_bytes(&one) >= 1_000);
        assert!(estimated_capacity_bytes(&two) >= estimated_capacity_bytes(&one) + 2_000);
        let huge = [entry("a", u64::MAX), entry("b", 1)];
        assert_eq!(estimated_capacity_bytes(&huge), u64::MAX);
    }
}
