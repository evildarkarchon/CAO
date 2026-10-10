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
use crate::recipe::{ProfileOverrides, TreeRecipe};
use crate::tree::{
    ArtifactComparison, ArtifactDifference, ArtifactVerdict, DefaultRules, TreeRules, TreeSide,
    compare_trees,
};

/// The top-level folders of a side that the harness provisions. They are not
/// part of the case tree and never part of its output.
pub const HARNESS_OWNED: [&str; 3] = ["profiles", "bin", "logs"];

/// Whether a top-level name is one of the [`HARNESS_OWNED`] folders. Matched
/// ASCII case-insensitively, as Windows matches names: `Logs` is `logs/`.
pub fn is_harness_owned(name: impl AsRef<std::ffi::OsStr>) -> bool {
    let name = name.as_ref();
    HARNESS_OWNED
        .iter()
        .any(|owned| name.eq_ignore_ascii_case(owned))
}

/// The contents of `case.json`: the whole case (#473).
///
/// The spec says what both builds are asked to do; the profile overrides and
/// the tree recipe say what they are run on. A case directory can always be
/// rebuilt from this file alone, so `case <id>` replays the same bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseFile {
    pub spec: CaseSpec,
    #[serde(default, skip_serializing_if = "ProfileOverrides::is_empty")]
    pub profile_overrides: ProfileOverrides,
    #[serde(default)]
    pub tree: TreeRecipe,
    /// The [`crate::generate::GENERATOR_VERSION`] that produced a generated
    /// case; absent for a seed. A replay compares it with the running
    /// generator's, because another version may build different bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_version: Option<u32>,
}

impl CaseFile {
    /// A case with no profile overrides and an empty tree.
    pub fn new(spec: CaseSpec) -> Self {
        Self {
            spec,
            profile_overrides: ProfileOverrides::default(),
            tree: TreeRecipe::default(),
            generator_version: None,
        }
    }
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
    pub mod_selection: ModSelection,
    pub dry_run: bool,
    pub textures: TextureOptions,
    pub meshes: MeshOptions,
    pub animations: bool,
    pub archives: ArchiveOptions,
}

/// The Mod Selection: one Mod Root (`om`), or the child Mod Roots of a mods
/// directory (`sm`). In JSON: `{"kind": "one_mod", "folder": "mods/Mod"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModSelection {
    /// `folder` is the Mod Root.
    OneMod { folder: String },
    /// `folder` is the mods directory whose children are the Mod Roots.
    SeveralMods { folder: String },
}

