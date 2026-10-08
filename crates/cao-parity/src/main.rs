//! `cao-parity`: the differential parity harness for the Rust port (#472).
//!
//! Subcommands:
//!
//! - `run --case <case.json> --app-dir <dir> --facts <path>`: the Rust driver.
//!   It runs the case through the composition root with `<dir>` as the app
//!   directory and writes the raw `RunFacts` JSON. It exits 0 whenever it
//!   produced facts, whatever the Run Outcome.
//! - `case <id> [--work <dir>] [--oracle <exe>] [--profiles <dir>]
//!   [--hkxcmd <exe>] [--timeout <seconds>]`: runs the oracle and then the Rust
//!   driver on one case and compares them. `<id>` names a hand-written case,
//!   which is materialised afresh, or a case kept in the work directory, which
//!   is replayed from its `input/`. Exits 0 when both verdicts pass, 1 for a
//!   Different verdict, 2 for a harness error and 3 when the case cannot run
//!   here.
//! - `corpus` and `calibrate` need the corpus generator, which lands in a later
//!   slice, so they report that they are not available yet.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cao_parity::case::{CaseFile, CaseLayout, ProductionDrivers, Side, SideResources, run_case};
use cao_parity::cases::hand_written;
use cao_parity::driver::drive;
use cao_parity::tree::DefaultRules;

const USAGE: &str = "usage: cao-parity <run|corpus|case <id>|calibrate> [options]";

/// The per-case timeout when `--timeout` is not given.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments.first().map(String::as_str) {
        Some("run") => run(&arguments[1..]).map(|()| ExitCode::SUCCESS),
        Some("case") => case(&arguments[1..]),
        Some(command @ ("corpus" | "calibrate")) => Err(anyhow!(
            "`cao-parity {command}` is not implemented yet; it needs the corpus generator"
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

/// `cao-parity case <id>`: runs one case through both builds and compares them.
fn case(arguments: &[String]) -> Result<ExitCode> {
    let Some((id, rest)) = arguments.split_first() else {
        bail!("`cao-parity case` needs a case id\n{USAGE}");
    };
    let flags = Flags::parse(rest, &["work", "oracle", "profiles", "hkxcmd", "timeout"])?;
    let work = match flags.get("work") {
        Some(work) => PathBuf::from(work),
        None => target_dir()?.join("parity"),
    };
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
        .or_else(|| Some(workspace_root().join("bin/hkxcmd.exe")).filter(|path| path.is_file()));
    let timeout = match flags.get("timeout") {
        Some(seconds) => Duration::from_secs(
            seconds
                .parse()
                .with_context(|| format!("`--timeout {seconds}` is not a number of seconds"))?,
        ),
        None => DEFAULT_TIMEOUT,
    };

    let layout = CaseLayout::new(&work, id)?;
    prepare_case(&layout)?;
    let spec = layout.read_case()?.spec;
    if spec.animations && hkxcmd.is_none() {
        println!(
            "Case `{id}`: not run, because it requests Animations and no hkxcmd.exe was found"
        );
        return Ok(ExitCode::from(3));
    }
    layout.provision(&SideResources {
        profiles: &profiles,
        hkxcmd: hkxcmd.as_deref(),
    })?;

    let drivers = ProductionDrivers {
        oracle_exe,
        parity_exe: std::env::current_exe().context("locating cao-parity itself")?,
    };
    match run_case(&layout, &drivers, &DefaultRules, timeout) {
        Ok(result) => {
            println!("Case `{id}`");
            println!("  Run facts:   {}", result.facts.name());
            println!("  Output tree: {}", result.tree.name());
            if result.passed() {
                Ok(ExitCode::SUCCESS)
            } else {
                println!("  Report:      {}", layout.report().display());
                Ok(ExitCode::from(1))
            }
        }
        Err(error) => {
            println!("Case `{id}`: harness error: {error}");
            println!("  Report:      {}", layout.report().display());
            Ok(ExitCode::from(2))
        }
    }
}

/// Leaves the case directory holding only `case.json` and a pristine `input/`.
///
/// A hand-written case is written afresh. Any other id must be a case kept in
/// the work directory; its previous sides, captures and report are removed so
/// it replays from its own `input/`.
fn prepare_case(layout: &CaseLayout) -> Result<()> {
    if let Some(case) = hand_written(layout.id()) {
        remove_if_present(layout.root())?;
        layout.write_case(&CaseFile {
            spec: (case.spec)(),
        })?;
        (case.materialise)(&layout.input())?;
        return Ok(());
    }
    if !layout.case_file().is_file() || !layout.input().is_dir() {
        bail!(
            "`{}` is neither a hand-written case nor a case kept in {}",
            layout.id(),
            layout.root().display()
        );
    }
    let leftovers = [Side::Oracle, Side::Rust]
        .into_iter()
        .flat_map(|side| [layout.side(side), layout.stdout(side), layout.stderr(side)])
        .chain([layout.rust_facts(), layout.report()]);
    for path in leftovers {
        remove_if_present(&path)?;
    }
    Ok(())
}

/// Removes a file or a whole directory tree, if anything is at `path`.
fn remove_if_present(path: &Path) -> Result<()> {
    let removed = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else if path.exists() {
        std::fs::remove_file(path)
    } else {
        return Ok(());
    };
    removed.with_context(|| format!("removing {}", path.display()))
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
