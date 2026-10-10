//! The whole differential corpus (#472, #473, #500): seeds and generated cases
//! run one after another, each reported Identical, Equivalent or Different per
//! artifact, with not-run cases counted apart from passing ones.
//!
//! - **Budget.** At most [`MAX_CASES`] cases, seeds included; generated cases
//!   are trimmed before seeds. Each case's input holds at most
//!   [`MAX_CASE_FILES`] files and [`MAX_CASE_BYTES`] bytes; a bigger one is a
//!   harness error. A whole run should take about [`TIME_BUDGET`]: once it is
//!   spent, the generated cases still to come are trimmed, never the seeds.
//!   The summary warns when a run holds fewer than [`MIN_CASES`].
//! - **Outcomes.** A case passes, is Different, hits a harness error (the
//!   deviation guard's rejections included), or is not run because the host
//!   lacks a prerequisite. A case that did not run is never counted as passing.
//! - **Replay.** A Different case keeps its directory and a `report.md` whose
//!   replay command reruns it by id: [`resolve_case`] rebuilds a seed or a
//!   generated case from its id, so the same bytes are compared again.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::HarnessError;
use crate::case::{
    CaseDrivers, CaseFile, CaseLayout, CaseResult, ReportContext, run_case,
    write_harness_error_report,
};
use crate::cases::{seed, seeds};
use crate::compare::Verdict;
use crate::generate::{GENERATOR_VERSION, generated_case, generated_cases};
use crate::materialise::{Environment, Readiness, materialise};
use crate::tree::{ArtifactVerdict, TreeRules};

/// The most cases one corpus run holds, seeds included.
pub const MAX_CASES: usize = 250;

/// The fewest cases a full corpus is meant to hold, seeds included. A run
/// below it still runs; the summary says so, since coverage is thinner than
/// the spec plans.
pub const MIN_CASES: usize = 150;

/// The most files one case's input may hold.
pub const MAX_CASE_FILES: usize = 64;

/// The most bytes one case's input may hold: 32 MiB.
pub const MAX_CASE_BYTES: u64 = 32 * 1024 * 1024;

/// About how long a whole corpus run should take on the maintainer's machine.
pub const TIME_BUDGET: Duration = Duration::from_secs(30 * 60);

/// Where a corpus case comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A committed seed under `crates/cao-parity/seeds/`.
    Seed,
    /// A case the generator builds afresh on every run.
    Generated,
}

/// One case of a corpus run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusCase {
    pub id: String,
    pub case: CaseFile,
    pub origin: Origin,
}

/// The cases a run keeps, and how many of each origin the budget trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budgeted {
    /// The seeds, then the generated cases, each in their given order.
    pub cases: Vec<CorpusCase>,
    pub trimmed_generated: usize,
    pub trimmed_seeds: usize,
}

/// Keeps at most `max` cases: every seed that fits, then as many generated
/// cases as there is room left. Generated cases go first, from the end, and
/// seeds only once no generated case is left.
pub fn within_budget(
    mut seeds: Vec<CorpusCase>,
    mut generated: Vec<CorpusCase>,
    max: usize,
) -> Budgeted {
    let room = max.saturating_sub(seeds.len());
    let trimmed_generated = generated.len().saturating_sub(room);
    generated.truncate(room);
    let trimmed_seeds = seeds.len().saturating_sub(max);
    seeds.truncate(max);
    seeds.extend(generated);
    Budgeted {
        cases: seeds,
        trimmed_generated,
        trimmed_seeds,
    }
}

/// Every case of a corpus run, within [`MAX_CASES`]: the committed seeds, then
/// this generator version's cases.
///
/// # Errors
/// Those of [`seeds`], and [`HarnessError::InvalidCase`] when a generated id
/// is also a seed's, since the two would share one case directory.
pub fn corpus_cases() -> Result<Budgeted, HarnessError> {
    let seeds: Vec<CorpusCase> = seeds()?
        .into_iter()
        .map(|(id, case)| CorpusCase {
            id,
            case,
            origin: Origin::Seed,
        })
        .collect();
    let generated: Vec<CorpusCase> = generated_cases()
        .into_iter()
        .map(|(id, case)| CorpusCase {
            id,
            case,
            origin: Origin::Generated,
        })
        .collect();
    if let Some(clash) = generated.iter().find(|case| {
        seeds
            .iter()
            .any(|seed| seed.id.eq_ignore_ascii_case(&case.id))
    }) {
        return Err(HarnessError::InvalidCase(format!(
            "the generated case `{}` has a seed's id",
            clash.id
        )));
    }
    Ok(within_budget(seeds, generated, MAX_CASES))
}