impl ModSelection {
    /// The selected folder, relative to the side's case root and `/`-separated.
    pub fn folder(&self) -> &str {
        match self {
            ModSelection::OneMod { folder } | ModSelection::SeveralMods { folder } => folder,
        }
    }
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

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
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
        serde_json::from_slice(&read_file(&path)?)
            .map_err(|source| HarnessError::Json { path, source })
    }

    /// Creates both sides from the materialised `input/` tree.
    ///
    /// Each side gets a byte-for-byte copy of the input, under the same names,
    /// plus private `profiles/`, `logs/` and, when given, `bin/hkxcmd.exe`.
    /// The input may not use a harness-owned top-level name, since the side's
    /// own folder would shadow it. Filesystem-shape operations (links,
    /// read-only files) are applied to each copy afterwards by
    /// [`crate::materialise`], so the input must contain only plain files and
    /// directories.
    pub fn provision(&self, resources: &SideResources<'_>) -> Result<(), HarnessError> {
        let input = self.input();
        for item in list_dir(&input)? {
            let name = item.file_name();
            if is_harness_owned(&name) {
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

/// Lists a directory's entries, reporting any failure with the directory's path.
pub(crate) fn list_dir(directory: &Path) -> Result<Vec<std::fs::DirEntry>, HarnessError> {
    let listing_error = |error| HarnessError::io(format!("listing {}", directory.display()), error);
    std::fs::read_dir(directory)
        .map_err(listing_error)?
        .map(|item| item.map_err(listing_error))
        .collect()
}

/// Reads a whole file, reporting any failure with its path.
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, HarnessError> {
    std::fs::read(path)
        .map_err(|error| HarnessError::io(format!("reading {}", path.display()), error))
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
    for item in list_dir(from)? {
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
    let selected = selected_folder(spec, oracle_root)?;
    let mode = match spec.mod_selection {
        ModSelection::OneMod { .. } => "om",
        ModSelection::SeveralMods { .. } => "sm",
    };
    let mut arguments = Arguments(vec![
        selected.into_os_string(),
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

/// Checks that both builds can express a spec, and returns its selected folder
/// under a side's case root.
///
/// The oracle's argv and the Rust driver's options model are both built from
/// this, so the two sides always select the same folder. A selection outside the
/// case tree or inside a harness-owned folder, a profile that is not one folder
/// name, or a mesh level the GUI cannot produce is a
/// [`HarnessError::InvalidCase`].
pub fn selected_folder(spec: &CaseSpec, side_root: &Path) -> Result<PathBuf, HarnessError> {
    let invalid = |message: String| Err(HarnessError::InvalidCase(message));
    let folder = spec.mod_selection.folder();
    let components: Vec<&str> = folder.split('/').collect();
    let plain = |component: &str| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && !component.contains(['\\', ':'])
    };
    if !components.iter().all(|component| plain(component)) || is_harness_owned(components[0]) {
        return invalid(format!(
            "Mod Selection folder `{folder}` is not a folder of the case tree"
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

    Ok(components
        .iter()
        .fold(side_root.to_path_buf(), |path, component| {
            path.join(component)
        }))
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

/// A case's two verdicts, and the verdict of every artifact behind the second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseResult {
    pub facts: Verdict<FactDifference>,
    pub tree: Verdict<ArtifactDifference>,
    /// Every compared path of the output tree, then, for a Dry Run, each path
    /// a side changed from the input, as a Different artifact.
    pub artifacts: Vec<ArtifactComparison>,
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
/// harness error, is kept with a `report.md` written for `report`. A harness
/// error is returned as `Err` after its report is written.
pub fn run_case(
    layout: &CaseLayout,
    drivers: &dyn CaseDrivers,
    rules: &dyn TreeRules,
    timeout: Duration,
    report: &ReportContext,
) -> Result<CaseResult, HarnessError> {
    match evaluate(layout, drivers, rules, timeout) {
        Ok(result) if result.passed() => {
            std::fs::remove_dir_all(layout.root()).map_err(|error| {
                HarnessError::io(format!("deleting {}", layout.root().display()), error)
            })?;
            Ok(result)
        }
        Ok(result) => {
            write_report(layout, report, &different_report(&result))?;
            Ok(result)
        }
        Err(error) => {
            write_harness_error_report(layout, report, &error);
            Err(error)
        }
    }
}

/// What a case's `report.md` says beyond its verdicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportContext {
    /// The exact command line that replays the case, with every option the
    /// run used.
    pub replay: String,
    /// The generator version the run used, which every corpus report records;
    /// `None` outside a corpus run, where no generator is involved.
    pub generator_version: Option<u32>,
}

/// Writes the `report.md` of a case that hit a harness error, best effort.
///
/// The harness error is what the caller must see; a report that cannot be
/// written as well must not replace it, so a write failure is dropped.
pub fn write_harness_error_report(
    layout: &CaseLayout,
    report: &ReportContext,
    error: &HarnessError,
) {
    let _ = write_report(layout, report, &format!("## Harness error\n\n{error}\n"));
}

/// Runs both sides and compares them, without deciding the case directory's fate.
///
/// `timeout` is the whole case's budget: the Rust side gets whatever time the
/// oracle left, so a case never takes longer than the timeout in total.
fn evaluate(
    layout: &CaseLayout,
    drivers: &dyn CaseDrivers,
    rules: &dyn TreeRules,
    timeout: Duration,
) -> Result<CaseResult, HarnessError> {
    let spec = layout.read_case()?.spec;
    let deadline = Deadline {
        at: Instant::now() + timeout,
        budget: timeout,
    };
    // The oracle always runs first, so a Rust driver crash cannot disturb it.
    let oracle_exit = run_side(
        layout,
        Side::Oracle,
        drivers.oracle(layout, &spec)?,
        deadline,
    )?;
    let rust_exit = run_side(layout, Side::Rust, drivers.rust(layout)?, deadline)?;
    if rust_exit != 0 {
        return Err(HarnessError::DriverFailed { code: rust_exit });
    }

    let oracle_facts = oracle::parse(&read_file(&layout.stdout(Side::Oracle))?, oracle_exit)?;
    let facts_path = layout.rust_facts();
    let rust_facts: RunFacts =
        serde_json::from_slice(&read_file(&facts_path)?).map_err(|source| HarnessError::Json {
            path: facts_path,
            source,
        })?;

    let (oracle_root, rust_root) = (layout.side(Side::Oracle), layout.side(Side::Rust));
    let facts = compare_facts(
        &normalise(&oracle_facts, &oracle_root)?,
        &normalise(&rust_facts, &rust_root)?,
    );
    let comparison = compare_trees(
        TreeSide {
            root: &oracle_root,
            run_id: run_id(&oracle_facts),
        },
        TreeSide {
            root: &rust_root,
            run_id: run_id(&rust_facts),
        },
        rules,
    )?;
    let mut tree = comparison.verdict();
    let mut artifacts = comparison.artifacts;
    if spec.dry_run {
        let changed = dry_run_changes(layout, &oracle_facts, &rust_facts)?;
        if !changed.is_empty() {
            artifacts.extend(changed.iter().map(|difference| ArtifactComparison {
                path: difference.path.clone(),
                verdict: ArtifactVerdict::Different(difference.clone()),
            }));
            let mut differences = match tree {
                Verdict::Different(differences) => differences,
                _ => Vec::new(),
            };
            differences.extend(changed);
            tree = Verdict::Different(differences);
        }
    }
    Ok(CaseResult {
        facts,
        tree,
        artifacts,
    })
}

/// The rule a Dry Run breaks when it leaves its side's tree different from the input.
pub const DRY_RUN_UNCHANGED: &str = "Dry Run Leaves The Input Unchanged";

/// Compares each side's tree with the pristine input, byte for byte.
///
/// Comparing the two sides with each other cannot catch a Dry Run that both
/// builds wrongly mutate the same way, so a Dry Run case also checks each side
/// against `input/`. Every path that is not Identical is a difference, reported
/// under its side's folder name.
fn dry_run_changes(
    layout: &CaseLayout,
    oracle_facts: &RunFacts,
    rust_facts: &RunFacts,
) -> Result<Vec<ArtifactDifference>, HarnessError> {
    let input = layout.input();
    let mut changes = Vec::new();
    for (side, facts) in [(Side::Oracle, oracle_facts), (Side::Rust, rust_facts)] {
        let root = layout.side(side);
        let comparison = compare_trees(
            TreeSide {
                root: &input,
                run_id: None,
            },
            TreeSide {
                root: &root,
                run_id: run_id(facts),
            },
            &DefaultRules,
        )?;
        for artifact in comparison.artifacts {
            let detail = match artifact.verdict {
                ArtifactVerdict::Identical => continue,
                ArtifactVerdict::Equivalent { rule } => {
                    format!("changed, though {rule} accepts it")
                }
                // The comparator's own wording names its two trees "oracle" and
                // "Rust"; here those are `input/` and this side.
                ArtifactVerdict::Different(difference) => format!(
                    "{} (oracle side = input/, Rust side = {side}/)",
                    difference.detail
                ),
            };
            changes.push(ArtifactDifference {
                path: format!("{side}/{}", artifact.path),
                rule: DRY_RUN_UNCHANGED,
                detail,
            });
        }
    }
    Ok(changes)
}

/// The Run ID a side's staging names carry, if it started a run.
fn run_id(facts: &RunFacts) -> Option<&str> {
    match facts {
        RunFacts::Started(run) => Some(&run.run_id),
        RunFacts::StartError(_) => None,
    }
}

/// The instant a case's time runs out, and the budget it was given.
#[derive(Clone, Copy)]
struct Deadline {
    at: Instant,
    budget: Duration,
}

/// How often a running side is checked against the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Runs one side to completion, capturing its output into the case directory.
///
/// Output goes straight to files rather than pipes: a pipe would need reader
/// threads, and a grandchild that inherited it (`hkxcmd.exe`) could keep those
/// threads blocked after the side itself was killed. Returns the exit code, or
/// [`HarnessError::Timeout`] after killing a side still running at the deadline.
fn run_side(
    layout: &CaseLayout,
    side: Side,
    mut command: Command,
    deadline: Deadline,
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
        .map_err(|error| HarnessError::io(format!("starting the {side} side"), error))?;
    loop {
        let status = child
            .try_wait()
            .map_err(|error| HarnessError::io(format!("waiting for the {side} side"), error))?;
        if let Some(status) = status {
            // Windows always reports an exit code; `None` would mean a signal.
            return Ok(status.code().unwrap_or(-1));
        }
        if Instant::now() >= deadline.at {
            // Kill and reap; the side may have exited in between, which is fine.
            let _ = child.kill();
            let _ = child.wait();
            return Err(HarnessError::Timeout {
                side,
                seconds: deadline.budget.as_secs(),
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The body of a Different case's report: each verdict and its differences.
fn different_report(result: &CaseResult) -> String {
    let mut text = format!(
        "## Verdicts\n\n- Run facts: {}\n- Output tree: {}\n",
        result.facts.name(),
        result.tree.name()
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

/// The most of one capture a report embeds; the whole file stays in the case
/// directory.
const EMBEDDED_CAPTURE_BYTES: usize = 64 * 1024;

/// Writes `report.md`: the generator version, the body, both sides' captures
/// embedded, the Rust facts and both sides' Application Logs by path, and the
/// exact replay command.
fn write_report(
    layout: &CaseLayout,
    report: &ReportContext,
    body: &str,
) -> Result<(), HarnessError> {
    let mut text = format!("# Parity case `{}`\n\n", layout.id());
    if let Some(version) = report.generator_version {
        text.push_str(&format!("Generator version: {version}\n\n"));
    }
    text.push_str(body);
    text.push_str("\n## Captures\n");
    let captures = [Side::Oracle, Side::Rust]
        .into_iter()
        .flat_map(|side| [layout.stdout(side), layout.stderr(side)]);
    for path in captures {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        text.push_str(&format!("\n### `{name}`\n\n"));
        match std::fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => text.push_str("(empty)\n"),
            Ok(bytes) => {
                let shown = &bytes[..bytes.len().min(EMBEDDED_CAPTURE_BYTES)];
                text.push_str(&fenced(&String::from_utf8_lossy(shown)));
                if shown.len() < bytes.len() {
                    text.push_str(&format!(
                        "\n(the first {} of {} bytes; the whole capture is in the case directory)\n",
                        shown.len(),
                        bytes.len()
                    ));
                }
            }
            // The side never ran, as when the case failed before it started.
            Err(_) => text.push_str("(not captured)\n"),
        }
    }
    text.push_str("\n## Rust facts\n\n");
    text.push_str(if layout.rust_facts().is_file() {
        "- `rust.facts.json`\n"
    } else {
        "(not written)\n"
    });
    text.push_str("\n## Logs\n\n");
    for side in [Side::Oracle, Side::Rust] {
        let mut logs = Vec::new();
        collect_files(&layout.side(side).join("logs"), side.name(), &mut logs);
        logs.sort();
        if logs.is_empty() {
            text.push_str(&format!("- {side}: no Application Log was written\n"));
        }
        for log in logs {
            text.push_str(&format!("- `{log}`\n"));
        }
    }
    text.push_str(&format!("\n## Replay\n\n```text\n{}\n```\n", report.replay));
    std::fs::write(layout.report(), text)
        .map_err(|error| HarnessError::io(format!("writing {}", layout.report().display()), error))
}

/// `text` in a code fence longer than any backtick run inside it, so a capture
/// can never close its own fence.
fn fenced(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let newline = if text.ends_with('\n') { "" } else { "\n" };
    format!("{fence}text\n{text}{newline}{fence}\n")
}

/// Collects every file beneath `directory` as a `/`-separated path: `prefix`,
/// then `directory`'s own name, then the path within it. A missing directory
/// adds nothing.
fn collect_files(directory: &Path, prefix: &str, found: &mut Vec<String>) {
    let Ok(items) = std::fs::read_dir(directory) else {
        // No `logs/` folder: the side never got far enough to write one.
        return;
    };
    let prefix = format!(
        "{prefix}/{}",
        directory.file_name().unwrap_or_default().to_string_lossy()
    );
    // An entry that cannot be read is skipped: the list only points a reader
    // at logs, and the case directory itself is kept for them to browse.
    for item in items.flatten() {
        let path = item.path();
        if path.is_dir() {
            collect_files(&path, &prefix, found);
        } else {
            found.push(format!("{prefix}/{}", item.file_name().to_string_lossy()));
        }
    }
}
