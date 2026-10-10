//! `cao-parity`: the differential parity harness for the Rust port (#472).
//!
//! Subcommands:
//!
//! - `run --case <case.json> --app-dir <dir> --facts <path>`: the Rust driver.
//!   It runs the case through the composition root with `<dir>` as the app
//!   directory and writes the raw `RunFacts` JSON. It exits 0 whenever it
//!   produced facts, whatever the Run Outcome.
//! - `case <id> [options]`: runs the oracle and then the Rust driver on one
//!   case and compares them. `<id>` names a committed seed, a case this
//!   generator version produces, or a case kept in the work directory; either
//!   way the case is materialised afresh. Exits 0 when both verdicts pass, 1
//!   for a Different verdict, 2 for a harness error and 3 when the case cannot
//!   run here (it needs `hkxcmd.exe`, symlink rights or a local asset pool
//!   entry this host lacks). A case
//!   the deviation guard rejects is a harness error, before either build runs.
//! - `corpus [options]`: runs every seed and generated case, within the case
//!   budget, prints a summary and writes it to `<work>/corpus-report.md`.
//!   Exits 2 when any case hit a harness error, otherwise 1 when any was
//!   Different, otherwise 3 when some did not run here, and 0 only when every
//!   case passed. The summary lists every Different even when it exits 2.
//! - `calibrate` lands with the BC7/BC6H calibration (#513), so it reports
//!   that it is not available yet.
//!
//! - `local-assets [--local-assets <dir>]`: checks every entry of the pinned
//!   local asset list against the pool and prints each one's status, with the
//!   actual SHA-256 of any changed entry. Exits 0 when the pool supplies every
//!   entry, otherwise 3.
//!
//! `case` and `corpus` take the same options: `--work <dir>` (default
//! `target/parity`), `--oracle <exe>` (or `CAO_ORACLE`), `--profiles <dir>`
//! (default the repository's), `--hkxcmd <exe>` (or `CAO_HKXCMD`, or the
//! repository's `bin/hkxcmd.exe`), `--local-assets <dir>` (or
//! `CAO_LOCAL_ASSETS`, or the repository's `tests/local`) and
//! `--timeout <seconds>` per case. A case that uses a pool entry the pool
//! lacks, or whose bytes no longer match the pin, is not run.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use cao_parity::case::{CaseFile, CaseLayout, ProductionDrivers, SideResources};
use cao_parity::cases::fixtures_dir;
use cao_parity::corpus::{
    CaseOutcome, Harness, TIME_BUDGET, corpus_cases, resolve_case, run_corpus, run_one,
};
use cao_parity::driver::drive;
use cao_parity::local_assets::{LocalAssetPool, PinnedAsset, PinnedList, default_pool_dir};
use cao_parity::materialise::{Environment, can_create_symlinks};
use cao_parity::rules::ParityRules;

const USAGE: &str = "usage: cao-parity <run|corpus|case <id>|local-assets|calibrate> [options]";

/// The per-case timeout when `--timeout` is not given.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments.first().map(String::as_str) {
        Some("run") => run(&arguments[1..]).map(|()| ExitCode::SUCCESS),
        Some("case") => case(&arguments[1..]),
        Some("corpus") => corpus(&arguments[1..]),
        Some("local-assets") => local_assets(&arguments[1..]),
        Some("calibrate") => Err(anyhow!(
            "`cao-parity calibrate` is not implemented yet; it lands with the BC7/BC6H calibration"
        )),
        Some("--help" | "-h") => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(anyhow!("{USAGE}")),
    };
    outcome.unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::from(2)
    })
}

/// Named options after a subcommand's positional arguments.
struct Flags(Vec<(String, String)>);

impl Flags {
    /// Parses `--name value` pairs, rejecting anything not in `known`.
    fn parse(arguments: &[String], known: &[&str]) -> Result<Self> {
        let mut flags = Vec::new();
        let mut rest = arguments.iter();
        while let Some(name) = rest.next() {
            let Some(key) = name.strip_prefix("--").filter(|key| known.contains(key)) else {
                bail!("unexpected argument `{name}`\n{USAGE}");
            };
            let value = rest
                .next()
                .with_context(|| format!("`{name}` needs a value"))?;
            flags.push((key.to_owned(), value.clone()));
        }
        Ok(Self(flags))
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn require(&self, key: &str) -> Result<&str> {
        self.get(key)
            .with_context(|| format!("`--{key}` is required"))
    }
}

/// `cao-parity run`: drives one case through the composition root.
fn run(arguments: &[String]) -> Result<()> {
    install_logger();
    let flags = Flags::parse(arguments, &["case", "app-dir", "facts"])?;
    let case_path = Path::new(flags.require("case")?);
    let app_dir = Path::new(flags.require("app-dir")?);
    if !app_dir.is_absolute() {
        bail!("`--app-dir` must be absolute; nothing may resolve against the working directory");
    }
    let facts_path = Path::new(flags.require("facts")?);

    let bytes =
        std::fs::read(case_path).with_context(|| format!("reading {}", case_path.display()))?;
    let case: CaseFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {}", case_path.display()))?;
    let facts = drive(&case.spec, app_dir)?;
    let json = serde_json::to_vec_pretty(&facts)?;
    std::fs::write(facts_path, json)
        .with_context(|| format!("writing {}", facts_path.display()))?;
    Ok(())
}

/// The options `case` and `corpus` share, resolved.
struct Settings {
    /// Absolute, so the replay command works from any directory.
    work: PathBuf,
    oracle_exe: PathBuf,
    profiles: PathBuf,
    hkxcmd: Option<PathBuf>,
    local_assets: LocalAssetPool,
    timeout: Duration,
    parity_exe: PathBuf,
}

impl Settings {
    /// The options `case` and `corpus` take.
    const FLAGS: [&str; 6] = [
        "work",
        "oracle",
        "profiles",
        "hkxcmd",
        "local-assets",
        "timeout",
    ];

