//! Planned Archive Work: the Archive work a Dry Run records instead of
//! attempting it.
//!
//! These are facts, not attempts. Run Evidence keeps them apart from every
//! attempt list, so Run Outcome classification and mutation summaries, which
//! read only attempts, never count them. The two kinds are independent lists:
//! Extracting Archives records planned extractions, and Archive Finalization
//! records planned output Archives, each only while its phase is Planned.

use std::path::PathBuf;

use crate::run::PackedArchiveKind;

/// One Archive extraction a Dry Run planned instead of attempting: what an
/// Apply run would extract from the Archive, and what it would do with the
/// Archive afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedArchiveExtraction {
    /// The Archive that would be extracted.
    pub archive_path: PathBuf,
    /// The canonical Mod Root its entries would be published into.
    pub mod_root: PathBuf,
    /// Every entry in the Archive's manifest.
    pub entry_count: usize,
    /// The entries the Archive wins under Archive Precedence and Loose Asset
    /// precedence: the files the extraction would add.
    pub winner_count: usize,
    /// The staging bytes the extraction is estimated to need, overhead
    /// included, as the Capacity Check counts them.
    pub estimated_capacity_bytes: u64,
    /// Whether Source Cleanup (Delete backups) would remove the Archive
    /// after a successful extraction.
    pub deleted_by_source_cleanup: bool,
}

/// One output Archive a Dry Run planned instead of creating.
///
/// It is an estimate made from the Mod Roots as they are when the Dry Run
/// plans: an Apply run plans after extraction and Loose Asset optimization,
/// so its contents, sizes and possibly names can differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOutputArchive {
    /// The Archive that would be created.
    pub archive_path: PathBuf,
    /// The canonical Mod Root whose Loose Assets it would pack.
    pub mod_root: PathBuf,
    pub kind: PackedArchiveKind,
    /// The Loose Assets it would pack.
    pub file_count: usize,
    /// Their total size on disk in bytes, as planning measured it.
    pub total_bytes: u64,
    /// The Dummy Plugin the Archive would need to load, or `None` when a
    /// Loading Plugin already loads it or Dummy Plugins are not created.
    pub dummy_plugin_path: Option<PathBuf>,
    /// Whether its packed sources would be deleted once it is committed.
    pub deletes_packed_sources: bool,
}

/// What a Dry Run's planned output Archives were planned from, which says
/// how far an Apply run's outputs can differ from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputPlanBasis {
    /// An estimate made from the Mod Roots as they were when the Dry Run
    /// planned. An Apply run plans after Archive extraction and Loose Asset
    /// optimization, which a Dry Run never performs, so the Apply run's
    /// contents and sizes can differ, and so can its names: the source
    /// Archives it would back up or remove are still on disk when the Dry
    /// Run picks names.
    EstimateFromModRootsAsTheyAre,
}

/// The Planned Archive Work one Dry Run recorded, as two independent lists in
/// record order. Empty for an Apply run, and for a Dry Run that planned no
/// Archive work.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlannedArchiveWork {
    extractions: Vec<PlannedArchiveExtraction>,
    outputs: Vec<PlannedOutputArchive>,
    /// Set once Archive Finalization planned Archive creation, before any
    /// output is recorded; `None` when no creation was planned.
    output_basis: Option<OutputPlanBasis>,
}

impl PlannedArchiveWork {
    /// The planned Archive extractions, in the order Extracting Archives
    /// recorded them.
    pub fn extractions(&self) -> &[PlannedArchiveExtraction] {
        &self.extractions
    }

    /// The planned output Archives, in the order Archive Finalization
    /// recorded them. A split Archive appears as several outputs. Every one
    /// is an estimate made from the Mod Roots as they were when planned (see
    /// [`PlannedOutputArchive`]).
    pub fn outputs(&self) -> &[PlannedOutputArchive] {
        &self.outputs
    }

    /// What the planned output Archives were planned from, or `None` when
    /// the Dry Run planned no Archive creation. Set whenever creation was
    /// planned, even when it planned no output: an Apply run could still
    /// create Archives from the files it extracts first.
    pub fn output_basis(&self) -> Option<OutputPlanBasis> {
        self.output_basis
    }

    /// Whether neither list holds a fact.
    pub fn is_empty(&self) -> bool {
        self.extractions.is_empty() && self.outputs.is_empty()
    }

    /// Records what the planned outputs are planned from; Run Evidence
    /// checks the phase and that it is set only once.
    pub(super) fn set_output_basis(&mut self, basis: OutputPlanBasis) {
        self.output_basis = Some(basis);
    }

    /// Appends one planned extraction; Run Evidence checks the phase first.
    pub(super) fn push_extraction(&mut self, extraction: PlannedArchiveExtraction) {
        self.extractions.push(extraction);
    }

    /// Appends one planned output Archive; Run Evidence checks the phase first.
    pub(super) fn push_output(&mut self, output: PlannedOutputArchive) {
        self.outputs.push(output);
    }
}
