//! Committed seed cases, which `cao-parity case <id>` can materialise by name.
//!
//! A seed is a whole `case.json` (spec, profile overrides and tree recipe)
//! committed under `crates/cao-parity/seeds/`, in any subfolder. Its case id is
//! its file stem, so ids must be unique across the folder. The hand-written
//! tracer and Several Mods cases live in `seeds/hand-written/`; transcribed C++
//! scenarios join them (#507).
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

/// Every seed, by case id, sorted by id.
///
/// # Errors
/// [`HarnessError::InvalidCase`] when two seeds share an id or a seed is not a
/// valid `case.json`; [`HarnessError::Io`] when the folder cannot be read.
pub fn seeds() -> Result<Vec<(String, CaseFile)>, HarnessError> {
    let mut found = Vec::new();
    collect(&seeds_dir(), &mut found)?;
    // Ids are compared ignoring ASCII case, as Windows compares the case
    // folders they name: `Foo` and `foo` would share one work directory.
    found.sort_by_key(|(id, _)| id.to_ascii_lowercase());
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
        .map(|(id, path)| {
            let case = serde_json::from_slice(&read_file(&path)?)
                .map_err(|source| HarnessError::Json { path, source })?;
            Ok((id, case))
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

/// Collects every `*.json` beneath `directory` with its file stem.
fn collect(directory: &Path, found: &mut Vec<(String, PathBuf)>) -> Result<(), HarnessError> {
    for item in list_dir(directory)? {
        let path = item.path();
        if path.is_dir() {
            collect(&path, found)?;
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
            found.push((id, path));
        }
    }
    Ok(())
}
