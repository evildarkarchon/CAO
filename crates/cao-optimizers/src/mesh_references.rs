//! Mesh Reference Maintenance (#506): when TGA Textures are converted to DDS,
//! every Mesh's referenced TGA names are rewritten to DDS, so no Mesh points at
//! a deleted TGA, and the run's failed conversions are kept to withhold the
//! rewrites that would name a DDS that was never written.
//!
//! Texture paths are NIF strings in no particular encoding, so the rewrite
//! works on their bytes:
//!
//! - A name is rewritten only when it ends in `.tga`, in any ASCII case, and
//!   only that suffix becomes `.dds`; a `.tga` elsewhere in a name, as in
//!   `a.tga.dds`, is left alone (audit BE-13).
//! - Every Mesh is rewritten, whether or not the TGA it names was in the Mod
//!   Root, since a Mesh may name a Texture another mod provides. The only
//!   reference withheld is one naming a Texture whose own conversion failed
//!   earlier in the run, because only then is the DDS the rewrite would name
//!   known to be absent.

use std::path::Path;

use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{AssetOperation, OptimizerTarget, RoutedAsset};
use nifly_sys::{Nif, NifError};

use crate::plugins::decode_game_text;

const TGA_EXTENSION: &[u8] = b".tga";
const DDS_EXTENSION: &[u8] = b".dds";

/// Whether `value` ends in `.tga`, in any ASCII case.
fn ends_with_tga(value: &[u8]) -> bool {
    value
        .len()
        .checked_sub(TGA_EXTENSION.len())
        .is_some_and(|start| value[start..].eq_ignore_ascii_case(TGA_EXTENSION))
}

/// `value`, which ends in `.tga`, with that suffix replaced by `.dds`. Both
/// are four bytes, so the rest of the name is unchanged.
fn replace_tga(value: &[u8]) -> Vec<u8> {
    let mut replaced = value.to_vec();
    let start = replaced.len() - TGA_EXTENSION.len();
    replaced[start..].copy_from_slice(DDS_EXTENSION);
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
        ends_with_tga(reference) && is_eligible(reference)
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
        if !ends_with_tga(reference) || !is_eligible(reference) {
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
/// carry Windows separators whatever the host.
fn normalize_asset_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Folds a Mesh's Texture reference as [`normalize_asset_path`] does, after
/// decoding it as plugin text is decoded ([`decode_game_text`]), and drops
/// what the game ignores before it resolves the reference against its Data
/// directory: leading separators and a leading `data/` (audit BE-13).
fn normalize_reference(reference: &[u8]) -> String {
    let folded = normalize_asset_path(&decode_game_text(reference));
    let relative = folded.trim_start_matches('/');
    relative
        .strip_prefix("data/")
        .map_or(relative, |within| within.trim_start_matches('/'))
        .to_owned()
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
/// It lives for the whole run, in the backend. The Run Executor attempts
/// every Texture before any Mesh, so the record is complete by the time a
/// Mesh consults it.
#[derive(Debug, Clone, Default)]
pub struct FailedTextureConversions {
    paths: Vec<String>,
}

impl FailedTextureConversions {
    /// Records `asset` if it is a Texture conversion whose attempt failed.
    ///
    /// A failure to remove the TGA after the DDS committed safely is exempt:
    /// the DDS exists, so references to it must still be rewritten even
    /// though the TGA remains. Dry Run failures are recorded too, so a Dry
    /// Run reports the rewrites Apply would make.
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
        let reference = normalize_reference(reference);
        let mesh_path = normalize_asset_path(&mesh_path.to_string_lossy());
        self.paths
            .iter()
            .any(|failed| names_same_texture(failed, &reference, &mesh_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit BE-13: only a trailing `.tga`, in any case, is rewritten, and
    /// only it; a `.tga` inside a name is left alone.
    #[test]
    fn only_a_trailing_tga_is_rewritten() {
        assert!(ends_with_tga(b"textures\\armor\\body.TGA"));
        assert_eq!(replace_tga(b"textures\\body.Tga"), b"textures\\body.dds");
        assert!(!ends_with_tga(b"textures\\a.tga.dds"));
        assert!(!ends_with_tga(b"textures\\a.tga_n.dds"));
        assert!(!ends_with_tga(b"tga"));
    }

    /// Audit BE-13: a reference spelled with a leading separator or a
    /// leading `data\` names the same Texture as the plain one, so a failed
    /// conversion withholds its rewrite too.
    #[test]
    fn a_reference_with_a_leading_separator_or_data_names_the_same_texture() {
        let failed = normalize_asset_path(r"C:\Mods\A\textures\body.tga");
        let mesh = normalize_asset_path(r"C:\Mods\A\meshes\body.nif");
        for reference in [
            &br"textures\body.tga"[..],
            br"\textures\body.tga",
            br"Data\Textures\Body.tga",
            br"\data\textures\body.tga",
            b"data/textures/body.tga",
        ] {
            let normalized = normalize_reference(reference);
            assert!(
                names_same_texture(&failed, &normalized, &mesh),
                "{}",
                String::from_utf8_lossy(reference)
            );
        }
        // `data` only as a whole first folder.
        assert!(!names_same_texture(
            &failed,
            &normalize_reference(br"database\textures\body.tga"),
            &mesh
        ));
    }

    /// Audit BE-13: a reference in the games' ANSI code page names the
    /// Texture whose file name has the same letters.
    #[test]
    fn a_windows_1252_reference_names_its_texture() {
        let failed = normalize_asset_path("C:/Mods/A/textures/café.tga");
        let mesh = normalize_asset_path("C:/Mods/A/meshes/café.nif");
        let reference = b"textures\\caf\xE9.tga";
        assert!(names_same_texture(
            &failed,
            &normalize_reference(reference),
            &mesh
        ));
    }
}
