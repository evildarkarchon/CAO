//! The per-case layout, the `CaseSpec`, and the sequential case runner (#472).
//!
//! Each case lives in its own directory under the work directory:
//!
//! ```text
//! <work>/<case-id>/
//!   case.json            the CaseFile, holding the CaseSpec
//!   input/               the pristine case tree
//!   oracle/              the oracle's working directory
//!     profiles/  bin/hkxcmd.exe  logs/  <case tree>
//!   rust/                the Rust driver's app directory
//!     profiles/  bin/hkxcmd.exe  logs/  <case tree>
//!   oracle.stdout.txt  oracle.stderr.txt  rust.stdout.txt  rust.stderr.txt
//!   rust.facts.json      the Rust driver's raw RunFacts
//!   report.md            only for a Different verdict or a harness error
//! ```
//!
//! Both sides sit under one directory, so they share one volume, and both
//! copies of the case tree keep the input's Mod Root names, because Archive and
//! plugin names derive from them. Each side has private profiles, `hkxcmd.exe`
//! and logs. Cases run one at a time, oracle first, each side under the
//! per-case timeout. A passing case is deleted; any other is kept for diagnosis.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::HarnessError;
use crate::compare::{FactDifference, Verdict, compare_facts};
use crate::facts::RunFacts;
use crate::normalise::normalise;
use crate::oracle;
use crate::tree::{ArtifactDifference, TreeRules, TreeSide, compare_trees};

/// The top-level folders of a side that the harness provisions. They are not
/// part of the case tree and never part of its output.
pub const HARNESS_OWNED: [&str; 3] = ["profiles", "bin", "logs"];

/// The contents of `case.json`.
///
/// The corpus generator adds the GUI-reachable profile overrides and the tree
/// recipe beside the spec; until then the spec is the whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseFile {
    pub spec: CaseSpec,
}

/// What a case asks both builds to do: the options a user sets in the GUI.
///
/// The harness renders it into oracle argv ([`oracle_arguments`]), and
/// `cao-parity run` fills the options model from it as the GUI fills it from
/// its widgets. Every field is required, so a spec never relies on a default
/// that one build might apply differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseSpec {
    /// A profile folder name under `profiles/`, such as `SSE`.
    pub profile: String,
    pub mode: SelectionMode,
    /// The selected folder, relative to the side's case root and
    /// `/`-separated: the Mod Root for one mod, or its parent for several.
    pub selection: String,
    pub dry_run: bool,
    pub textures: TextureOptions,
    pub meshes: MeshOptions,
    pub animations: bool,
    pub archives: ArchiveOptions,
}

/// One mod (`om`) or several mods (`sm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    OneMod,
    SeveralMods,
}

/// The Textures tab. The ratio and size values exist whether or not their
/// resize mode is enabled, as the GUI's spin boxes do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureOptions {
    pub necessary: bool,
    pub compress: bool,
    pub mipmaps: bool,
    pub resize_by_ratio: bool,
    pub ratio_width: u32,
    pub ratio_height: u32,
    pub resize_by_size: bool,
    pub target_width: u32,
    pub target_height: u32,
}

/// The Meshes tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshOptions {
    /// 0 (off) to 3 (full).
    pub level: u8,
    pub headparts: bool,
    pub resave: bool,
}

/// The Archives tab, including the five options the GUI keeps in `settings.ini`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveOptions {
    pub extract: bool,
    pub create: bool,
    pub delete_backup: bool,
    pub compress: bool,
    pub create_dummies: bool,
    pub merge_incompressible: bool,
    pub merge_textures: bool,
    pub delete_sources: bool,
}

/// One of a case's two builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Oracle,
    Rust,
}

impl Side {
    /// The side's folder and capture-file stem.
    pub fn name(self) -> &'static str {
        match self {
            Side::Oracle => "oracle",
            Side::Rust => "rust",
        }
    }
}

/// What each side gets a private copy of.
pub struct SideResources<'a> {
    /// The `profiles/` folder to copy.
    pub profiles: &'a Path,
    /// The `hkxcmd.exe` to copy into `bin/`, when one was found. Without it,
    /// cases that request Animations must be reported as not run.
    pub hkxcmd: Option<&'a Path>,
}

/// The paths of one case directory.
#[derive(Debug, Clone)]
pub struct CaseLayout {
    id: String,
    root: PathBuf,
}

