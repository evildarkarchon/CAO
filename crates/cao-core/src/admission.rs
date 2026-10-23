//! Admission Rules: the one statement of which of the user's choices a run's
//! Run Request may carry.
//!
//! A run's Mod Selection and, when known, its Profile Capabilities each hold
//! back some choices:
//!
//! - a Several Mods selection holds back Mesh levels above Necessary;
//! - an unsupported Profile Capability holds back the requested work that
//!   needs it.
//!
//! The execution mode holds back nothing: a Dry Run requests the work the
//! user chose, Archive work included, and its work phases mutate no Archive.
//!
//! A held-back choice is kept as the user made it: the rules never erase it,
//! they only leave it out of what the request carries. It is neither a Policy
//! Conflict nor a Start Error, and raises no Run Diagnostic.

use crate::routing::{ProfileCapabilities, RequestedWork, work_capabilities};
use crate::run::{MeshLevel, WorkChoices};

/// The Admission Rules of one run, answering which requested work and Work
/// Choices its Run Request may carry.
///
/// It says what is admitted, never which rule held a choice back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admission {
    /// The admitted [`RequestedWork`], one bit each ([`work_bit`]). Every
    /// bit starts set, so work no rule holds back is admitted.
    work: u8,
    /// The highest Mesh level the request may carry.
    highest_mesh_level: MeshLevel,
}

impl Admission {
    /// The Admission Rules of a run whose Mod Selection is Several Mods when
    /// `several_mods` is true.
    ///
    /// `capabilities` are the Selected Profile's when the caller knows them;
    /// `None` holds back nothing for capabilities, leaving the Profile to be
    /// judged when the Routing Policy is compiled at Preparing.
    pub fn new(several_mods: bool, capabilities: Option<&ProfileCapabilities>) -> Self {
        let mut work = u8::MAX;
        if let Some(capabilities) = capabilities {
            for (requested, capability) in work_capabilities() {
                if !capabilities.supports(capability) {
                    work &= !work_bit(requested);
                }
            }
        }
        let highest_mesh_level = if several_mods {
            MeshLevel::NECESSARY
        } else {
            MeshLevel::FULL
        };
        Self {
            work,
            highest_mesh_level,
        }
    }

    /// Reports whether the run's Run Request may carry `work`.
    pub fn admits(&self, work: RequestedWork) -> bool {
        self.work & work_bit(work) != 0
    }

    /// The highest Mesh level the run's Run Request may carry: Necessary
    /// under Several Mods, Full otherwise.
    pub fn highest_mesh_level(&self) -> MeshLevel {
        self.highest_mesh_level
    }

    /// The part of the user's chosen `work` the Run Request may carry, in
    /// the order given.
    pub fn admitted_work(&self, work: &[RequestedWork]) -> Vec<RequestedWork> {
        work.iter()
            .copied()
            .filter(|work| self.admits(*work))
            .collect()
    }

    /// The user's `choices` as the Run Request may carry them: the Mesh
    /// level is capped at [`Self::highest_mesh_level`], and every other
    /// choice is kept as chosen.
    ///
    /// No choice is dropped for held-back work, since a choice for work that
    /// is not requested has no effect. Resaving is kept too, even under
    /// Several Mods: it requests Mesh work by itself, at any level.
    pub fn admitted_work_choices(&self, mut choices: WorkChoices) -> WorkChoices {
        choices.meshes.level = choices.meshes.level.min(self.highest_mesh_level);
        choices
    }
}

/// The bit of [`Admission`]'s set that stands for `work`.
///
/// The match has no wildcard arm, so a new [`RequestedWork`] does not compile
/// until it is given a bit here; a ninth bit overflows `u8`, which the
/// `arithmetic_overflow` lint also rejects at compile time. Indexing by
/// discriminant would instead compile and panic at run time.
const fn work_bit(work: RequestedWork) -> u8 {
    match work {
        RequestedWork::NativeTextureOptimization => 1 << 0,
        RequestedWork::StandardMeshOptimization => 1 << 1,
        RequestedWork::TerrainMeshOptimization => 1 << 2,
        RequestedWork::AnimationOptimization => 1 << 3,
        RequestedWork::ArchiveExtraction => 1 << 4,
        RequestedWork::ArchiveCreation => 1 << 5,
    }
}