    /// Resolves the shared options, with their environment and default
    /// fallbacks, and creates the work directory.
    fn resolve(flags: &Flags) -> Result<Self> {
        let work = match flags.get("work") {
            Some(work) => PathBuf::from(work),
            None => target_dir()?.join("parity"),
        };
        std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
        let work =
            std::path::absolute(&work).with_context(|| format!("resolving {}", work.display()))?;
        let oracle_exe = flags
            .get("oracle")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("CAO_ORACLE").map(PathBuf::from))
            .context("the oracle exe is needed: pass `--oracle <exe>` or set CAO_ORACLE")?;
        let profiles = flags
            .get("profiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root().join("profiles"));
        let hkxcmd = flags
            .get("hkxcmd")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("CAO_HKXCMD").map(PathBuf::from))
            .or_else(|| {
                Some(workspace_root().join("bin/hkxcmd.exe")).filter(|path| path.is_file())
            });
        let timeout =
            match flags.get("timeout") {
                Some(seconds) => Duration::from_secs(seconds.parse().with_context(|| {
                    format!("`--timeout {seconds}` is not a number of seconds")
                })?),
                None => DEFAULT_TIMEOUT,
            };
        // Absolute, so the replay command names the same files from anywhere.
        let absolute = |path: PathBuf| {
            std::path::absolute(&path).with_context(|| format!("resolving {}", path.display()))
        };
        Ok(Self {
            work,
            oracle_exe: absolute(oracle_exe)?,
            profiles: absolute(profiles)?,
            hkxcmd: hkxcmd.map(absolute).transpose()?,
            local_assets: local_asset_pool(flags)?,
            timeout,
            parity_exe: std::env::current_exe().context("locating cao-parity itself")?,
        })
    }

    /// The exact command that replays case `id` with these settings: every
    /// option is spelled out, so neither the environment nor a default can
    /// change what the replay runs.
    fn replay_command(&self, id: &str) -> String {
        let quoted = |path: &Path| format!("\"{}\"", path.display());
        let mut command = format!(
            "{} case {id} --work {} --oracle {} --profiles {}",
            quoted(&self.parity_exe),
            quoted(&self.work),
            quoted(&self.oracle_exe),
            quoted(&self.profiles)
        );
        if let Some(hkxcmd) = &self.hkxcmd {
            command.push_str(&format!(" --hkxcmd {}", quoted(hkxcmd)));
        }
        command.push_str(&format!(
            " --local-assets {}",
            quoted(self.local_assets.root())
        ));
        command.push_str(&format!(" --timeout {}", self.timeout.as_secs()));
        command
    }

