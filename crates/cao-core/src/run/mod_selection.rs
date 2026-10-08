//! Mod Selection resolution: the Preparing step that turns a request's Mod
//! Selection into the canonical Mod Roots a run processes.
//!
//! Ported from `resolveModRoots` in `src/Run/RunExecutor.cpp`. One Mod Root
//! resolves to itself. Several Mods resolves each immediate child directory of
//! the mods directory as its own Mod Root, less its Mod Exclusions, which are
//! recorded as Run Diagnostics and never affect the Run Outcome. Nothing here
//! mutates the filesystem.
//!
//! Two recorded deviations from C++ live here:
//!
//! - Deviation 19: a child in CAO's reserved `.cao-staging` namespace is never
//!   a Mod Root, and is skipped silently rather than diagnosed.
//! - Deviation 20: a separator is a child whose name *ends in* a configured
//!   suffix (`_separator`, MO2's convention), not one containing a marker.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use cao_winfs::{Access, FileIdentity, Open, Share};

use crate::run::{
    CancellationToken, ModSelection, MutableRunEvidence, RunConfiguration, RunDiagnostic,
    RunDiagnosticCode, RunFailure, RunFailureCode, RunPhase, is_staging_name,
};

/// A ModSelectionResolutionFailed Run Failure in Preparing.
fn resolution_failure(detail: impl Into<String>) -> RunFailure {
    RunFailure::new(
        RunFailureCode::ModSelectionResolutionFailed,
        RunPhase::Preparing,
        detail,
    )
}

/// Folds a name for case-insensitive matching and ordering, independently of
/// the process locale.
///
/// C++ used utf8proc's full Unicode case folding. This is Unicode lowercase
/// plus the full foldings in which lowercase and folding disagree for letters
/// a mod name plausibly holds: `ß` and `ẞ` fold to `ss`, final sigma to sigma,
/// long s to `s` and the micro sign to mu. Corpus names are ASCII, where both
/// agree exactly.
fn folded_name(name: &str) -> String {
    let mut folded = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '\u{df}' | '\u{1e9e}' => folded.push_str("ss"),
            '\u{3c2}' => folded.push('\u{3c3}'),
            '\u{17f}' => folded.push('s'),
            '\u{b5}' => folded.push('\u{3bc}'),
            other => folded.extend(other.to_lowercase()),
        }
    }
    folded
}

/// The identity of the directory at `path`, which must exist.
///
/// The open never follows a link, so `path` should already be canonical; then
/// every component of it is a real directory.
fn directory_identity(path: &Path) -> io::Result<FileIdentity> {
    let directory = Open::new(
        Access::READ_ATTRIBUTES,
        Share::READ | Share::WRITE | Share::DELETE,
    )
    .directory()
    .open(path)?;
    FileIdentity::of(&directory)
}