impl CaseLayout {
    /// The layout of case `id` under `work`. The id becomes one folder name, so
    /// it is limited to ASCII letters, digits, `-`, `_` and `.`.
    pub fn new(work: &Path, id: &str) -> Result<Self, HarnessError> {
        let valid = !id.is_empty()
            && id != "."
            && id != ".."
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        if !valid {
            return Err(HarnessError::InvalidCase(format!(
                "`{id}` is not a usable case id"
            )));
        }
        Ok(Self {
            id: id.to_owned(),
            root: work.join(id),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn case_file(&self) -> PathBuf {
        self.root.join("case.json")
    }

    pub fn input(&self) -> PathBuf {
        self.root.join("input")
    }

    /// The side's case root: the oracle's working directory, or the Rust
    /// driver's app directory.
    pub fn side(&self, side: Side) -> PathBuf {
        self.root.join(side.name())
    }

    pub fn report(&self) -> PathBuf {
        self.root.join("report.md")
    }

    pub fn stdout(&self, side: Side) -> PathBuf {
        self.root.join(format!("{}.stdout.txt", side.name()))
    }

    pub fn stderr(&self, side: Side) -> PathBuf {
        self.root.join(format!("{}.stderr.txt", side.name()))
    }

    /// Where `cao-parity run` writes its raw `RunFacts`.
    pub fn rust_facts(&self) -> PathBuf {
        self.root.join("rust.facts.json")
    }

    /// Writes `case.json`, creating the case directory.
    pub fn write_case(&self, case: &CaseFile) -> Result<(), HarnessError> {
        std::fs::create_dir_all(&self.root).map_err(|error| {
            HarnessError::io(format!("creating {}", self.root.display()), error)
        })?;
        let json = serde_json::to_vec_pretty(case).map_err(|source| HarnessError::Json {
            path: self.case_file(),
            source,
        })?;
        std::fs::write(self.case_file(), json).map_err(|error| {
            HarnessError::io(format!("writing {}", self.case_file().display()), error)
        })
    }

    /// Reads `case.json`.
    pub fn read_case(&self) -> Result<CaseFile, HarnessError> {
        let path = self.case_file();
        let bytes = std::fs::read(&path)
            .map_err(|error| HarnessError::io(format!("reading {}", path.display()), error))?;
        serde_json::from_slice(&bytes).map_err(|source| HarnessError::Json { path, source })
    }

    /// Creates both sides from the materialised `input/` tree.
    ///
    /// Each side gets a byte-for-byte copy of the input, under the same names,
    /// plus private `profiles/`, `logs/` and, when given, `bin/hkxcmd.exe`.
    /// The input may not use a harness-owned top-level name, since the side's
    /// own folder would shadow it. Filesystem-shape operations (links,
    /// read-only files) are applied to each side afterwards by the generator,
    /// so the input must contain only plain files and directories.
    pub fn provision(&self, resources: &SideResources<'_>) -> Result<(), HarnessError> {
        let input = self.input();
        let listing = std::fs::read_dir(&input)
            .map_err(|error| HarnessError::io(format!("listing {}", input.display()), error))?;
        for item in listing {
            let item = item
                .map_err(|error| HarnessError::io(format!("listing {}", input.display()), error))?;
            let name = item.file_name();
            // Windows names are case-insensitive, so `Logs` would land in `logs/`.
            if HARNESS_OWNED
                .iter()
                .any(|owned| name.eq_ignore_ascii_case(owned))
            {
                return Err(HarnessError::InvalidCase(format!(
                    "the case tree uses the harness-owned name {name:?}"
                )));
            }
        }
        for side in [Side::Oracle, Side::Rust] {
            let root = self.side(side);
            if root.exists() {
                return Err(HarnessError::InvalidCase(format!(
                    "{} already exists",
                    root.display()
                )));
            }
            copy_tree(&input, &root)?;
            copy_tree(resources.profiles, &root.join("profiles"))?;
            create_dir(&root.join("logs"))?;
            if let Some(hkxcmd) = resources.hkxcmd {
                create_dir(&root.join("bin"))?;
                copy_file(hkxcmd, &root.join("bin").join("hkxcmd.exe"))?;
            }
        }
        Ok(())
    }
}

fn create_dir(path: &Path) -> Result<(), HarnessError> {
    std::fs::create_dir(path)
        .map_err(|error| HarnessError::io(format!("creating {}", path.display()), error))
}

fn copy_file(from: &Path, to: &Path) -> Result<(), HarnessError> {
    std::fs::copy(from, to).map(drop).map_err(|error| {
        HarnessError::io(
            format!("copying {} to {}", from.display(), to.display()),
            error,
        )
    })
}

/// Copies a tree of plain files and directories; a link is an invalid case.
fn copy_tree(from: &Path, to: &Path) -> Result<(), HarnessError> {
    create_dir(to)?;
    let listing = std::fs::read_dir(from)
        .map_err(|error| HarnessError::io(format!("listing {}", from.display()), error))?;
    for item in listing {
        let item =
            item.map_err(|error| HarnessError::io(format!("listing {}", from.display()), error))?;
        let file_type = item.file_type().map_err(|error| {
            HarnessError::io(format!("reading {}", item.path().display()), error)
        })?;
        let target = to.join(item.file_name());
        if file_type.is_symlink() {
            return Err(HarnessError::InvalidCase(format!(
                "{} is a link; links are applied per side, never copied",
                item.path().display()
            )));
        } else if file_type.is_dir() {
            copy_tree(&item.path(), &target)?;
        } else {
            copy_file(&item.path(), &target)?;
        }
    }
    Ok(())
}

/// Renders a spec into the oracle's command line, run from `oracle_root`.
///
/// Every option is stated explicitly, so the oracle's defaults never decide a
/// value. A spec the oracle cannot express is a [`HarnessError::InvalidCase`]
/// here, never a silently different run: a selection outside the case tree or
/// inside a harness-owned folder, a profile that is not one folder name, or a
/// mesh level the GUI cannot produce.
pub fn oracle_arguments(
    spec: &CaseSpec,
    oracle_root: &Path,
) -> Result<Vec<OsString>, HarnessError> {
    let invalid = |message: String| Err(HarnessError::InvalidCase(message));
    let components: Vec<&str> = spec.selection.split('/').collect();
    let plain = |component: &str| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && !component.contains(['\\', ':'])
    };
    if !components.iter().all(|component| plain(component))
        || HARNESS_OWNED
            .iter()
            .any(|owned| components[0].eq_ignore_ascii_case(owned))
    {
        return invalid(format!(
            "selection `{}` is not a folder of the case tree",
            spec.selection
        ));
    }
    if !plain(&spec.profile) || spec.profile.contains('/') {
        return invalid(format!(
            "profile `{}` is not a profile folder name",
            spec.profile
        ));
    }
    if spec.meshes.level > 3 {
        return invalid(format!("mesh level {} is outside 0-3", spec.meshes.level));
    }

    let selection = components
        .iter()
        .fold(oracle_root.to_path_buf(), |path, component| {
            path.join(component)
        });
    let mode = match spec.mode {
        SelectionMode::OneMod => "om",
        SelectionMode::SeveralMods => "sm",
    };
    let mut arguments = Arguments(vec![
        selection.into_os_string(),
        mode.into(),
        spec.profile.as_str().into(),
    ]);
    let (textures, meshes, archives) = (&spec.textures, &spec.meshes, &spec.archives);
    // Qt parses a single-dash multi-letter option as compacted short options,
    // so every option uses `--`. Values are separate arguments.
    arguments.switch(spec.dry_run, "--dr");
    arguments.value("--m", meshes.level);
    arguments.switch(meshes.headparts, "--mh");
    arguments.switch(meshes.resave, "--mr");
    arguments.switch(textures.necessary, "--t0");
    arguments.switch(textures.compress, "--t1");
    arguments.switch(textures.mipmaps, "--t2");
    arguments.switch(textures.resize_by_ratio, "--trr");
    arguments.value("--trrw", textures.ratio_width);
    arguments.value("--trrh", textures.ratio_height);
    arguments.switch(textures.resize_by_size, "--trs");
    arguments.value("--trsw", textures.target_width);
    arguments.value("--trsh", textures.target_height);
    arguments.switch(spec.animations, "--a");
    arguments.switch(archives.extract, "--be");
    arguments.switch(archives.create, "--bc");
    arguments.switch(archives.delete_backup, "--bd");
    arguments.value("--bcomp", u8::from(archives.compress));
    arguments.value("--bdum", u8::from(archives.create_dummies));
    arguments.value("--bmi", u8::from(archives.merge_incompressible));
    arguments.value("--bmt", u8::from(archives.merge_textures));
    arguments.value("--bds", u8::from(archives.delete_sources));
    Ok(arguments.0)
}

/// An oracle command line under construction.
struct Arguments(Vec<OsString>);

impl Arguments {
    /// Adds a presence switch when it is enabled.
    fn switch(&mut self, enabled: bool, name: &str) {
        if enabled {
            self.0.push(name.into());
        }
    }

    /// Adds an option and its value as two arguments.
    fn value(&mut self, name: &str, value: impl ToString) {
        self.0.push(name.into());
        self.0.push(value.to_string().into());
    }
}

/// Builds the two processes a case runs.
pub trait CaseDrivers {
    /// The oracle process, with its working directory set to the case's
    /// `oracle/` folder.
    fn oracle(&self, layout: &CaseLayout, spec: &CaseSpec) -> Result<Command, HarnessError>;

    /// The Rust driver process, which must write the case's raw `RunFacts` to
    /// [`CaseLayout::rust_facts`] and exit 0 whenever it produced facts.
    fn rust(&self, layout: &CaseLayout) -> Result<Command, HarnessError>;
}

/// The real builds: the oracle exe and `cao-parity run`.
pub struct ProductionDrivers {
    /// From `--oracle` or `CAO_ORACLE`; never discovered.
    pub oracle_exe: PathBuf,
    /// The `cao-parity` exe itself.
    pub parity_exe: PathBuf,
}

impl CaseDrivers for ProductionDrivers {
    fn oracle(&self, layout: &CaseLayout, spec: &CaseSpec) -> Result<Command, HarnessError> {
        let root = layout.side(Side::Oracle);
        let mut command = Command::new(&self.oracle_exe);
        // C++ resolves profiles/, bin/ and logs/ against the working directory.
        command
            .args(oracle_arguments(spec, &root)?)
            .current_dir(root);
        Ok(command)
    }

    fn rust(&self, layout: &CaseLayout) -> Result<Command, HarnessError> {
        let mut command = Command::new(&self.parity_exe);
        command
            .arg("run")
            .arg("--case")
            .arg(layout.case_file())
            .arg("--app-dir")
            .arg(layout.side(Side::Rust))
            .arg("--facts")
            .arg(layout.rust_facts())
            .current_dir(layout.side(Side::Rust));
        Ok(command)
    }
}

/// A case's two verdicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseResult {
    pub facts: Verdict<FactDifference>,
    pub tree: Verdict<ArtifactDifference>,
}

impl CaseResult {
    /// Whether neither verdict is Different.
    pub fn passed(&self) -> bool {
        self.facts.passed() && self.tree.passed()
    }
}

/// Runs one provisioned case: the oracle, then the Rust driver, then the fact
/// and output-tree comparisons.
///
/// A passing case's directory is deleted. A Different case, or one that hit a
/// harness error, is kept with a `report.md`. A harness error is returned as
/// `Err` after its report is written.
pub fn run_case(
    layout: &CaseLayout,
    drivers: &dyn CaseDrivers,
    rules: &dyn TreeRules,
    timeout: Duration,
) -> Result<CaseResult, HarnessError> {
    match evaluate(layout, drivers, rules, timeout) {
        Ok(result) if result.passed() => {
            std::fs::remove_dir_all(layout.root()).map_err(|error| {
                HarnessError::io(format!("deleting {}", layout.root().display()), error)
            })?;
            Ok(result)
        }
        Ok(result) => {
            write_report(layout, &different_report(&result))?;
            Ok(result)
        }
        Err(error) => {
            // The harness error is what the caller must see; a report that
            // cannot be written as well must not replace it.
            let _ = write_report(layout, &format!("## Harness error\n\n{error}\n"));
            Err(error)
        }
    }
}

fn evaluate(
    layout: &CaseLayout,
    drivers: &dyn CaseDrivers,
    rules: &dyn TreeRules,
    timeout: Duration,
) -> Result<CaseResult, HarnessError> {
    let spec = layout.read_case()?.spec;
    // The oracle always runs first, so a Rust driver crash cannot disturb it.
    let oracle_exit = run_side(
        layout,
        Side::Oracle,
        drivers.oracle(layout, &spec)?,
        timeout,
    )?;
    let rust_exit = run_side(layout, Side::Rust, drivers.rust(layout)?, timeout)?;
    if rust_exit != 0 {
        return Err(HarnessError::DriverFailed { code: rust_exit });
    }

    let stdout_path = layout.stdout(Side::Oracle);
    let stdout = std::fs::read(&stdout_path)
        .map_err(|error| HarnessError::io(format!("reading {}", stdout_path.display()), error))?;
    let oracle_facts = oracle::parse(&stdout, oracle_exit)?;
    let facts_path = layout.rust_facts();
    let facts_bytes = std::fs::read(&facts_path)
        .map_err(|error| HarnessError::io(format!("reading {}", facts_path.display()), error))?;
    let rust_facts: RunFacts =
        serde_json::from_slice(&facts_bytes).map_err(|source| HarnessError::Json {
            path: facts_path,
            source,
        })?;

    let (oracle_root, rust_root) = (layout.side(Side::Oracle), layout.side(Side::Rust));
    let facts = compare_facts(
        &normalise(&oracle_facts, &oracle_root)?,
        &normalise(&rust_facts, &rust_root)?,
    );
    let tree = compare_trees(
        TreeSide {
            root: &oracle_root,
            run_id: run_id(&oracle_facts),
        },
        TreeSide {
            root: &rust_root,
            run_id: run_id(&rust_facts),
        },
        rules,
    )?
    .verdict();
    Ok(CaseResult { facts, tree })
}

fn run_id(facts: &RunFacts) -> Option<&str> {
    match facts {
        RunFacts::Started(run) => Some(&run.run_id),
        RunFacts::StartError(_) => None,
    }
}

/// How often a running side is checked against the timeout.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Runs one side to completion, capturing its output into the case directory.
///
/// Output goes straight to files rather than pipes: a pipe would need reader
/// threads, and a grandchild that inherited it (`hkxcmd.exe`) could keep those
/// threads blocked after the side itself was killed. Returns the exit code.
fn run_side(
    layout: &CaseLayout,
    side: Side,
    mut command: Command,
    timeout: Duration,
) -> Result<i32, HarnessError> {
    let capture = |path: PathBuf| {
        std::fs::File::create(&path)
            .map_err(|error| HarnessError::io(format!("creating {}", path.display()), error))
    };
    command
        .stdin(Stdio::null())
        .stdout(capture(layout.stdout(side))?)
        .stderr(capture(layout.stderr(side))?);
    let mut child = command
        .spawn()
        .map_err(|error| HarnessError::io(format!("starting the {} side", side.name()), error))?;
    let deadline = Instant::now() + timeout;
    loop {
        let status = child.try_wait().map_err(|error| {
            HarnessError::io(format!("waiting for the {} side", side.name()), error)
        })?;
        if let Some(status) = status {
            // Windows always reports an exit code; `None` would mean a signal.
            return Ok(status.code().unwrap_or(-1));
        }
        if Instant::now() >= deadline {
            // Kill and reap; the side may have exited in between, which is fine.
            let _ = child.kill();
            let _ = child.wait();
            return Err(HarnessError::Timeout {
                side: side.name(),
                seconds: timeout.as_secs(),
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn verdict_name<D>(verdict: &Verdict<D>) -> &'static str {
    match verdict {
        Verdict::Identical => "Identical",
        Verdict::Equivalent => "Equivalent",
        Verdict::Different(_) => "Different",
    }
}

/// The body of a Different case's report: each verdict and its differences.
fn different_report(result: &CaseResult) -> String {
    let mut text = format!(
        "## Verdicts\n\n- Run facts: {}\n- Output tree: {}\n",
        verdict_name(&result.facts),
        verdict_name(&result.tree)
    );
    if let Verdict::Different(differences) = &result.facts {
        text.push_str("\n## Run fact differences\n\n```text\n");
        for difference in differences {
            text.push_str(&difference.to_string());
        }
        text.push_str("```\n");
    }
    if let Verdict::Different(differences) = &result.tree {
        text.push_str("\n## Output tree differences\n\n");
        for difference in differences {
            text.push_str(&format!(
                "- `{}` broke {}: {}\n",
                difference.path, difference.rule, difference.detail
            ));
        }
    }
    text
}

/// Writes `report.md`: the body, then the captures and the replay command.
fn write_report(layout: &CaseLayout, body: &str) -> Result<(), HarnessError> {
    let mut text = format!("# Parity case `{}`\n\n{body}\n## Captures\n\n", layout.id());
    for side in [Side::Oracle, Side::Rust] {
        for path in [layout.stdout(side), layout.stderr(side)] {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            text.push_str(&format!("- `{name}`\n"));
        }
    }
    text.push_str(&format!(
        "- `rust.facts.json`\n\n## Replay\n\n```text\ncao-parity case {}\n```\n",
        layout.id()
    ));
    std::fs::write(layout.report(), text)
        .map_err(|error| HarnessError::io(format!("writing {}", layout.report().display()), error))
}