    /// Runs `body` with a harness over these settings.
    fn with_harness<T>(&self, body: impl FnOnce(&Harness<'_>) -> T) -> T {
        let drivers = ProductionDrivers {
            oracle_exe: self.oracle_exe.clone(),
            parity_exe: self.parity_exe.clone(),
        };
        let fixtures = fixtures_dir();
        let replay = |id: &str| self.replay_command(id);
        let harness = Harness {
            work: &self.work,
            environment: Environment {
                resources: SideResources {
                    profiles: &self.profiles,
                    hkxcmd: self.hkxcmd.as_deref(),
                },
                fixtures: &fixtures,
                symlink_rights: can_create_symlinks(&self.work),
                local_assets: &self.local_assets,
            },
            drivers: &drivers,
            rules: &ParityRules,
            timeout: self.timeout,
            time_budget: TIME_BUDGET,
            replay: &replay,
        };
        body(&harness)
    }
}

/// `cao-parity case <id>`: runs one case through both builds and compares them.
fn case(arguments: &[String]) -> Result<ExitCode> {
    let Some((id, rest)) = arguments.split_first() else {
        bail!("`cao-parity case` needs a case id\n{USAGE}");
    };
    let settings = Settings::resolve(&Flags::parse(rest, &Settings::FLAGS)?)?;
    let layout = CaseLayout::new(&settings.work, id)?;
    let kept: Option<CaseFile> = if layout.case_file().is_file() {
        Some(layout.read_case()?)
    } else {
        None
    };
    let resolved = resolve_case(id, kept)?;
    if let Some(warning) = &resolved.warning {
        println!("warning: {warning}");
    }

    let outcome = settings.with_harness(|harness| run_one(harness, id, &resolved.case));
    println!("Case `{id}`: {}", outcome.name());
    Ok(match &outcome {
        CaseOutcome::Passed(result) | CaseOutcome::Different(result) => {
            println!("  Run facts:   {}", result.facts.name());
            println!("  Output tree: {}", result.tree.name());
            if result.passed() {
                ExitCode::SUCCESS
            } else {
                println!("  Report:      {}", layout.report().display());
                ExitCode::from(1)
            }
        }
        CaseOutcome::HarnessError(message) => {
            println!("  {message}");
            println!("  Report:      {}", layout.report().display());
            ExitCode::from(2)
        }
        CaseOutcome::NotRun(reason) => {
            println!("  because {reason}");
            ExitCode::from(3)
        }
    })
}

/// `cao-parity corpus`: runs every seed and generated case and summarises them.
fn corpus(arguments: &[String]) -> Result<ExitCode> {
    let settings = Settings::resolve(&Flags::parse(arguments, &Settings::FLAGS)?)?;
    let budgeted = corpus_cases()?;
    let total = budgeted.cases.len();
    let mut index = 0;
    let mut started = Instant::now();
    let summary = settings.with_harness(|harness| {
        run_corpus(harness, &budgeted, &mut |id, outcome| {
            index += 1;
            println!(
                "[{index:>3}/{total}] {id}: {} ({:.1} s)",
                outcome.name(),
                started.elapsed().as_secs_f64()
            );
            started = Instant::now();
        })
    });

    let text = summary.render();
    println!("\n{text}");
    let report = settings.work.join("corpus-report.md");
    std::fs::write(&report, format!("```text\n{text}```\n"))
        .with_context(|| format!("writing {}", report.display()))?;
    println!("Summary: {}", report.display());
    Ok(ExitCode::from(summary.exit_code()))
}

/// The local asset pool: `--local-assets`, then `CAO_LOCAL_ASSETS`, then the
/// repository's gitignored `tests/local`, checked against the committed pinned
/// list. The folder need not exist; a missing pool only makes the cases that
/// use it not run.
///
/// # Errors
/// When the committed pinned list cannot be read or is invalid, which is a
/// harness error for every case, not a reason to skip some.
fn local_asset_pool(flags: &Flags) -> Result<LocalAssetPool> {
    let root = flags
        .get("local-assets")
        .map(PathBuf::from)
        // An empty variable counts as unset: it cannot name a folder, and
        // failing on it would fail every case, not just those using the pool.
        .or_else(|| {
            std::env::var_os("CAO_LOCAL_ASSETS")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(default_pool_dir);
    // Absolute, so the replay command names the same pool from anywhere.
    let root =
        std::path::absolute(&root).with_context(|| format!("resolving {}", root.display()))?;
    Ok(LocalAssetPool::new(root, PinnedList::committed()?))
}

/// `cao-parity local-assets`: checks every pinned entry against the pool,
/// printing `ok` or `missing` with the reason (a changed entry's reason holds
/// its actual SHA-256). Exits 0 when every entry is available, otherwise 3.
fn local_assets(arguments: &[String]) -> Result<ExitCode> {
    let pool = local_asset_pool(&Flags::parse(arguments, &["local-assets"])?)?;
    let assets: Vec<&PinnedAsset> = pool.pinned().assets.iter().collect();
    println!("Pool: {}", pool.root().display());
    let mut unavailable = 0;
    for (asset, result) in assets.iter().zip(pool.verify_each(&assets)) {
        match result {
            Ok(_) => println!("  ok       {}", asset.id),
            Err(reason) => {
                unavailable += 1;
                println!("  missing  {}: {reason}", asset.id);
            }
        }
    }
    println!(
        "{} of {} pinned entries are available.",
        assets.len() - unavailable,
        assets.len()
    );
    Ok(if unavailable == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(3)
    })
}

/// The Cargo target directory this exe was built into: `target/<profile>/cao-parity.exe`.
fn target_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating cao-parity itself")?;
    exe.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("cao-parity is not inside a Cargo target directory")
}

/// The repository root, which holds the shipped `profiles/`. The harness only
/// ever runs from a checkout, so the build-time path is the right one.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Sends the driver's log records to stderr, which the harness captures into
/// `rust.stderr.txt` for the case report.
fn install_logger() {
    struct StderrLog;

    impl log::Log for StderrLog {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            eprintln!("{} [{}] {}", record.level(), record.target(), record.args());
        }

        fn flush(&self) {
            // Nothing to flush: `eprintln!` writes unbuffered.
        }
    }

    static LOGGER: StderrLog = StderrLog;
    // Fails only if a logger is already installed, which leaves logging working.
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}
