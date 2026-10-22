//! A Profile's Mesh Target: the NIF file, user and stream version it writes
//! Meshes as. Each axis is a closed set, validated on its own; combinations are
//! not checked.
//!
//! The numbers are the ones `profile.ini` has always held, which are nifly's.
//! This crate does not depend on nifly: the composition root maps each member to
//! its `NifVersion` part.

use crate::closed::ClosedSet;

/// The NIF file version Meshes are written as (`[Meshes] meshesFileVersion`),
/// stored as nifly's `NiFileVersion` number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeshFileVersion {
    /// 20.0.0.5 (Oblivion), stored as `335544325` (`0x14000005`).
    V20_0_0_5,
    /// 20.2.0.7 (Fallout 3 through Fallout 4), stored as `335675399`
    /// (`0x14020007`).
    V20_2_0_7,
}

impl ClosedSet for MeshFileVersion {
    const ALL: &'static [Self] = &[Self::V20_0_0_5, Self::V20_2_0_7];

    fn number(self) -> u32 {
        match self {
            Self::V20_0_0_5 => 0x1400_0005,
            Self::V20_2_0_7 => 0x1402_0007,
        }
    }

    /// The stored number with its dotted version, since `335675399` alone does
    /// not say which version it is.
    fn describe(self) -> String {
        format!("{} ({})", self.number(), self.dotted())
    }
}

impl MeshFileVersion {
    /// The version as NIF headers spell it, such as `20.2.0.7`.
    pub fn dotted(self) -> &'static str {
        match self {
            Self::V20_0_0_5 => "20.0.0.5",
            Self::V20_2_0_7 => "20.2.0.7",
        }
    }
}

/// The NIF user version Meshes are written with (`[Meshes] meshesUser`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeshUserVersion {
    /// 11 (Oblivion).
    V11,
    /// 12 (Fallout 3 through Fallout 4).
    V12,
}

impl ClosedSet for MeshUserVersion {
    const ALL: &'static [Self] = &[Self::V11, Self::V12];

    fn number(self) -> u32 {
        match self {
            Self::V11 => 11,
            Self::V12 => 12,
        }
    }
}

/// The NIF stream version Meshes are written with (`[Meshes] meshesStream`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeshStreamVersion {
    /// 82 (Oblivion and Fallout 3).
    V82,
    /// 83 (Skyrim).
    V83,
    /// 100 (Skyrim Special Edition).
    V100,
    /// 130 (Fallout 4).
    V130,
}

impl ClosedSet for MeshStreamVersion {
    const ALL: &'static [Self] = &[Self::V82, Self::V83, Self::V100, Self::V130];

    fn number(self) -> u32 {
        match self {
            Self::V82 => 82,
            Self::V83 => 83,
            Self::V100 => 100,
            Self::V130 => 130,
        }
    }
}
