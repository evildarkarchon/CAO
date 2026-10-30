//! The Capacity Check: an Optimization Run's estimated Archive work compared
//! against the free space on each affected volume.
//!
//! Callers own their estimates; this module owns everything from there to
//! what a shortage means: per-volume accounting, sampling the
//! [`VolumeProbe`] with its panic rule, the per-run freeze of volume
//! identities, the shortage's wording, and the verdict. A caller builds a
//! [`Demand`] of each Mod Root's estimated bytes, names the
//! [`CapacityStage`] it checks at, and records the [`CapacityVerdict`] the
//! [`CapacityCheck`] returns.
//!
//! The rule is decided here, in one place, so that changing it is a
//! one-module change: a shortage before any Archive is extracted is a Run
//! Failure under Apply, a later shortage fails only the Archive work it
//! guards, and a Dry Run warns at both of the Apply run's pre-mutation
//! checks.

use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::routing::ExecutionMode;
use crate::run::{
    RunDiagnostic, RunDiagnosticCode, RunFailure, RunFailureCode, RunPhase, VolumeProbe,
    take_panic_message,
};

/// Where in an Optimization Run a Capacity Check runs, which with the run's
/// Execution Mode decides what a shortage means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapacityStage {
    /// Archive discovery's preflight, before any Archive is extracted.
    BeforeExtraction,
    /// Archive Finalization's phase-wide check before any Mod Root mutates,
    /// and a Dry Run's planning check over the same creation demand.
    BeforeCreation,
    /// A check just before one piece of Archive work: an Archive
    /// extraction attempt, an output attempt, or one Mod Root's Loading
    /// Plugin maintenance. A shortage fails only the work it guards.
    Recheck,
}

/// What a Capacity Check found, decided from its [`CapacityStage`] and the
/// run's Execution Mode, for the caller to record without deciding it.
///
/// | Stage             | Apply     | Dry Run   |
/// |-------------------|-----------|-----------|
/// | Before extraction | Fail run  | Warn      |
/// | Before creation   | Fail work | Warn      |
/// | Recheck           | Fail work | Fail work |
///
/// A Dry Run never reaches a recheck; giving it *Fail work* keeps the
/// verdict total without an impossible arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CapacityVerdict {
    /// The work fits, or free space is unknown.
    Fits,
    /// The run fails before extracting anything: a ready-to-record Archive
    /// Insufficient Capacity Run Failure at Discovering Archives, attributed
    /// to the Mod Root that ran short.
    FailRun(RunFailure),
    /// Only the guarded Archive work fails; its module turns the shortage
    /// into its own Insufficient Capacity Operation Failure.
    FailWork(CapacityShortage),
    /// A Dry Run previews the Apply run's shortage: a ready-to-record
    /// warning-level [`RunDiagnosticCode::CapacityShortage`] whose phase
    /// follows the stage and whose detail names the root's volume.
    Warn(RunDiagnostic),
}

/// Checks estimated Archive work against the free space on each affected
/// volume, through one [`VolumeProbe`].
///
/// Each Mod Root is checked only against the work sharing its volume. A
/// volume identity is looked up the first time any check asks for its root
/// and then frozen, so every check over the same [`FrozenVolumes`] groups
/// roots alike. Free space is never frozen: it is a momentary reading,
/// sampled afresh by every check.
///
/// A probe that cannot answer, even by panicking, gives an unknown answer.
/// Unknown free space never fails a check, and a root on an unknown volume is
/// checked against the whole demand.
///
/// A check is built with its run's Execution Mode, so callers name only the
/// [`CapacityStage`] and get back a [`CapacityVerdict`].
pub(crate) struct CapacityCheck<'p> {
    probe: &'p dyn VolumeProbe,
    volumes: FrozenVolumes,
    /// The run's Execution Mode, which with the stage decides each verdict.
    mode: ExecutionMode,
}

