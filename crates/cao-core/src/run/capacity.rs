//! The Capacity Check: an Optimization Run's estimated Archive work compared
//! against the free space on each affected volume.
//!
//! Ported from `src/Run/ArchiveCapacity.h`. Callers own their estimates and
//! their failure codes; this module owns everything between them: per-volume
//! accounting, sampling the [`VolumeProbe`] with its panic rule, the per-run
//! freeze of volume identities, and the shortfall message. A caller builds a
//! [`Demand`] of each Mod Root's estimated bytes and asks the
//! [`CapacityCheck`] for a [`Shortfall`] or nothing.

use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::run::{RunDiagnostic, RunDiagnosticCode, RunPhase, VolumeProbe, take_panic_message};

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
pub(crate) struct CapacityCheck<'p> {
    probe: &'p dyn VolumeProbe,
    volumes: FrozenVolumes,
}

impl<'p> CapacityCheck<'p> {
    /// A check over `probe` with volume identities of its own, frozen for as
    /// long as it lives.
    ///
    /// Test-only: every production check shares the run's frozen volumes
    /// through [`Self::sharing`], so a check of its own exists only for this
    /// module's unit tests (#598).
    #[cfg(test)]
    pub(crate) fn new(probe: &'p dyn VolumeProbe) -> Self {
        Self::sharing(probe, &FrozenVolumes::default())
    }

    /// A check over `probe` sharing `volumes` with every other check over
    /// them, so each Mod Root's volume is looked up at most once among all of
    /// them. Every check sharing `volumes` must be over the same probe.
    pub(crate) fn sharing(probe: &'p dyn VolumeProbe, volumes: &FrozenVolumes) -> Self {
        Self {
            probe,
            volumes: volumes.clone(),
        }
    }

    /// An empty demand, to which the caller adds each Mod Root's estimate.
    pub(crate) fn demand(&self) -> Demand {
        Demand::default()
    }

    /// Tests `root` against the whole of `demand` on its volume: the
    /// shortfall when that is known not to fit, `None` when it fits or free
    /// space is unknown.
    pub(crate) fn shortfall_at(&self, demand: &Demand, root: &Path) -> Option<Shortfall> {
        let required = self.required_at(demand, root);
        let available = self.available_bytes(root)?;
        (available < required).then(|| Shortfall {
            root: root.to_path_buf(),
            required,
            available,
        })
    }

    /// The first shortfall, in the order roots were added to `demand`, among
    /// the roots whose own demand is non-zero.
    ///
    /// A root needing no bytes cannot run out of space, so it is never
    /// checked, and its free space is never sampled.
    pub(crate) fn first_shortfall(&self, demand: &Demand) -> Option<Shortfall> {
        demand
            .roots
            .iter()
            .filter(|(_, bytes)| *bytes != 0)
            .find_map(|(root, _)| self.shortfall_at(demand, root))
    }

    /// The warning-level [`RunDiagnostic`] a Dry Run records, at `phase`,
    /// for a `shortfall` its Apply run would fail on: a
    /// [`RunDiagnosticCode::CapacityShortage`] whose path is the shortfall's
    /// Mod Root and whose detail names that root's volume, the demand and the
    /// available space.
    ///
    /// The volume is the root's frozen identity, looked up through the probe
    /// only if no check of the run has yet; an unknown or panicking answer
    /// leaves the detail naming "the volume holding this Mod Root" instead.
    /// Only a Dry Run calls this, so an Apply run never looks a volume up
    /// just to describe its failure.
    pub(crate) fn shortage_warning(&self, phase: RunPhase, shortfall: &Shortfall) -> RunDiagnostic {
        let volume = self.volume_identity(&shortfall.root);
        shortfall.warning(phase, volume.as_deref())
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
    /// is, and as a throwing one was in C++. The check is advisory: unknown
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
/// [`Display`](fmt::Display) form is the failure detail each caller reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shortfall {
    /// The Mod Root that ran short.
    pub root: PathBuf,
    /// The estimated bytes the root was checked against.
    pub required: u64,
    /// The free space sampled at the root.
    pub available: u64,
}

impl Shortfall {
    /// The warning-level [`RunDiagnosticCode::CapacityShortage`] at `phase`
    /// for this shortfall, attributed to its Mod Root, naming `volume`, the
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
    }
}

impl fmt::Display for Shortfall {
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

    fn shortfall(root: &str, required: u64, available: u64) -> Option<Shortfall> {
        Some(Shortfall {
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
            check.shortfall_at(&demand, Path::new("a")),
            shortfall("a", 300, 250)
        );
        assert_eq!(check.shortfall_at(&demand, Path::new("d")), None);
        assert_eq!(check.first_shortfall(&demand), shortfall("a", 300, 250));
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
            check.shortfall_at(&demand, Path::new("a")),
            shortfall("a", 300, 250)
        );
        assert_eq!(
            check.shortfall_at(&demand, Path::new("b")),
            shortfall("b", 300, 250)
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

        assert_eq!(check.first_shortfall(&demand), None);
        assert_eq!(check.shortfall_at(&demand, Path::new("a")), None);
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

        assert_eq!(check.shortfall_at(&demand, Path::new("a")), None);
        assert_eq!(check.first_shortfall(&demand), shortfall("b", 300, 250));
        // The panic poisoned nothing: the frozen volumes still answer.
        assert_eq!(
            check.shortfall_at(&demand, Path::new("b")),
            shortfall("b", 300, 250)
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
            let check = CapacityCheck::sharing(&probe, &volumes);
            let demand = demand_of(&check, &[("a", 100), ("b", 200), ("a", 100)]);
            assert_eq!(check.first_shortfall(&demand), None);
            assert_eq!(check.shortfall_at(&demand, Path::new("b")), None);
        }

        assert_eq!(probe.lookups("a"), 1);
        assert_eq!(probe.lookups("b"), 1);
        // A check with volumes of its own looks each root up again.
        let fresh = CapacityCheck::new(&probe);
        let demand = demand_of(&fresh, &[("a", 100), ("b", 200)]);
        assert_eq!(fresh.first_shortfall(&demand), None);
        assert_eq!(probe.lookups("a"), 2);
    }

    /// #642: a Dry Run's capacity warning is attributed to the Mod Root and
    /// names its volume, or the volume holding it when the identity is
    /// unknown, with the demand and the available space.
    #[test]
    fn a_shortage_warning_names_the_volume_the_demand_and_the_space() {
        let probe = TableProbe::default().with_volume("known", "volume-1");
        let check = CapacityCheck::new(&probe);
        for (root, volume) in [
            ("known", "volume volume-1"),
            ("unknown", "the volume holding this Mod Root"),
        ] {
            let warning = check.shortage_warning(
                RunPhase::DiscoveringArchives,
                &Shortfall {
                    root: PathBuf::from(root),
                    required: 300,
                    available: 250,
                },
            );

            assert_eq!(warning.code, RunDiagnosticCode::CapacityShortage);
            assert_eq!(warning.phase, RunPhase::DiscoveringArchives);
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

    /// Rust-only, for #593: the shortfall explains itself word for word as
    /// the C++ capacity detail did.
    #[test]
    fn a_shortfall_reports_its_estimate_without_promising_a_reservation() {
        let shortfall = Shortfall {
            root: PathBuf::from("a"),
            required: 300,
            available: 250,
        };

        assert_eq!(
            shortfall.to_string(),
            "Insufficient Archive staging capacity: estimated 300 bytes, available 250 bytes. \
             Estimates include staging overhead allowances but do not reserve space or \
             guarantee filesystem metadata, quotas, or concurrent writes."
        );
    }
}
