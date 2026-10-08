//! The output-tree comparator (#467).
//!
//! Both sides' case trees must hold the same relative paths, compared
//! case-sensitively. A file whose bytes match on both sides is Identical. A
//! file whose bytes differ is decided by a rule from the [`TreeRules`] hooks:
//! the leftovers hook first, then the per-Asset-Kind hook. Without a rule,
//! bytes must be equal. Timestamps and attributes are never read.
//!
//! The engine slices fill the hooks: texture PSNR, archive parsing, the
//! staging manifest's semantic comparison. Until then [`DefaultRules`]
//! requires every file to be byte-identical.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::HarnessError;
use crate::case::HARNESS_OWNED;
use crate::compare::Verdict;
use crate::normalise::staging_placeholders;

/// One side's output tree.
pub struct TreeSide<'a> {
    /// The side's case root (`oracle/` or `rust/`). Its harness-owned folders
    /// (`profiles/`, `bin/`, `logs/`) are not output and are skipped.
    pub root: &'a Path,
    /// The side's Run ID, used to normalise staging names. `None` when the
    /// side reported a Start Error, so no staging can carry its Run ID.
    pub run_id: Option<&'a str>,
}

/// What a rule decided about two files whose bytes differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleOutcome {
    Equivalent,
    /// Breaks the rule, with a description for the report.
    Different(String),
}

/// A comparison rule for one kind of artifact, such as a texture's PSNR rule.
pub trait ArtifactRule {
    /// The rule's name, reported with every verdict it produces.
    fn name(&self) -> &'static str;

    /// Compares one artifact's two files. It is only called when their bytes
    /// differ, so it never needs to report Identical.
    fn compare(&self, oracle: &Path, rust: &Path) -> Result<RuleOutcome, HarnessError>;
}

/// The comparator's hooks. Both receive the artifact's normalised relative
/// path and return `None` when they have no rule for it.
pub trait TreeRules {
    /// The leftovers hook: `.caobad` and `.bak` files, staging residue,
    /// `ownership.manifest` and `owner.lock`. Consulted first.
    fn leftover_rule(&self, path: &str) -> Option<&dyn ArtifactRule> {
        let _ = path;
        None
    }

    /// The per-Asset-Kind hook: Textures, Meshes, Animations, Archives and
    /// Loading Plugins.
    fn asset_rule(&self, path: &str) -> Option<&dyn ArtifactRule> {
        let _ = path;
        None
    }
}

/// No rules: every file must be byte-identical.
pub struct DefaultRules;

impl TreeRules for DefaultRules {}

/// Why one artifact failed the comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDifference {
    /// The artifact's normalised relative path.
    pub path: String,
    /// The broken rule's name.
    pub rule: &'static str,
    pub detail: String,
}

/// One artifact's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactVerdict {
    Identical,
    /// Different bytes that the named rule accepts.
    Equivalent {
        rule: &'static str,
    },
    Different(ArtifactDifference),
}

/// One compared path: a file, a directory or a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactComparison {
    /// The normalised relative path.
    pub path: String,
    pub verdict: ArtifactVerdict,
}

/// Every artifact of both trees, in path order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeComparison {
    pub artifacts: Vec<ArtifactComparison>,
}

impl TreeComparison {
    /// The whole tree's verdict: Different if any artifact is, Equivalent if
    /// any artifact needed a rule, Identical otherwise.
    pub fn verdict(&self) -> Verdict<ArtifactDifference> {
        let different: Vec<ArtifactDifference> = self
            .artifacts
            .iter()
            .filter_map(|artifact| match &artifact.verdict {
                ArtifactVerdict::Different(difference) => Some(difference.clone()),
                _ => None,
            })
            .collect();
        if !different.is_empty() {
            Verdict::Different(different)
        } else if self
            .artifacts
            .iter()
            .any(|artifact| matches!(artifact.verdict, ArtifactVerdict::Equivalent { .. }))
        {
            Verdict::Equivalent
        } else {
            Verdict::Identical
        }
    }
}

const SAME_PATHS: &str = "Same Relative Paths";
const SAME_KIND: &str = "Same Entry Kind";
const BYTE_EQUALITY: &str = "Byte Equality";

/// What a tree entry is. Links (symlinks and junctions) are never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
    Link,
}

/// One entry, keyed elsewhere by its normalised path.
struct Entry {
    absolute: PathBuf,
    kind: EntryKind,
}