/// The case a replay of `id` runs, and a warning when its bytes may differ
/// from the run that kept it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub case: CaseFile,
    pub warning: Option<String>,
}

/// Finds the case `cao-parity case <id>` replays: a seed, then a case this
/// generator version produces, then `kept`, the `case.json` a corpus run left
/// in the work directory.
///
/// A generated case is always rebuilt from its id, never read back, so a
/// replay compares the bytes the generator builds today. When `kept` records
/// another [`GENERATOR_VERSION`], the warning says the bytes may differ from
/// the run that kept it.
///
/// # Errors
/// Those of [`seed`], and [`HarnessError::InvalidCase`] when `id` is none of
/// the three.
pub fn resolve_case(id: &str, kept: Option<CaseFile>) -> Result<Resolved, HarnessError> {
    if let Some(case) = seed(id)? {
        return Ok(Resolved {
            case,
            warning: None,
        });
    }
    let warning = kept
        .as_ref()
        .and_then(|kept| kept.generator_version)
        .filter(|&version| version != GENERATOR_VERSION)
        .map(|version| {
            format!(
                "`{id}` was kept by generator version {version}, and this is version \
                 {GENERATOR_VERSION}, so its bytes may differ"
            )
        });
    let case = generated_case(id).or(kept).ok_or_else(|| {
        HarnessError::InvalidCase(format!(
            "`{id}` is neither a seed, a generated case, nor a case kept in the work directory"
        ))
    })?;
    Ok(Resolved { case, warning })
}

/// What a run needs beyond its cases.
pub struct Harness<'a> {
    /// The work directory every case directory sits in.
    pub work: &'a Path,
    pub environment: Environment<'a>,
    pub drivers: &'a dyn CaseDrivers,
    pub rules: &'a dyn TreeRules,
    /// Each case's timeout, covering both sides.
    pub timeout: Duration,
    /// How long a whole run may take before it stops starting generated cases;
    /// normally [`TIME_BUDGET`].
    pub time_budget: Duration,
    /// Renders the exact command that replays a case, from its id.
    pub replay: &'a dyn Fn(&str) -> String,
}

/// How one case ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseOutcome {
    /// Both verdicts are Identical or Equivalent; the directory is deleted.
    Passed(CaseResult),
    /// At least one verdict is Different; the directory is kept with a report.
    Different(CaseResult),
    /// The harness could not produce a verdict, with why.
    HarnessError(String),
    /// The host lacks a prerequisite, with which one. Never a pass.
    NotRun(String),
}

impl CaseOutcome {
    /// The outcome's name, as progress lines print it.
    pub fn name(&self) -> &'static str {
        match self {
            CaseOutcome::Passed(_) => "passed",
            CaseOutcome::Different(_) => "Different",
            CaseOutcome::HarnessError(_) => "harness error",
            CaseOutcome::NotRun(_) => "not run",
        }
    }
}

/// Runs one case from a fresh case directory: writes its `case.json`,
/// materialises it, checks the per-case budget, then runs both sides and
/// compares them.
///
/// Whatever was in the case directory before is removed first, and the case is
/// rebuilt from its recipe. The recipe is seeded by the case id, so the rebuild
/// has the same bytes, and a shaped `input/` (links, read-only files) never has
/// to be copied. A not-run case keeps only its `case.json`; a harness error
/// keeps the directory with a report, as a Different case does.
pub fn run_one(harness: &Harness<'_>, id: &str, case: &CaseFile) -> CaseOutcome {
    let layout = match CaseLayout::new(harness.work, id) {
        Ok(layout) => layout,
        Err(error) => return CaseOutcome::HarnessError(error.to_string()),
    };
    // Every report records the running generator, seeds' too: the replay
    // rebuilds the case with whatever generator is running then.
    let report = ReportContext {
        replay: (harness.replay)(id),
        generator_version: Some(GENERATOR_VERSION),
    };
    match try_run(harness, &layout, case, &report) {
        Ok(outcome) => outcome,
        Err(error) => CaseOutcome::HarnessError(error.to_string()),
    }
}