/// Reports whether `directory` is `boundary` or lies beneath it, by directory
/// identity rather than text (C++ `containsDirectory`).
///
/// A text prefix would confuse siblings such as `Mod` and `Mod2`, and cannot
/// recognize two paths to one directory, such as a volume mounted twice.
/// Lookup errors propagate, so Preparing fails rather than accept uncertain
/// containment.
fn contains_directory(boundary: &Path, directory: &Path) -> io::Result<bool> {
    let boundary = directory_identity(boundary)?;
    for ancestor in directory.ancestors() {
        if directory_identity(ancestor)? == boundary {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The name of a child as `/`-separated generic text, for failure details.
fn display_name(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// One immediate child directory of a mods directory, before resolution.
struct Child {
    path: PathBuf,
    name: String,
    folded: String,
}

/// Which Mod Exclusion, if any, applies to a child.
///
/// A child matching both policies owes one exclusion, and the separator rule
/// takes precedence, as in C++.
fn exclusion(
    child: &Child,
    configuration: &RunConfiguration,
    ignored: &HashSet<String>,
) -> Option<RunDiagnostic> {
    // Deviation 20: a suffix, matched case-sensitively. An empty suffix would
    // match every child, so it never excludes anything.
    let separator = configuration
        .separator_suffixes
        .iter()
        .any(|suffix| !suffix.is_empty() && child.name.ends_with(suffix.as_str()));
    let (code, detail) = if separator {
        (
            RunDiagnosticCode::SeparatorModExcluded,
            "The child Mod Root matches a configured separator marker",
        )
    } else if ignored.contains(&child.folded) {
        (
            RunDiagnosticCode::IgnoredModExcluded,
            "The child Mod Root matches an ignored-mod name",
        )
    } else {
        return None;
    };
    Some(RunDiagnostic::new(code, RunPhase::Preparing, detail).with_path(&child.path))
}

/// Resolves the Mod Selection into canonical Mod Roots, in run order.
///
/// Mod Exclusions enter `evidence`, and are published, as they are found, so
/// an observer can cancel between children. On cancellation the roots found so
/// far are returned and the caller discards them. A filesystem root cannot
/// bound one mod or a mods directory safely, and two Several Mods children
/// that resolve to overlapping directories would process the same files
/// twice, so both fail Preparing.
pub(crate) fn resolve_mod_roots(
    selection: &ModSelection,
    configuration: &RunConfiguration,
    evidence: &RefCell<MutableRunEvidence<'_>>,
    stop: &CancellationToken,
) -> Result<Vec<PathBuf>, RunFailure> {
    // MSVC's canonical text, so a Mod Root compares equal to the one a C++
    // run recorded in its staging manifest.
    let root = cao_winfs::msvc_canonical(selection.directory())
        .ok()
        .filter(|root| root.is_dir())
        .ok_or_else(|| {
            resolution_failure(
                "The selected Mod Root could not be resolved to an existing directory",
            )
        })?;
    if root.parent().is_none() {
        return Err(resolution_failure(
            "A filesystem root cannot be selected as a Mod Root or mods directory",
        ));
    }
    let ModSelection::ChildModRoots(_) = selection else {
        return Ok(vec![root]);
    };

    let ignored: HashSet<String> = configuration
        .ignored_mods
        .iter()
        .map(|name| folded_name(name))
        .collect();
    let failed = |error: io::Error| resolution_failure(error.to_string());

    let mut children = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(failed)? {
        if stop.is_cancelled() {
            return Ok(Vec::new());
        }
        let entry = entry.map_err(failed)?;
        // Deviation 19: CAO's own reserved namespace is never a mod, so it is
        // skipped before any other rule and owes no diagnostic.
        if is_staging_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        // Follows links, as C++'s `directory_entry::is_directory` does; a
        // dangling link is not a directory.
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        children.push(Child {
            folded: folded_name(&name),
            name,
            path,
        });
    }
    // Entry names define run order; link targets do not.
    children.sort_by(|left, right| {
        left.folded
            .cmp(&right.folded)
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut roots: Vec<PathBuf> = Vec::new();
    for child in &children {
        if stop.is_cancelled() {
            return Ok(Vec::new());
        }
        if let Some(diagnostic) = exclusion(child, configuration, &ignored) {
            evidence.borrow_mut().record_diagnostic(diagnostic);
            continue;
        }
        let resolved = cao_winfs::msvc_canonical(&child.path).map_err(failed)?;
        if resolved.parent().is_none() || contains_directory(&resolved, &root).map_err(failed)? {
            return Err(resolution_failure(
                "A child Mod Root cannot resolve to the selected mods directory or its ancestor",
            ));
        }
        for existing in &roots {
            if contains_directory(existing, &resolved).map_err(failed)?
                || contains_directory(&resolved, existing).map_err(failed)?
            {
                return Err(RunFailure::new(
                    RunFailureCode::ConflictingModRoots,
                    RunPhase::Preparing,
                    format!(
                        "The selected Mod Roots overlap: {} and {}",
                        display_name(existing),
                        display_name(&child.path)
                    ),
                ));
            }
        }
        roots.push(resolved);
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::folded_name;

    #[test]
    fn folding_matches_full_case_folding_for_common_letters() {
        assert_eq!(folded_name("NeMeSiS"), "nemesis");
        assert_eq!(folded_name("Stra\u{df}e"), "strasse");
        assert_eq!(folded_name("STRA\u{1e9e}E"), "strasse");
        assert_eq!(folded_name("\u{100}ssets"), "\u{101}ssets");
        assert_eq!(folded_name("\u{3a3}\u{3c2}"), "\u{3c3}\u{3c3}");
    }
}
