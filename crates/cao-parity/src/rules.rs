//! The comparator rules every parity case uses (#467).
//!
//! [`ParityRules`] fills the output-tree comparator's two hooks with the rules
//! landed so far: the leftovers rules of [`crate::leftovers`] (#491), the
//! Texture rule of [`crate::textures`] (#494) and the Archive rule of
//! [`crate::archives`] (#497). Meshes (#504), Animations (#502) and Loading
//! Plugins (#498) have no rule, so their files must be byte-identical, which is
//! their final rule. For Meshes that holds even though the oracle builds nifly
//! with vcpkg and CMake and `nifly-sys` with `cc`: if nifly's float output
//! drifts between the two, the builds' compiler flags are aligned rather than
//! the rule loosened. A structural comparator with ulp tolerance is only the
//! recorded fallback, and needs the maintainer's agreement.

use crate::archives::ArchiveRule;
use crate::leftovers::{ManifestRule, PresenceRule};
use crate::textures::TextureRule;
use crate::tree::{ArtifactRule, TreeRules};

/// The comparator rules every parity case uses.
pub struct ParityRules;

impl TreeRules for ParityRules {
    fn leftover_rule(&self, path: &str) -> Option<&dyn ArtifactRule> {
        let in_staging = |name: &str| {
            path == format!(".cao-staging/{name}")
                || path.ends_with(&format!("/.cao-staging/{name}"))
        };
        if in_staging("ownership.manifest") {
            Some(&ManifestRule)
        } else if in_staging("owner.lock") {
            Some(&PresenceRule)
        } else {
            None
        }
    }

    /// Matched by extension, ignoring ASCII case as Asset Routing does:
    /// `.dds` files follow the Texture rule, and `.bsa` and `.ba2` files the
    /// Archive rule. `.caobad` and `.bak` files never match, so they stay
    /// byte-identical: they are renamed originals.
    fn asset_rule(&self, path: &str) -> Option<&dyn ArtifactRule> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let has_extension = |extension: &str| {
            name.len() > extension.len()
                && name.is_char_boundary(name.len() - extension.len())
                && name[name.len() - extension.len()..].eq_ignore_ascii_case(extension)
        };
        if has_extension(".dds") {
            Some(&TextureRule)
        } else if has_extension(".bsa") || has_extension(".ba2") {
            Some(&ArchiveRule)
        } else {
            None
        }
    }
}
