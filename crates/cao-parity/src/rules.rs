//! The comparator rules every parity case uses (#467).
//!
//! [`ParityRules`] fills the output-tree comparator's two hooks with the rules
//! landed so far: the leftovers rules of [`crate::leftovers`] (#491) and the
//! Texture rule of [`crate::textures`] (#494). Meshes, Animations, Archives
//! and Loading Plugins have no rule yet, so their files must be
//! byte-identical, which is also their final rule for all but Archives.

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

    /// `.dds` files, matched by extension ignoring ASCII case as Asset
    /// Routing does, follow the Texture rule. `.caobad` and `.bak` files never
    /// match, so they stay byte-identical.
    fn asset_rule(&self, path: &str) -> Option<&dyn ArtifactRule> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let is_dds = name.len() > 4
            && name.is_char_boundary(name.len() - 4)
            && name[name.len() - 4..].eq_ignore_ascii_case(".dds");
        is_dds.then_some(&TextureRule as &dyn ArtifactRule)
    }
}
