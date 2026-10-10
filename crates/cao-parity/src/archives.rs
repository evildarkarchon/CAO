//! The Archive rule of the output-tree comparator (#467, #497).
//!
//! Two Archives whose bytes differ are Equivalent when they parse alike:
//!
//! - the same header: format, version, archive flags and archive types, the
//!   FO4 container kind (`GNRL` or `DX10`), and whether a name table is
//!   present;
//! - the same entry names, compared case-sensitively;
//! - per entry, the same compression state, chunk count and DX10 mip ranges;
//! - per entry, decompressed content that passes its own Asset Kind's rule:
//!   the Texture rule for a `.dds` entry, applied to the DDS a DX10 entry
//!   rebuilds, and byte equality for every other entry.
//!
//! Compressed bytes, offsets and entry order are never compared: zlib and
//! LZ4 builds may compress differently, and entry order follows the format's
//! name hashes rather than anything CAO decides.

use std::collections::BTreeMap;
use std::path::Path;

use cao_archive::{ArchivedAsset, ReadArchive};

use crate::HarnessError;
use crate::textures::compare_dds;
use crate::tree::{ArtifactRule, RuleOutcome};

/// The comparator rule for `.bsa` and `.ba2` files.
pub struct ArchiveRule;

impl ArtifactRule for ArchiveRule {
    fn name(&self) -> &'static str {
        "Archive Structure and Entries"
    }

    fn compare(&self, oracle: &Path, rust: &Path) -> Result<RuleOutcome, HarnessError> {
        compare_archives(oracle, rust)
    }
}

/// Compares two Archive files under the Archive rule, reporting every
/// difference found.
///
/// An Archive that does not parse on either side is a difference, not a
/// harness error: both builds wrote it. Both Archives stay mapped only for
/// the length of the call.
///
/// # Errors
/// Never in practice; the signature matches [`ArtifactRule::compare`].
pub fn compare_archives(oracle: &Path, rust: &Path) -> Result<RuleOutcome, HarnessError> {
    let (oracle, rust) = match (open(oracle), open(rust)) {
        (Ok(oracle), Ok(rust)) => (oracle, rust),
        (oracle, rust) => {
            let problems: Vec<String> = [("oracle", oracle.err()), ("Rust", rust.err())]
                .into_iter()
                .filter_map(|(side, error)| error.map(|error| format!("the {side} {error}")))
                .collect();
            return Ok(RuleOutcome::Different(problems.join("; ")));
        }
    };
    let mut differences = Vec::new();
    let (oracle_header, rust_header) = (oracle.header(), rust.header());
    if oracle_header != rust_header {
        differences.push(format!(
            "header {oracle_header:?} on the oracle side, {rust_header:?} on the Rust side"
        ));
    }
    let (oracle_entries, rust_entries) = match (entries(&oracle), entries(&rust)) {
        (Ok(oracle), Ok(rust)) => (oracle, rust),
        (oracle, rust) => {
            differences.extend(oracle.err().map(|error| format!("the oracle {error}")));
            differences.extend(rust.err().map(|error| format!("the Rust {error}")));
            return Ok(RuleOutcome::Different(differences.join("; ")));
        }
    };
    for (name, oracle_entry) in &oracle_entries {
        let Some(rust_entry) = rust_entries.get(name) else {
            differences.push(format!("entry `{name}` is only on the oracle side"));
            continue;
        };
        differences.extend(compare_entry(
            name,
            oracle_entry,
            rust_entry,
            &oracle,
            &rust,
        ));
    }
    differences.extend(
        rust_entries
            .keys()
            .filter(|name| !oracle_entries.contains_key(*name))
            .map(|name| format!("entry `{name}` is only on the Rust side")),
    );
    Ok(if differences.is_empty() {
        RuleOutcome::Equivalent
    } else {
        RuleOutcome::Different(differences.join("; "))
    })
}

/// Opens an Archive, describing why one cannot be read.
fn open(path: &Path) -> Result<ReadArchive, String> {
    match ReadArchive::open(path) {
        Ok(Some(archive)) => Ok(archive),
        Ok(None) => Err("Archive has no known magic".to_owned()),
        Err(error) => Err(format!("Archive does not parse: {error}")),
    }
}

/// An Archive's entries by name, so the two sides pair up whatever their
/// stored order.
fn entries(archive: &ReadArchive) -> Result<BTreeMap<String, ArchivedAsset>, String> {
    let assets = archive
        .archived_assets()
        .map_err(|error| format!("Archive does not list: {error}"))?;
    Ok(assets
        .into_iter()
        .map(|asset| (asset.name.clone(), asset))
        .collect())
}

/// Every difference between one entry present on both sides.
fn compare_entry(
    name: &str,
    oracle_entry: &ArchivedAsset,
    rust_entry: &ArchivedAsset,
    oracle: &ReadArchive,
    rust: &ReadArchive,
) -> Vec<String> {
    let mut differences = Vec::new();
    if oracle_entry.compressed != rust_entry.compressed {
        differences.push(format!(
            "entry `{name}` is {} on the oracle side and {} on the Rust side",
            compression(oracle_entry),
            compression(rust_entry)
        ));
    }
    if (oracle_entry.chunks, &oracle_entry.mip_ranges)
        != (rust_entry.chunks, &rust_entry.mip_ranges)
    {
        differences.push(format!(
            "entry `{name}` has {} chunks with mips {:?} on the oracle side, {} with {:?} on the \
             Rust side",
            oracle_entry.chunks, oracle_entry.mip_ranges, rust_entry.chunks, rust_entry.mip_ranges
        ));
    }
    let extract = |archive: &ReadArchive| {
        let mut bytes = Vec::new();
        archive
            .extract(name, &mut bytes)
            .map(|()| bytes)
            .map_err(|error| error.to_string())
    };
    match (extract(oracle), extract(rust)) {
        (Ok(oracle_bytes), Ok(rust_bytes)) => {
            if oracle_bytes != rust_bytes {
                if is_dds(name) {
                    if let RuleOutcome::Different(detail) = compare_dds(&oracle_bytes, &rust_bytes)
                    {
                        differences.push(format!("entry `{name}`: {detail}"));
                    }
                } else {
                    differences.push(format!("entry `{name}` decompresses to different bytes"));
                }
            }
        }
        (oracle_bytes, rust_bytes) => {
            for (side, error) in [("oracle", oracle_bytes.err()), ("Rust", rust_bytes.err())] {
                if let Some(error) = error {
                    differences.push(format!(
                        "entry `{name}` does not extract on the {side} side: {error}"
                    ));
                }
            }
        }
    }
    differences
}

fn compression(entry: &ArchivedAsset) -> &'static str {
    if entry.compressed {
        "compressed"
    } else {
        "stored"
    }
}

/// Whether an entry name ends in `.dds`, ignoring ASCII case, as Asset Routing
/// matches extensions.
fn is_dds(name: &str) -> bool {
    name.len() > 4
        && name.is_char_boundary(name.len() - 4)
        && name[name.len() - 4..].eq_ignore_ascii_case(".dds")
}
