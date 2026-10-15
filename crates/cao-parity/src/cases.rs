//! Committed seed cases, which `cao-parity case <id>` can materialise by name.
//!
//! A seed is a whole `case.json` (spec, profile overrides and tree recipe)
//! committed under `crates/cao-parity/seeds/`, in any subfolder. Its case id is
//! its file stem, so ids must be unique across the folder. The hand-written
//! tracer and Several Mods cases live in `seeds/hand-written/`. Transcribed C++
//! scenarios (#507) live in `seeds/<CppSuite>/`, each naming the C++ test it
//! came from in its `origin`; the scenarios that could not be transcribed are
//! listed in `seeds/UNTRANSCRIBED.md`. The fault seeds, each provoked purely
//! by the tree, live in `seeds/fault/`.
//!
//! The harness only ever runs from a checkout, so seeds and fixtures are read
//! from the source tree rather than embedded.

use std::path::{Path, PathBuf};

use crate::HarnessError;
use crate::case::{CaseFile, list_dir, read_file};

/// `crates/cao-parity/seeds`.
pub fn seeds_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds")
}

/// `crates/cao-parity/fixtures`, where a recipe's `raw` entries find their
/// committed fixture files.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// The folder under `seeds/` holding the fault seeds (#507).
pub const FAULT_FOLDER: &str = "fault";

/// One committed seed and where it sits.
#[derive(Debug, Clone)]
pub struct Seed {
    /// The case id: the file stem.
    pub id: String,
    /// The first folder below `seeds/` the seed sits in, such as
    /// `hand-written`, `fault` or a C++ suite name; empty for a seed directly
    /// in `seeds/`.
    pub folder: String,
    pub case: CaseFile,
}

/// Every seed, by case id, sorted by id.
///
/// # Errors
/// Those of [`seed_files`].
pub fn seeds() -> Result<Vec<(String, CaseFile)>, HarnessError> {
    Ok(seed_files()?
        .into_iter()
        .map(|seed| (seed.id, seed.case))
        .collect())
}

/// Every seed with the folder it sits in, sorted by id.
///
/// # Errors
/// [`HarnessError::InvalidCase`] when two seeds share an id or a seed is not a
/// valid `case.json`; [`HarnessError::Io`] when the folder cannot be read.
pub fn seed_files() -> Result<Vec<Seed>, HarnessError> {
    let mut found = Vec::new();
    collect(&seeds_dir(), None, &mut found)?;
    // Ids are compared ignoring ASCII case, as Windows compares the case
    // folders they name: `Foo` and `foo` would share one work directory.
    found.sort_by_key(|(id, _, _)| id.to_ascii_lowercase());
    if let Some(pair) = found
        .windows(2)
        .find(|pair| pair[0].0.eq_ignore_ascii_case(&pair[1].0))
    {
        return Err(HarnessError::InvalidCase(format!(
            "two seeds have the case id `{}`",
            pair[0].0
        )));
    }
    found
        .into_iter()
        .map(|(id, folder, path)| {
            let case = serde_json::from_slice(&read_file(&path)?)
                .map_err(|source| HarnessError::Json { path, source })?;
            Ok(Seed { id, folder, case })
        })
        .collect()
}

/// The seed whose case id is `id`, if there is one.
///
/// # Errors
/// Those of [`seeds`]: every seed is read, so a broken seed is never hidden.
pub fn seed(id: &str) -> Result<Option<CaseFile>, HarnessError> {
    Ok(seeds()?
        .into_iter()
        .find(|(seed_id, _)| seed_id == id)
        .map(|(_, case)| case))
}

/// Collects every `*.json` beneath `directory` with its file stem and the
/// first folder below `seeds/` it sits in (`top`, once known).
fn collect(
    directory: &Path,
    top: Option<&str>,
    found: &mut Vec<(String, String, PathBuf)>,
) -> Result<(), HarnessError> {
    for item in list_dir(directory)? {
        let path = item.path();
        if path.is_dir() {
            let name = item.file_name().to_string_lossy().into_owned();
            collect(&path, Some(top.unwrap_or(&name)), found)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| {
                    HarnessError::InvalidCase(format!("{} has no usable case id", path.display()))
                })?
                .to_owned();
            found.push((id, top.unwrap_or_default().to_owned(), path));
        }
    }
    Ok(())
}