impl<'p> CapacityCheck<'p> {
    /// An Apply run's check over `probe` with volume identities of its own,
    /// frozen for as long as it lives.
    ///
    /// Test-only: every production check shares the run's frozen volumes
    /// through [`Self::sharing`], so a check of its own exists only for this
    /// module's unit tests (#598).
    #[cfg(test)]
    pub(crate) fn new(probe: &'p dyn VolumeProbe) -> Self {
        Self::sharing(probe, &FrozenVolumes::default(), ExecutionMode::Apply)
    }

    /// A check of a run in `mode` over `probe`, sharing `volumes` with every
    /// other check over them, so each Mod Root's volume is looked up at most
    /// once among all of them. Every check sharing `volumes` must be over the
    /// same probe.
    pub(crate) fn sharing(
        probe: &'p dyn VolumeProbe,
        volumes: &FrozenVolumes,
        mode: ExecutionMode,
    ) -> Self {
        Self {
            probe,
            volumes: volumes.clone(),
            mode,
        }
    }

    /// An empty demand, to which the caller adds each Mod Root's estimate.
    pub(crate) fn demand(&self) -> Demand {
        Demand::default()
    }

    /// The verdict at `stage` over `demand`: [`CapacityVerdict::Fits`]
    /// unless a root with demand of its own is known to run short, and
    /// otherwise the verdict for the first such root, in the order roots were
    /// added to `demand`.
    pub(crate) fn check(&self, stage: CapacityStage, demand: &Demand) -> CapacityVerdict {
        self.verdict(stage, self.first_shortage(demand))
    }

    /// The verdict at `stage` for `root` alone, tested against the whole of
    /// `demand` on its volume, so other roots' demand sharing that volume is
    /// charged to it as well.
    pub(crate) fn check_at(
        &self,
        stage: CapacityStage,
        demand: &Demand,
        root: &Path,
    ) -> CapacityVerdict {
        self.verdict(stage, self.shortage_at(demand, root))
    }

    /// [`Self::check_at`] at [`CapacityStage::Recheck`]: the shortage that
    /// fails the one piece of Archive work guarded, or `None` when it fits or
    /// free space is unknown.
    ///
    /// The verdict still comes from the stage table, so a recheck that ever
    /// meant more than failing its work would panic here rather than be
    /// silently recorded as a work failure by every caller.
    pub(crate) fn recheck_at(&self, demand: &Demand, root: &Path) -> Option<CapacityShortage> {
        match self.check_at(CapacityStage::Recheck, demand, root) {
            CapacityVerdict::Fits => None,
            CapacityVerdict::FailWork(shortage) => Some(shortage),
            // A recheck runs only once Archive work may have mutated a Mod
            // Root, where the table never fails the run or merely warns.
            verdict @ (CapacityVerdict::FailRun(_) | CapacityVerdict::Warn(_)) => {
                unreachable!("a recheck fails only the work it guards, never {verdict:?}")
            }
        }
    }

    /// What `shortage`, found at `stage`, means in this check's run: the
    /// whole stage × mode rule, in one table.
    fn verdict(&self, stage: CapacityStage, shortage: Option<CapacityShortage>) -> CapacityVerdict {
        let Some(shortage) = shortage else {
            return CapacityVerdict::Fits;
        };
        match (stage, self.mode) {
            // An Apply run must not start extracting what it cannot fit, so
            // no Mod Root is left half-extracted.
            (CapacityStage::BeforeExtraction, ExecutionMode::Apply) => CapacityVerdict::FailRun(
                RunFailure::new(
                    RunFailureCode::ArchiveInsufficientCapacity,
                    RunPhase::DiscoveringArchives,
                    shortage.to_string(),
                )
                .with_path(&shortage.root)
                .with_capacity_shortage(shortage),
            ),
            // A Dry Run needs no space, so it only previews the shortage and
            // still reports its whole plan.
            (CapacityStage::BeforeExtraction, ExecutionMode::DryRun) => {
                CapacityVerdict::Warn(self.warning(RunPhase::DiscoveringArchives, &shortage))
            }
            (CapacityStage::BeforeCreation, ExecutionMode::DryRun) => {
                CapacityVerdict::Warn(self.warning(RunPhase::ArchiveFinalization, &shortage))
            }
            // Once extraction may have run, the rest of the run is still safe
            // to continue, so a shortage fails only the work it guards.
            (CapacityStage::BeforeCreation, ExecutionMode::Apply) | (CapacityStage::Recheck, _) => {
                CapacityVerdict::FailWork(shortage)
            }
        }
    }