/// Compares the oracle's output tree with the Rust side's.
///
/// Fails only when a tree cannot be read or a rule cannot run; every
/// difference between the trees is a verdict instead.
pub fn compare_trees(
    oracle: TreeSide<'_>,
    rust: TreeSide<'_>,
    rules: &dyn TreeRules,
) -> Result<TreeComparison, HarnessError> {
    let mut oracle_entries = entries(&oracle)?;
    let mut rust_entries = entries(&rust)?;
    let mut paths: Vec<String> = oracle_entries
        .keys()
        .chain(rust_entries.keys())
        .cloned()
        .collect();
    paths.sort_unstable();
    paths.dedup();

    let mut artifacts = Vec::new();
    for path in paths {
        let oracle_side = oracle_entries.remove(&path).unwrap_or_default();
        let rust_side = rust_entries.remove(&path).unwrap_or_default();
        let paired = oracle_side.len().min(rust_side.len());
        for (oracle_entry, rust_entry) in oracle_side.iter().zip(&rust_side) {
            let verdict = compare_entry(&path, oracle_entry, rust_entry, rules)?;
            artifacts.push(ArtifactComparison {
                path: path.clone(),
                verdict,
            });
        }
        // Normalised staging names can collide; unpaired extras exist on one side only.
        for (side, extra) in [
            ("oracle", &oracle_side[paired..]),
            ("Rust", &rust_side[paired..]),
        ] {
            for _ in extra {
                artifacts.push(different(
                    &path,
                    SAME_PATHS,
                    format!("present only on the {side} side"),
                ));
            }
        }
    }
    Ok(TreeComparison { artifacts })
}

fn different(path: &str, rule: &'static str, detail: String) -> ArtifactComparison {
    ArtifactComparison {
        path: path.to_owned(),
        verdict: ArtifactVerdict::Different(ArtifactDifference {
            path: path.to_owned(),
            rule,
            detail,
        }),
    }
}

fn compare_entry(
    path: &str,
    oracle: &Entry,
    rust: &Entry,
    rules: &dyn TreeRules,
) -> Result<ArtifactVerdict, HarnessError> {
    if oracle.kind != rust.kind {
        return Ok(different(
            path,
            SAME_KIND,
            format!(
                "{:?} on the oracle side, {:?} on the Rust side",
                oracle.kind, rust.kind
            ),
        )
        .verdict);
    }
    if oracle.kind != EntryKind::File {
        return Ok(ArtifactVerdict::Identical);
    }
    let read = |entry: &Entry| {
        std::fs::read(&entry.absolute).map_err(|error| {
            HarnessError::io(format!("reading {}", entry.absolute.display()), error)
        })
    };
    let (oracle_bytes, rust_bytes) = (read(oracle)?, read(rust)?);
    if oracle_bytes == rust_bytes {
        return Ok(ArtifactVerdict::Identical);
    }
    let Some(rule) = rules.leftover_rule(path).or_else(|| rules.asset_rule(path)) else {
        return Ok(different(
            path,
            BYTE_EQUALITY,
            byte_difference(&oracle_bytes, &rust_bytes),
        )
        .verdict);
    };
    Ok(match rule.compare(&oracle.absolute, &rust.absolute)? {
        RuleOutcome::Equivalent => ArtifactVerdict::Equivalent { rule: rule.name() },
        RuleOutcome::Different(detail) => different(path, rule.name(), detail).verdict,
    })
}

/// Describes where two unequal byte strings first differ.
fn byte_difference(oracle: &[u8], rust: &[u8]) -> String {
    let first = oracle
        .iter()
        .zip(rust)
        .position(|(a, b)| a != b)
        .unwrap_or(oracle.len().min(rust.len()));
    format!(
        "{} bytes on the oracle side, {} on the Rust side; first difference at byte {first}",
        oracle.len(),
        rust.len()
    )
}

/// Lists a side's entries by normalised relative path, skipping harness-owned
/// top-level folders.
///
/// Several entries can share a normalised path when staging names differ only
/// in their nonces; each list is sorted by its real path, so pairing is
/// deterministic.
fn entries(side: &TreeSide<'_>) -> Result<BTreeMap<String, Vec<Entry>>, HarnessError> {
    let mut found: Vec<(String, Entry)> = Vec::new();
    walk(side.root, "", &mut found)?;
    found.sort_by(|a, b| a.1.absolute.cmp(&b.1.absolute));
    let mut entries: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for (relative, entry) in found {
        let key = match side.run_id {
            Some(run_id) => staging_placeholders(&relative, run_id),
            None => relative,
        };
        entries.entry(key).or_default().push(entry);
    }
    Ok(entries)
}

fn walk(
    directory: &Path,
    relative: &str,
    found: &mut Vec<(String, Entry)>,
) -> Result<(), HarnessError> {
    let listing = std::fs::read_dir(directory)
        .map_err(|error| HarnessError::io(format!("listing {}", directory.display()), error))?;
    for item in listing {
        let item = item
            .map_err(|error| HarnessError::io(format!("listing {}", directory.display()), error))?;
        let name = item.file_name();
        let name = name.to_str().ok_or_else(|| {
            HarnessError::InvalidCase(format!("{} holds a non-UTF-8 name", directory.display()))
        })?;
        if relative.is_empty() && HARNESS_OWNED.contains(&name) {
            continue;
        }
        let child = if relative.is_empty() {
            name.to_owned()
        } else {
            format!("{relative}/{name}")
        };
        // `file_type` comes from the directory listing and does not follow
        // links; std reports junctions as symlinks too.
        let file_type = item.file_type().map_err(|error| {
            HarnessError::io(format!("reading {}", item.path().display()), error)
        })?;
        let kind = if file_type.is_symlink() {
            EntryKind::Link
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        if kind == EntryKind::Directory {
            walk(&item.path(), &child, found)?;
        }
        found.push((
            child,
            Entry {
                absolute: item.path(),
                kind,
            },
        ));
    }
    Ok(())
}
