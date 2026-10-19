//! Mesh Reference Maintenance (#506): when TGA Textures are converted to DDS,
//! every Mesh's referenced TGA names are rewritten to DDS, so no Mesh points at
//! a deleted TGA.
//!
//! Ported from C++ `MeshReferenceMaintenance` and the failed-conversion
//! bookkeeping of `MainOptimizer`. Texture paths are NIF strings in no
//! particular encoding, so the rewrite works on their bytes.
//!
//! Ported as-is from C++:
//!
//! - `.tga` matches case-insensitively (ASCII only) *anywhere* in a name, not
//!   only as its suffix, and every occurrence becomes `.dds`.
//! - Every Mesh is rewritten, whether or not the TGA it names was in the Mod
//!   Root. The only reference withheld is one naming a Texture whose own
//!   conversion failed earlier in the run, because only then is the DDS the
//!   rewrite would name known to be absent.

use std::path::Path;

use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{AssetOperation, OptimizerTarget, RoutedAsset};
use nifly_sys::{Nif, NifError};

const TGA_EXTENSION: &[u8] = b".tga";
const DDS_EXTENSION: &[u8] = b".dds";

/// Whether `value` holds `.tga`, in any ASCII case, at `position`.
fn matches_tga_at(value: &[u8], position: usize) -> bool {
    value
        .get(position..position + TGA_EXTENSION.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(TGA_EXTENSION))
}

/// Whether `value` holds `.tga`, in any ASCII case, anywhere.
fn contains_tga(value: &[u8]) -> bool {
    (0..value.len()).any(|position| matches_tga_at(value, position))
}

/// `value` with every `.tga`, in any ASCII case, replaced by `.dds`. Both are
/// four bytes, so the rewrite never moves the rest of the name.
fn replace_tga(value: &[u8]) -> Vec<u8> {
    let mut replaced = value.to_vec();
    let mut position = 0;
    while position < replaced.len() {
        if matches_tga_at(&replaced, position) {
            replaced[position..position + DDS_EXTENSION.len()].copy_from_slice(DDS_EXTENSION);
            position += TGA_EXTENSION.len();
        } else {
            position += 1;
        }
    }
    replaced
}

/// Whether `nif` references a TGA Texture that `is_eligible` accepts.
///
/// `is_eligible` sees only names that would be rewritten, so a caller can
/// treat every rejection as one withheld rewrite. Detection stops at the first
/// eligible name.
///
/// # Errors
/// [`NifError::Exception`] when nifly throws while listing the paths.
pub fn has_referenced_tga_texture(
    nif: &mut Nif,
    mut is_eligible: impl FnMut(&[u8]) -> bool,
) -> Result<bool, NifError> {
    let textures = nif.texture_paths()?;
    Ok((0..textures.len()).any(|index| {
        let reference = textures.get(index).unwrap_or_default();
        contains_tga(reference) && is_eligible(reference)
    }))
}

/// Replaces `.tga` with `.dds` in every referenced name `is_eligible` accepts,
/// reporting whether the Mesh changed.
///
/// A texture set shared by several shapes is listed once per shape; after its
/// first rewrite the later entries no longer name a TGA, so `is_eligible` is
/// asked about each string once.
///
/// # Errors
/// [`NifError::Exception`] when nifly throws while listing or setting a path.
pub fn replace_referenced_tga_texture_names(
    nif: &mut Nif,
    mut is_eligible: impl FnMut(&[u8]) -> bool,
) -> Result<bool, NifError> {
    let mut textures = nif.texture_paths()?;
    let mut changed = false;
    for index in 0..textures.len() {
        let reference = textures.get(index).unwrap_or_default();
        if !contains_tga(reference) || !is_eligible(reference) {
            continue;
        }
        let replaced = replace_tga(reference);
        textures.set(index, &replaced)?;
        changed = true;
    }
    Ok(changed)
}

/// Folds an Asset path, or a Mesh's Texture reference, to the form the two are
/// compared in: both separators become `/` and case is discarded. References
/// carry Windows separators whatever the host. Callers decode non-UTF-8 bytes
/// lossily first, as C++'s `QString::fromStdString` decoded them.
fn normalize_asset_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Whether the normalized Texture execution path `execution_path` names the
/// very Asset the normalized Mesh reference `reference` points at, for a Mesh
/// whose normalized execution path is `mesh_path`.
///
/// References are relative to the game's Data directory while execution paths
/// are rooted in the scanned mod, so the reference must match a whole trailing
/// component sequence. What precedes it is the Data directory the reference
/// resolved against, and the Mesh must lie beneath it: Several Mods scans
/// sibling Mod Roots that often hold identically named Textures, so a suffix
/// match alone would let one mod's failure withhold another mod's rewrite.
fn names_same_texture(execution_path: &str, reference: &str, mesh_path: &str) -> bool {
    if reference.is_empty() || !execution_path.ends_with(reference) {
        return false;
    }
    let data_root_len = execution_path.len() - reference.len();
    if data_root_len == 0 {
        // The reference is the whole execution path, so both Assets share one
        // traversal origin and there is no containment left to check.
        return true;
    }
    // A valid UTF-8 suffix starts on a character boundary, so this byte is
    // the one before the match.
    if execution_path.as_bytes()[data_root_len - 1] != b'/' {
        return false;
    }
    let data_root = &execution_path[..data_root_len - 1];
    data_root.is_empty()
        || (mesh_path.len() > data_root.len()
            && mesh_path.starts_with(data_root)
            && mesh_path.as_bytes()[data_root.len()] == b'/')
}

/// The run's Texture conversions that failed without committing a usable DDS,
/// as normalized execution paths.
///
/// It lives for the whole run, like C++'s `MainOptimizer`. The Run Executor
/// attempts every Texture before any Mesh, so the record is complete by the
/// time a Mesh consults it.
#[derive(Debug, Clone, Default)]
pub struct FailedTextureConversions {
    paths: Vec<String>,
}

impl FailedTextureConversions {
    /// Records `asset` if it is a Texture conversion whose attempt failed.
    ///
    /// A failure to remove the TGA after the DDS committed safely is exempt:
    /// the DDS exists, so references to it must still be rewritten even
    /// though the TGA remains. Dry Run failures are recorded too, as in C++,
    /// so a Dry Run reports the rewrites Apply would make.
    pub fn record_attempt(&mut self, asset: &RoutedAsset, result: &AssetExecutionResult) {
        let committed_dds = result.failure() == Some(AssetExecutionFailure::SourceRemovalFailed)
            && result.mutation_state() == MutationState::Committed
            && result.safe_to_continue();
        if result.succeeded()
            || asset.target() != OptimizerTarget::Texture
            || !asset.operations().contains(AssetOperation::Conversion)
            || committed_dds
        {
            return;
        }
        self.paths.push(normalize_asset_path(
            &asset.execution_path().to_string_lossy(),
        ));
    }

    /// Whether the rewrite of `reference`, in the Mesh whose execution path is
    /// `mesh_path`, must be withheld because that Texture's conversion failed.
    pub fn withholds(&self, reference: &[u8], mesh_path: &Path) -> bool {
        if self.paths.is_empty() {
            return false;
        }
        let reference = normalize_asset_path(&String::from_utf8_lossy(reference));
        let mesh_path = normalize_asset_path(&mesh_path.to_string_lossy());
        self.paths
            .iter()
            .any(|failed| names_same_texture(failed, &reference, &mesh_path))
    }
}