    /// Tests `root` against the whole of `demand` on its volume: the
    /// shortage when that is known not to fit, `None` when it fits or free
    /// space is unknown.
    fn shortage_at(&self, demand: &Demand, root: &Path) -> Option<CapacityShortage> {
        let required = self.required_at(demand, root);
        let available = self.available_bytes(root)?;
        (available < required).then(|| CapacityShortage {
            root: root.to_path_buf(),
            required,
            available,
        })
    }

    /// The first shortage, in the order roots were added to `demand`, among
    /// the roots whose own demand is non-zero.
    ///
    /// A root needing no bytes cannot run out of space, so it is never
    /// checked, and its free space is never sampled.
    fn first_shortage(&self, demand: &Demand) -> Option<CapacityShortage> {
        demand
            .roots
            .iter()
            .filter(|(_, bytes)| *bytes != 0)
            .find_map(|(root, _)| self.shortage_at(demand, root))
    }

    /// The warning-level [`RunDiagnostic`] a Dry Run records, at `phase`,
    /// for a `shortage` its Apply run would fail on: a
    /// [`RunDiagnosticCode::CapacityShortage`] whose path is the shortage's
    /// Mod Root and whose detail names that root's volume, the demand and the
    /// available space.
    ///
    /// The volume is the root's frozen identity, looked up through the probe
    /// only if no check of the run has yet; an unknown or panicking answer
    /// leaves the detail naming "the volume holding this Mod Root" instead.
    /// Only a Dry Run's verdict warns, so an Apply run never looks a volume
    /// up just to describe its failure.
    fn warning(&self, phase: RunPhase, shortage: &CapacityShortage) -> RunDiagnostic {
        let volume = self.volume_identity(&shortage.root);
        shortage.warning(phase, volume.as_deref())
    }

    /// The bytes `root` is checked against: the demand sharing its volume, or
    /// the whole demand when `root`'s volume, or the volume of any other root
    /// with demand, is unknown.
    fn required_at(&self, demand: &Demand, root: &Path) -> u64 {
        // Grouping can only lower a root's requirement below the whole demand
        // when some other root has demand, so only then is a volume looked up.
        if demand
            .roots
            .iter()
            .all(|(other, bytes)| *bytes == 0 || other == root)
        {
            return demand.total;
        }
        let Some(volume) = self.volume_identity(root) else {
            return demand.total;
        };
        let mut required: u64 = 0;
        // A root needing no bytes cannot consume another volume's capacity,
        // even if its identity is unknown, so it is never looked up.
        for (other, bytes) in demand.roots.iter().filter(|(_, bytes)| *bytes != 0) {
            match self.volume_identity(other) {
                // A root with work on an unknown volume could share any
                // volume, so it makes every root carry the whole demand.
                None => return demand.total,
                Some(other_volume) if other_volume == volume => {
                    required = required.saturating_add(*bytes);
                }
                Some(_) => {}
            }
        }
        required
    }

