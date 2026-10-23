//! Admission Rules: the one statement of which of the user's choices a run's
//! Run Request may carry.
//!
//! A run's execution mode, its Mod Selection and, when known, its Profile
//! Capabilities each hold back some choices:
//!
//! - a Dry Run holds back Archive extraction and Archive creation;
//! - a Several Mods selection holds back Mesh levels above Necessary;
//! - an unsupported Profile Capability holds back the requested work that
//!   needs it.
//!
//! A held-back choice is kept as the user made it: the rules never erase it,
//! they only leave it out of what the request carries. It is neither a Policy
//! Conflict nor a Start Error, and raises no Run Diagnostic.

use crate::routing::{ExecutionMode, ProfileCapabilities, RequestedWork, work_capabilities};
use crate::run::{MeshLevel, WorkChoices};

/// The Admission Rules of one run, answering which requested work and Work
/// Choices its Run Request may carry.
///
/// It says what is admitted, never which rule held a choice back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admission {
    /// Whether each [`RequestedWork`], indexed by its discriminant, is
    /// admitted.
    work: [bool; 6],
    /// The highest Mesh level the request may carry.
    highest_mesh_level: MeshLevel,
}

impl Admission {
    /// The Admission Rules of a run in `execution_mode`, whose Mod Selection
    /// is Several Mods when `several_mods` is true.
    ///
    /// `capabilities` are the Selected Profile's when the caller knows them;
    /// `None` holds back nothing for capabilities, leaving the Profile to be
    /// judged when the Routing Policy is compiled at Preparing.
    pub fn new(
        execution_mode: ExecutionMode,
        several_mods: bool,
        capabilities: Option<&ProfileCapabilities>,
    ) -> Self {
        let mut work = [true; 6];
        if execution_mode == ExecutionMode::DryRun {
            work[RequestedWork::ArchiveExtraction as usize] = false;
            work[RequestedWork::ArchiveCreation as usize] = false;
        }
        if let Some(capabilities) = capabilities {
            for (requested, capability) in work_capabilities() {
                if !capabilities.supports(capability) {
                    work[requested as usize] = false;
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
        self.work[work as usize]
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