/// [`run_one`]'s body, with harness errors as `Err`.
fn try_run(
    harness: &Harness<'_>,
    layout: &CaseLayout,
    case: &CaseFile,
    report: &ReportContext,
) -> Result<CaseOutcome, HarnessError> {
    remove_if_present(layout.root())?;
    layout.write_case(case)?;
    let prepared = materialise(layout, case, &harness.environment).and_then(|readiness| {
        if readiness == Readiness::Ready {
            check_input_budget(&layout.input())?;
        }
        Ok(readiness)
    });
    match prepared {
        Ok(Readiness::Ready) => {}
        Ok(Readiness::NotRun(reason)) => return Ok(CaseOutcome::NotRun(reason)),
        Err(error) => {
            write_harness_error_report(layout, report, &error);
            return Err(error);
        }
    }
    let result = run_case(
        layout,
        harness.drivers,
        harness.rules,
        harness.timeout,
        report,
    )?;
    Ok(if result.passed() {
        CaseOutcome::Passed(result)
    } else {
        CaseOutcome::Different(result)
    })
}

/// Fails when the materialised input is over the per-case budget.
fn check_input_budget(input: &Path) -> Result<(), HarnessError> {
    let (files, bytes) = tree_size(input)?;
    if files > MAX_CASE_FILES || bytes > MAX_CASE_BYTES {
        return Err(HarnessError::InvalidCase(format!(
            "the input holds {files} files and {bytes} bytes, over the per-case budget of \
             {MAX_CASE_FILES} files and {MAX_CASE_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Counts the files and bytes beneath `directory`. Links are not followed,
/// and count as one file of no size.
fn tree_size(directory: &Path) -> Result<(usize, u64), HarnessError> {
    let listing = |error| HarnessError::io(format!("listing {}", directory.display()), error);
    let mut totals = (0, 0);
    for item in std::fs::read_dir(directory).map_err(listing)? {
        let item = item.map_err(listing)?;
        let metadata = std::fs::symlink_metadata(item.path()).map_err(|error| {
            HarnessError::io(format!("reading {}", item.path().display()), error)
        })?;
        if metadata.is_dir() {
            let (files, bytes) = tree_size(&item.path())?;
            totals = (totals.0 + files, totals.1 + bytes);
        } else if metadata.is_symlink() {
            totals.0 += 1;
        } else {
            totals = (totals.0 + 1, totals.1 + metadata.len());
        }
    }
    Ok(totals)
}

/// Removes a file or a whole directory tree, if anything is at `path`.
fn remove_if_present(path: &Path) -> Result<(), HarnessError> {
    let removed = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else if path.exists() {
        std::fs::remove_file(path)
    } else {
        return Ok(());
    };
    removed.map_err(|error| HarnessError::io(format!("removing {}", path.display()), error))
}

/// Runs every budgeted case in order, reporting each outcome to `on_case` as
/// it ends. The summary carries the budget's trim counts.
///
/// Once the run has taken [`Harness::time_budget`], generated cases still to
/// come are trimmed rather than started, and counted with the ones the case
/// budget trimmed. Seeds are never trimmed for time; they come first anyway.
pub fn run_corpus(
    harness: &Harness<'_>,
    budgeted: &Budgeted,
    on_case: &mut dyn FnMut(&str, &CaseOutcome),
) -> CorpusSummary {
    let started = Instant::now();
    let mut records = Vec::with_capacity(budgeted.cases.len());
    let mut trimmed_for_time = 0;
    for case in &budgeted.cases {
        if case.origin == Origin::Generated && started.elapsed() >= harness.time_budget {
            trimmed_for_time += 1;
            continue;
        }
        let outcome = run_one(harness, &case.id, &case.case);
        on_case(&case.id, &outcome);
        // An id `CaseLayout` rejects is a harness error with no report at all.
        let report = CaseLayout::new(harness.work, &case.id)
            .map(|layout| layout.report())
            .ok();
        records.push(CaseRecord {
            id: case.id.clone(),
            origin: case.origin,
            report,
            outcome,
        });
    }
    CorpusSummary {
        records,
        trimmed_generated: budgeted.trimmed_generated,
        trimmed_for_time,
        trimmed_seeds: budgeted.trimmed_seeds,
        elapsed: started.elapsed(),
        time_budget: harness.time_budget,
    }
}

/// One case's place in the summary.
#[derive(Debug, Clone)]
struct CaseRecord {
    id: String,
    origin: Origin,
    /// Where the case's report is, when it has one; `None` when its id is
    /// not even a usable folder name.
    report: Option<PathBuf>,
    outcome: CaseOutcome,
}

/// What an artifact is, for the per-kind verdict counts: an Asset Kind, a
/// Loading Plugin, a leftover of a run, or any other file or directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Texture,
    Mesh,
    Animation,
    Archive,
    LoadingPlugin,
    /// `.caobad` and `.bak` files and staging residue.
    Leftover,
    Other,
}

impl ArtifactKind {
    const ALL: [ArtifactKind; 7] = [
        ArtifactKind::Texture,
        ArtifactKind::Mesh,
        ArtifactKind::Animation,
        ArtifactKind::Archive,
        ArtifactKind::LoadingPlugin,
        ArtifactKind::Leftover,
        ArtifactKind::Other,
    ];

    /// The kind's row label in the summary.
    pub fn label(self) -> &'static str {
        match self {
            ArtifactKind::Texture => "Texture",
            ArtifactKind::Mesh => "Mesh",
            ArtifactKind::Animation => "Animation",
            ArtifactKind::Archive => "Archive",
            ArtifactKind::LoadingPlugin => "Loading Plugin",
            ArtifactKind::Leftover => "Leftover",
            ArtifactKind::Other => "Other",
        }
    }
}

/// The kind of the artifact at a normalised relative path, by its extension,
/// ignoring ASCII case as Asset Routing does. Leftovers are recognised first,
/// since `a.dds.caobad` is a renamed Texture, not a Texture.
pub fn artifact_kind(path: &str) -> ArtifactKind {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let in_staging = lower
        .split('/')
        .any(|component| component.starts_with(".cao-staging"));
    if in_staging || name.ends_with(".caobad") || name.ends_with(".bak") {
        return ArtifactKind::Leftover;
    }
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("dds" | "tga") => ArtifactKind::Texture,
        Some("nif" | "btr" | "bto") => ArtifactKind::Mesh,
        Some("hkx") => ArtifactKind::Animation,
        Some("bsa" | "ba2") => ArtifactKind::Archive,
        Some("esp" | "esm" | "esl") => ArtifactKind::LoadingPlugin,
        _ => ArtifactKind::Other,
    }
}

/// Identical, Equivalent and Different counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts([usize; 3]);

impl Counts {
    fn add<D>(&mut self, verdict: &Verdict<D>) {
        self.0[match verdict {
            Verdict::Identical => 0,
            Verdict::Equivalent => 1,
            Verdict::Different(_) => 2,
        }] += 1;
    }

    fn add_artifact(&mut self, verdict: &ArtifactVerdict) {
        self.0[match verdict {
            ArtifactVerdict::Identical => 0,
            ArtifactVerdict::Equivalent { .. } => 1,
            ArtifactVerdict::Different(_) => 2,
        }] += 1;
    }
}

/// A whole run's outcomes.
#[derive(Debug, Clone)]
pub struct CorpusSummary {
    records: Vec<CaseRecord>,
    /// Generated cases the case budget trimmed before the run.
    pub trimmed_generated: usize,
    /// Generated cases not started because the time budget was spent.
    pub trimmed_for_time: usize,
    /// Seeds the budget trimmed before the run.
    pub trimmed_seeds: usize,
    /// How long the run took.
    pub elapsed: Duration,
    /// The run's time budget, which `elapsed` is reported against.
    time_budget: Duration,
}

impl CorpusSummary {
    fn count(&self, test: fn(&CaseOutcome) -> bool) -> usize {
        self.records
            .iter()
            .filter(|record| test(&record.outcome))
            .count()
    }

    pub fn passed(&self) -> usize {
        self.count(|outcome| matches!(outcome, CaseOutcome::Passed(_)))
    }

    pub fn different(&self) -> usize {
        self.count(|outcome| matches!(outcome, CaseOutcome::Different(_)))
    }

    pub fn harness_errors(&self) -> usize {
        self.count(|outcome| matches!(outcome, CaseOutcome::HarnessError(_)))
    }

    pub fn not_run(&self) -> usize {
        self.count(|outcome| matches!(outcome, CaseOutcome::NotRun(_)))
    }

    /// The `corpus` command's exit code: 2 when any case hit a harness error,
    /// else 1 when any was Different, else 3 when any did not run, else 0.
    /// Not-run cases fail nothing, but a run with some is never reported as a
    /// clean pass either.
    pub fn exit_code(&self) -> u8 {
        if self.harness_errors() > 0 {
            2
        } else if self.different() > 0 {
            1
        } else if self.not_run() > 0 {
            3
        } else {
            0
        }
    }

    /// The summary as plain text: the counts by outcome, the verdict counts
    /// for run facts and per artifact kind, and every case that did not pass.
    pub fn render(&self) -> String {
        let seeds = self
            .records
            .iter()
            .filter(|record| record.origin == Origin::Seed)
            .count();
        let mut text = format!(
            "cao-parity corpus, generator version {GENERATOR_VERSION}\n\n\
             Cases: {} ({seeds} seeds, {} generated)\n",
            self.records.len(),
            self.records.len() - seeds
        );
        if self.trimmed_generated + self.trimmed_seeds > 0 {
            text.push_str(&format!(
                "Trimmed to the {MAX_CASES}-case budget: {} generated, {} seeds\n",
                self.trimmed_generated, self.trimmed_seeds
            ));
        }
        if self.trimmed_for_time > 0 {
            text.push_str(&format!(
                "Trimmed for time: {} generated cases were not started once the time \
                 budget was spent\n",
                self.trimmed_for_time
            ));
        }
        if self.records.len() < MIN_CASES {
            text.push_str(&format!(
                "Below the {MIN_CASES}-case floor the corpus is meant to reach\n"
            ));
        }
        text.push_str(&format!(
            "Passed: {}\nDifferent: {}\nHarness errors: {}\nNot run: {}\n",
            self.passed(),
            self.different(),
            self.harness_errors(),
            self.not_run()
        ));

        let mut facts = Counts::default();
        let mut kinds = [Counts::default(); ArtifactKind::ALL.len()];
        for record in &self.records {
            if let CaseOutcome::Passed(result) | CaseOutcome::Different(result) = &record.outcome {
                facts.add(&result.facts);
                for artifact in &result.artifacts {
                    let kind = artifact_kind(&artifact.path);
                    let index = ArtifactKind::ALL
                        .iter()
                        .position(|known| *known == kind)
                        .expect("ALL lists every kind");
                    kinds[index].add_artifact(&artifact.verdict);
                }
            }
        }
        let row = |label: &str, counts: Counts| {
            let [identical, equivalent, different] = counts.0;
            format!("  {label:<16}{identical:>10}{equivalent:>12}{different:>11}\n")
        };
        text.push_str(&format!(
            "\n  {:<16}{:>10}{:>12}{:>11}\n",
            "Verdicts", "Identical", "Equivalent", "Different"
        ));
        text.push_str(&row("Run facts", facts));
        for (kind, counts) in ArtifactKind::ALL.iter().zip(kinds) {
            text.push_str(&row(kind.label(), counts));
        }

        let report = |record: &CaseRecord| {
            record
                .report
                .as_ref()
                .map_or_else(|| "no report".to_owned(), |path| path.display().to_string())
        };
        let mut section = |title: &str, lines: Vec<String>| {
            if !lines.is_empty() {
                text.push_str(&format!("\n{title}:\n"));
                for line in lines {
                    text.push_str(&format!("  - {line}\n"));
                }
            }
        };
        section(
            "Different",
            self.records
                .iter()
                .filter(|record| matches!(record.outcome, CaseOutcome::Different(_)))
                .map(|record| format!("{}: {}", record.id, report(record)))
                .collect(),
        );
        section(
            "Harness errors",
            self.records
                .iter()
                .filter_map(|record| match &record.outcome {
                    CaseOutcome::HarnessError(message) => {
                        Some(format!("{}: {message} ({})", record.id, report(record)))
                    }
                    _ => None,
                })
                .collect(),
        );
        section(
            "Not run",
            self.records
                .iter()
                .filter_map(|record| match &record.outcome {
                    CaseOutcome::NotRun(reason) => Some(format!("{}: {reason}", record.id)),
                    _ => None,
                })
                .collect(),
        );

        let minutes = self.elapsed.as_secs() / 60;
        let seconds = self.elapsed.as_secs() % 60;
        let budget = self.time_budget.as_secs() / 60;
        let verdict = if self.elapsed <= self.time_budget {
            "within"
        } else {
            "OVER"
        };
        text.push_str(&format!(
            "\nElapsed: {minutes}m {seconds:02}s, {verdict} the {budget}-minute budget\n"
        ));
        text
    }
}