    /// `root`'s frozen volume identity, looked up through the probe the first
    /// time any check sharing these volumes asks for it; `None` when unknown.
    ///
    /// A probe that panics cannot answer, so its panic is caught here and the
    /// volume is unknown, as an `Err` from the production probe already is.
    /// That unknown answer is frozen like any other. An unknown volume keeps
    /// the check conservative: a root on it is checked against the whole
    /// demand. No Run Diagnostic is emitted.
    fn volume_identity(&self, root: &Path) -> Option<String> {
        if let Some(volume) = self.volumes.lock().get(root) {
            return volume.clone();
        }
        // The probe runs outside the lock, inside its own containment, so a
        // panicking probe can never poison the frozen volumes and a slow one
        // never holds the lock while it queries the volume.
        let volume = contained_probe(|| self.probe.volume_identity(root));
        // Should another check have frozen this root meanwhile, its answer
        // stands, so every check sees one identity per root.
        self.volumes
            .lock()
            .entry(root.to_path_buf())
            .or_insert(volume)
            .clone()
    }

    /// Samples `root`'s free space afresh, `None` when it is unknown.
    ///
    /// A probe that panics cannot answer, so its panic is caught here and the
    /// free space is unknown, as an `Err` from the production probe already
    /// is. The check is advisory: unknown
    /// free space never fails it, and the staged and atomic writers still
    /// handle real I/O errors, so no Run Diagnostic is emitted and the panic
    /// never escapes to fail the run or the attempt it guards.
    fn available_bytes(&self, root: &Path) -> Option<u64> {
        contained_probe(|| self.probe.available_bytes(root))
    }
}

/// Runs one [`VolumeProbe`] query, turning a panic into the unknown answer
/// `None`.
///
/// Both of the probe's answers share the panic rule: a probe that cannot
/// answer, even by panicking, gives an unknown answer, so the advisory check
/// never fails the run or the attempt it guards. The payload is disposed of
/// through [`take_panic_message`] and its message dropped, since an unknown
/// answer emits no Run Diagnostic. `AssertUnwindSafe` is sound because a
/// panicking query leaves nothing of the check half-updated: callers touch
/// no shared state inside `query`.
fn contained_probe<T>(query: impl FnOnce() -> Option<T>) -> Option<T> {
    catch_unwind(AssertUnwindSafe(query)).unwrap_or_else(|payload| {
        take_panic_message(payload);
        None
    })
}

/// The volume identities one Optimization Run's Capacity Checks share, each
/// frozen the first time a check asks for its Mod Root. Clones share them.
///
/// The map sits behind a `Mutex` so checks work through `&self`.
#[derive(Clone, Default)]
pub(crate) struct FrozenVolumes(Arc<Mutex<BTreeMap<PathBuf, Option<String>>>>);

impl FrozenVolumes {
    /// The frozen identities. No probe is ever called under this lock, so it
    /// cannot be poisoned by a probe; a poisoned map is still a valid one,
    /// since every insertion is a single complete entry.
    fn lock(&self) -> MutexGuard<'_, BTreeMap<PathBuf, Option<String>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Each Mod Root's estimated Archive work for one Capacity Check, in the
/// order roots were first added.
#[derive(Debug, Clone, Default)]
pub(crate) struct Demand {
    /// Each root once, with its summed estimate.
    roots: Vec<(PathBuf, u64)>,
    /// Every root's estimate summed.
    total: u64,
}

impl Demand {
    /// Adds an estimate of `bytes` to `root`'s demand. A root keeps the place
    /// of its first addition, even one of no bytes. Sums saturate rather
    /// than wrapping a huge estimate into a small one.
    pub(crate) fn add(&mut self, root: &Path, bytes: u64) {
        self.total = self.total.saturating_add(bytes);
        match self.roots.iter_mut().find(|(added, _)| added == root) {
            Some((_, demand)) => *demand = demand.saturating_add(bytes),
            None => self.roots.push((root.to_path_buf(), bytes)),
        }
    }
}

/// A Capacity Check that found too little space at a Mod Root. Its
/// [`Display`](fmt::Display) form is the detail of every capacity Run Failure
/// and Operation Failure.
///
/// Run Evidence carries it beside that detail on every capacity Run Failure,
/// Run Diagnostic and Operation Failure, so the bytes are read as data rather
/// than parsed out of text. It derives `Hash` because the
/// [`OperationFailure`](crate::OperationFailure) carrying it does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapacityShortage {
    /// The Mod Root that ran short.
    pub root: PathBuf,
    /// The estimated bytes the root was checked against.
    pub required: u64,
    /// The free space sampled at the root.
    pub available: u64,
}

impl CapacityShortage {
    /// The warning-level [`RunDiagnosticCode::CapacityShortage`] at `phase`
    /// for this shortage, attributed to its Mod Root, naming `volume`, the
    /// root's volume identity, or the volume holding the root when that is
    /// unknown. Unlike the [`Display`](fmt::Display) form, which is an Apply
    /// run's failure detail, it reads as a preview of that failure.
    fn warning(&self, phase: RunPhase, volume: Option<&str>) -> RunDiagnostic {
        let Self {
            root,
            required,
            available,
        } = self;
        let volume = match volume {
            Some(volume) => format!("volume {volume}"),
            None => "the volume holding this Mod Root".to_owned(),
        };
        RunDiagnostic::new(
            RunDiagnosticCode::CapacityShortage,
            phase,
            format!(
                "Capacity shortage: an Apply run would need an estimated {required} bytes on \
                 {volume}, but {available} bytes are available, so it would fail its Capacity \
                 Check. The Dry Run itself needs no space."
            ),
        )
        .with_path(root)
        .with_capacity_shortage(self.clone())
    }
}

impl fmt::Display for CapacityShortage {
    /// Explains a failed Capacity Check without promising a reservation.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            required,
            available,
            ..
        } = self;
        write!(
            formatter,
            "Insufficient Archive staging capacity: estimated {required} bytes, available \
             {available} bytes. Estimates include staging overhead allowances but do not reserve \
             space or guarantee filesystem metadata, quotas, or concurrent writes."
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    /// A probe answering from fixed tables, `None` for every root it does not
    /// name, panicking for the roots named in `panicking`, and counting its
    /// volume lookups per root.
    #[derive(Default)]
    struct TableProbe {
        available: BTreeMap<PathBuf, u64>,
        volumes: BTreeMap<PathBuf, String>,
        panicking: Vec<PathBuf>,
        lookups: Mutex<BTreeMap<PathBuf, usize>>,
    }

    impl TableProbe {
        fn with_available(mut self, root: &str, bytes: u64) -> Self {
            self.available.insert(PathBuf::from(root), bytes);
            self
        }

        fn with_volume(mut self, root: &str, volume: &str) -> Self {
            self.volumes.insert(PathBuf::from(root), volume.to_owned());
            self
        }

        fn panicking_at(mut self, root: &str) -> Self {
            self.panicking.push(PathBuf::from(root));
            self
        }

        /// How many times `root`'s volume was looked up.
        fn lookups(&self, root: &str) -> usize {
            let lookups = self.lookups.lock().unwrap();
            lookups.get(Path::new(root)).copied().unwrap_or(0)
        }
    }

    impl VolumeProbe for TableProbe {
        fn available_bytes(&self, root: &Path) -> Option<u64> {
            assert!(
                !self.panicking.iter().any(|panicking| panicking == root),
                "the free-space query panicked"
            );
            self.available.get(root).copied()
        }

        fn volume_identity(&self, root: &Path) -> Option<String> {
            *self
                .lookups
                .lock()
                .unwrap()
                .entry(root.to_path_buf())
                .or_default() += 1;
            assert!(
                !self.panicking.iter().any(|panicking| panicking == root),
                "the volume query panicked"
            );
            self.volumes.get(root).cloned()
        }
    }

    /// A demand of `bytes` at each root, in order.
    fn demand_of(check: &CapacityCheck<'_>, roots: &[(&str, u64)]) -> Demand {
        let mut demand = check.demand();
        for (root, bytes) in roots {
            demand.add(Path::new(root), *bytes);
        }
        demand
    }

    fn shortage(root: &str, required: u64, available: u64) -> Option<CapacityShortage> {
        Some(CapacityShortage {
            root: PathBuf::from(root),
            required,
            available,
        })
    }

    /// Rust-only, for #593: a root is checked only against the demand sharing
    /// its volume.
    #[test]
    fn each_root_is_checked_against_the_demand_on_its_volume() {
        let probe = TableProbe::default()
            .with_volume("a", "c")
            .with_volume("b", "c")
            .with_volume("d", "d")
            .with_available("a", 250)
            .with_available("b", 250)
            .with_available("d", 150);
        let check = CapacityCheck::new(&probe);
        let demand = demand_of(&check, &[("a", 100), ("b", 200), ("d", 150)]);

        assert_eq!(
            check.shortage_at(&demand, Path::new("a")),
            shortage("a", 300, 250)
        );
        assert_eq!(check.shortage_at(&demand, Path::new("d")), None);
        assert_eq!(check.first_shortage(&demand), shortage("a", 300, 250));
    }

    /// Rust-only, for #593: a root with work on an unknown volume could share
    /// any volume, so every root carries the whole demand.
    #[test]
    fn an_unknown_volume_with_work_charges_every_root_the_whole_demand() {
        let probe = TableProbe::default()
            .with_volume("a", "c")
            .with_available("a", 250)
            .with_available("b", 250);
        let check = CapacityCheck::new(&probe);
        let demand = demand_of(&check, &[("a", 100), ("b", 200)]);

        assert_eq!(
            check.shortage_at(&demand, Path::new("a")),
            shortage("a", 300, 250)
        );
        assert_eq!(
            check.shortage_at(&demand, Path::new("b")),
            shortage("b", 300, 250)
        );
    }

    /// Rust-only, for #593: a root needing no bytes is never checked, and its
    /// unknown volume does not inflate the others' demand.
    #[test]
    fn a_root_without_demand_is_neither_checked_nor_charged() {
        let probe = TableProbe::default()
            .with_volume("a", "c")
            .with_volume("b", "c")
            .with_available("a", 250)
            .with_available("b", 250)
            .with_available("idle", 0);
        let check = CapacityCheck::new(&probe);
        let demand = demand_of(&check, &[("idle", 0), ("a", 100), ("b", 100), ("idle", 0)]);

        assert_eq!(check.first_shortage(&demand), None);
        assert_eq!(check.shortage_at(&demand, Path::new("a")), None);
        assert_eq!(probe.lookups("idle"), 0);
    }

    /// Rust-only, for #593: a probe that panics gives an unknown answer.
    /// Unknown free space passes, and an unknown volume charges the whole
    /// demand.
    #[test]
    fn a_panicking_probe_is_an_unknown_answer() {
        let probe = TableProbe::default()
            .with_volume("b", "c")
            .with_available("b", 250)
            .panicking_at("a");
        let check = CapacityCheck::new(&probe);
        let demand = demand_of(&check, &[("a", 100), ("b", 200)]);

        assert_eq!(check.shortage_at(&demand, Path::new("a")), None);
        assert_eq!(check.first_shortage(&demand), shortage("b", 300, 250));
        // The panic poisoned nothing: the frozen volumes still answer.
        assert_eq!(
            check.shortage_at(&demand, Path::new("b")),
            shortage("b", 300, 250)
        );
        assert_eq!(probe.lookups("a"), 1);
    }

    /// Rust-only, for #593: each root's volume is looked up once, then frozen
    /// for every check sharing the run's volumes, while free space is sampled
    /// afresh each time.
    #[test]
    fn each_root_is_looked_up_once_across_every_check_of_a_run() {
        let probe = TableProbe::default()
            .with_volume("a", "c")
            .with_volume("b", "d")
            .with_available("a", 1_000)
            .with_available("b", 1_000);
        let volumes = FrozenVolumes::default();
        for _ in 0..3 {
            let check = CapacityCheck::sharing(&probe, &volumes, ExecutionMode::Apply);
            let demand = demand_of(&check, &[("a", 100), ("b", 200), ("a", 100)]);
            assert_eq!(check.first_shortage(&demand), None);
            assert_eq!(check.shortage_at(&demand, Path::new("b")), None);
        }

        assert_eq!(probe.lookups("a"), 1);
        assert_eq!(probe.lookups("b"), 1);
        // A check with volumes of its own looks each root up again.
        let fresh = CapacityCheck::new(&probe);
        let demand = demand_of(&fresh, &[("a", 100), ("b", 200)]);
        assert_eq!(fresh.first_shortage(&demand), None);
        assert_eq!(probe.lookups("a"), 2);
    }

    /// The verdict a check in `mode` over `probe` gives for `stage`, through
    /// both entry points: the first short root of a demand of 300 bytes at
    /// `root`, and `root` itself against that demand. Both must agree.
    fn verdict_of(
        probe: &TableProbe,
        mode: ExecutionMode,
        stage: CapacityStage,
        root: &str,
    ) -> CapacityVerdict {
        let check = CapacityCheck::sharing(probe, &FrozenVolumes::default(), mode);
        let demand = demand_of(&check, &[(root, 300)]);
        let verdict = check.check(stage, &demand);
        assert_eq!(check.check_at(stage, &demand, Path::new(root)), verdict);
        verdict
    }

    /// #655: the Capacity Check decides what a shortage means from the stage
    /// it runs at and the run's Execution Mode, one verdict per cell. A Dry
    /// Run never reaches a recheck; its cell fails the work so the verdict
    /// stays total.
    #[test]
    fn each_stage_and_mode_has_its_own_verdict() {
        use CapacityStage::{BeforeCreation, BeforeExtraction, Recheck};
        use ExecutionMode::{Apply, DryRun};

        /// What one cell's verdict must be, before its contents are checked.
        #[derive(Debug, PartialEq)]
        enum Expected {
            FailRun,
            FailWork,
            Warn(RunPhase),
        }

        let probe = TableProbe::default()
            .with_volume("a", "c")
            .with_available("a", 250);
        let shortage = CapacityShortage {
            root: PathBuf::from("a"),
            required: 300,
            available: 250,
        };
        for (stage, mode, expected) in [
            (BeforeExtraction, Apply, Expected::FailRun),
            (
                BeforeExtraction,
                DryRun,
                Expected::Warn(RunPhase::DiscoveringArchives),
            ),
            (BeforeCreation, Apply, Expected::FailWork),
            (
                BeforeCreation,
                DryRun,
                Expected::Warn(RunPhase::ArchiveFinalization),
            ),
            (Recheck, Apply, Expected::FailWork),
            (Recheck, DryRun, Expected::FailWork),
        ] {
            let cell = format!("{stage:?} in {mode:?}");
            match verdict_of(&probe, mode, stage, "a") {
                CapacityVerdict::FailRun(failure) => {
                    assert_eq!(expected, Expected::FailRun, "{cell}");
                    assert_eq!(
                        failure.code,
                        RunFailureCode::ArchiveInsufficientCapacity,
                        "{cell}"
                    );
                    assert_eq!(failure.phase, RunPhase::DiscoveringArchives, "{cell}");
                    assert_eq!(failure.path, Path::new("a"), "{cell}");
                    assert_eq!(failure.detail, shortage.to_string(), "{cell}");
                    assert_eq!(failure.capacity_shortage.as_deref(), Some(&shortage));
                }
                CapacityVerdict::FailWork(found) => {
                    assert_eq!(expected, Expected::FailWork, "{cell}");
                    assert_eq!(found, shortage, "{cell}");
                }
                CapacityVerdict::Warn(warning) => {
                    assert_eq!(expected, Expected::Warn(warning.phase), "{cell}");
                    assert_eq!(warning.code, RunDiagnosticCode::CapacityShortage);
                    assert_eq!(warning.path, Path::new("a"), "{cell}");
                    assert_eq!(warning.capacity_shortage.as_ref(), Some(&shortage));
                }
                CapacityVerdict::Fits => panic!("{cell}: a known shortage fits"),
            }
        }
    }

    /// A recheck hands its caller the shortage that fails the guarded work,
    /// in either mode, and nothing when the work fits or its space is
    /// unknown.
    #[test]
    fn a_recheck_yields_only_the_shortage_that_fails_its_work() {
        let probe = TableProbe::default()
            .with_volume("short", "c")
            .with_available("short", 250)
            .with_volume("roomy", "d")
            .with_available("roomy", 300);
        for mode in [ExecutionMode::Apply, ExecutionMode::DryRun] {
            let check = CapacityCheck::sharing(&probe, &FrozenVolumes::default(), mode);
            for (root, expected) in [
                ("short", shortage("short", 300, 250)),
                ("roomy", None),
                ("unknown", None),
            ] {
                let demand = demand_of(&check, &[(root, 300)]);
                assert_eq!(
                    check.recheck_at(&demand, Path::new(root)),
                    expected,
                    "{root} in {mode:?}"
                );
            }
        }
    }

    /// #655: work that fits, or whose free space is unknown, fits at every
    /// stage in either mode.
    #[test]
    fn work_that_fits_or_has_unknown_space_fits_at_every_stage() {
        let probe = TableProbe::default()
            .with_volume("roomy", "c")
            .with_available("roomy", 300)
            .panicking_at("panicking");
        for stage in [
            CapacityStage::BeforeExtraction,
            CapacityStage::BeforeCreation,
            CapacityStage::Recheck,
        ] {
            for mode in [ExecutionMode::Apply, ExecutionMode::DryRun] {
                for root in ["roomy", "unknown", "panicking"] {
                    assert_eq!(
                        verdict_of(&probe, mode, stage, root),
                        CapacityVerdict::Fits,
                        "{root} at {stage:?} in {mode:?}"
                    );
                }
            }
        }
    }

    /// #642: a Dry Run's capacity warning is attributed to the Mod Root and
    /// names its volume, or the volume holding it when the identity is
    /// unknown, with the demand and the available space. This is the one
    /// word-for-word check of the warning's text.
    #[test]
    fn a_shortage_warning_names_the_volume_the_demand_and_the_space() {
        let probe = TableProbe::default()
            .with_volume("known", "volume-1")
            .with_available("known", 250)
            .with_available("unknown", 250);
        for (root, volume) in [
            ("known", "volume volume-1"),
            ("unknown", "the volume holding this Mod Root"),
        ] {
            let CapacityVerdict::Warn(warning) = verdict_of(
                &probe,
                ExecutionMode::DryRun,
                CapacityStage::BeforeExtraction,
                root,
            ) else {
                panic!("a Dry Run warns of a shortage before extraction");
            };

            assert_eq!(warning.path, Path::new(root));
            assert_eq!(
                warning.detail,
                format!(
                    "Capacity shortage: an Apply run would need an estimated 300 bytes on \
                     {volume}, but 250 bytes are available, so it would fail its Capacity \
                     Check. The Dry Run itself needs no space."
                )
            );
        }
    }

    /// For #593: the shortage explains itself without promising a
    /// reservation, so a user who frees exactly the estimate is not misled
    /// into expecting the run to fit. This is the one word-for-word check of
    /// the Apply failure detail, which every capacity Run Failure and
    /// Operation Failure carries.
    #[test]
    fn a_shortage_reports_its_estimate_without_promising_a_reservation() {
        let shortage = CapacityShortage {
            root: PathBuf::from("a"),
            required: 300,
            available: 250,
        };

        assert_eq!(
            shortage.to_string(),
            "Insufficient Archive staging capacity: estimated 300 bytes, available 250 bytes. \
             Estimates include staging overhead allowances but do not reserve space or \
             guarantee filesystem metadata, quotas, or concurrent writes."
        );
    }
}
