//! The Work Choices a Run Request carries: the user's choices for how one
//! Optimization Run does its requested work.
//!
//! They are core-owned, domain-level values. No DXGI format, NIF version or
//! other backend type appears here; the composition root turns them into its
//! backend's settings. Each constructor enforces its own invariant and returns
//! a [`WorkChoiceError`] rather than panicking, so no code downstream has to
//! recheck one.
//!
//! Work Choices never request work or decide routing: requested work alone
//! does. Choices for work that is not requested are carried and have no
//! effect.

use super::{ArchiveFinalizationChoices, SourceCleanup};

/// The user's choices for how one Optimization Run does its requested work,
/// carried by its Run Request.
///
/// Each field holds a value whose own constructor enforced its invariants,
/// so the fields are public and any combination is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkChoices {
    /// How Textures are optimized and resized.
    pub textures: TextureChoices,
    /// How Meshes are optimized.
    pub meshes: MeshChoices,
    /// What becomes of each source Archive once it is extracted: backed up
    /// or removed.
    pub archive_extraction: SourceCleanup,
    /// How Archive Finalization packs and maintains Loading Plugins.
    pub archive_finalization: ArchiveFinalizationChoices,
}

impl Default for WorkChoices {
    /// The options model's default form: necessary Texture work only, Mesh
    /// level 0 with headparts on, extracted Archives backed up, and Archive
    /// Finalization's default packing choices.
    ///
    /// Core cannot see the options model, so `cao-optimizers`' Run Request
    /// tests pin this default to it.
    fn default() -> Self {
        Self {
            textures: TextureChoices::default(),
            meshes: MeshChoices::default(),
            archive_extraction: SourceCleanup::Backup,
            archive_finalization: ArchiveFinalizationChoices::default(),
        }
    }
}

/// How a run optimizes and resizes its Textures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureChoices {
    /// Fix what a Texture needs to load in the game.
    pub necessary: bool,
    /// Compress uncompressed Textures.
    pub compress: bool,
    /// Generate missing mipmaps.
    pub mipmaps: bool,
    /// Whether and how Textures are resized.
    pub resize: TextureResizeChoice,
}

impl Default for TextureChoices {
    /// The options model's defaults: necessary work only, no resize.
    fn default() -> Self {
        Self {
            necessary: true,
            compress: false,
            mipmaps: false,
            resize: TextureResizeChoice::None,
        }
    }
}

/// Whether and how a run resizes its Textures: one mode at most.
///
/// When the options form turns on both modes, its adapter picks one; this
/// choice never holds both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextureResizeChoice {
    /// Keep every Texture's size.
    #[default]
    None,
    /// Divide each side by a ratio.
    Ratio(ResizeRatio),
    /// Halve each Texture towards a size.
    Size(ResizeSize),
}

/// How a run optimizes its Meshes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshChoices {
    /// How hard Meshes are optimized.
    pub level: MeshLevel,
    /// Optimize Headpart Meshes as head parts.
    pub headparts: bool,
    /// Save every loaded Mesh again, whatever the level.
    pub resave: bool,
}

impl Default for MeshChoices {
    /// The options model's defaults: level 0, headparts on, no resave.
    fn default() -> Self {
        Self {
            level: MeshLevel::default(),
            headparts: true,
            resave: false,
        }
    }
}

/// Why a Work Choice cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkChoiceError {
    /// A Mesh level outside 0 to 3.
    #[error("Mesh optimization level must be between 0 and 3.")]
    MeshLevel(i32),
    /// A Texture resize ratio of zero.
    #[error("Texture resizing requires non-zero width and height ratios.")]
    ZeroResizeRatio,
    /// A Texture resize size with an odd side.
    #[error("Texture target width and height must be even.")]
    OddResizeSize,
    /// A Texture resize size with a zero side.
    #[error("Texture resizing requires non-zero width and height.")]
    ZeroResizeSize,
}

/// How hard a run optimizes its Meshes: 0 (off), 1 (necessary), 2 (medium)
/// or 3 (full).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MeshLevel(u8);

impl MeshLevel {
    /// Level 1: only what a Mesh needs to load in the game. A Several Mods
    /// selection optimizes Meshes at this level at most.
    pub const NECESSARY: Self = Self(1);

    /// Level 3, the highest.
    pub const FULL: Self = Self(3);

    /// The Mesh level `level`.
    ///
    /// # Errors
    /// [`WorkChoiceError::MeshLevel`] when `level` is outside 0 to 3.
    pub fn new(level: i32) -> Result<Self, WorkChoiceError> {
        match u8::try_from(level) {
            Ok(level @ 0..=3) => Ok(Self(level)),
            _ => Err(WorkChoiceError::MeshLevel(level)),
        }
    }

    /// The level, within 0 to 3.
    pub fn get(self) -> u8 {
        self.0
    }
}

/// The ratios a Texture's width and height are each divided by when resizing
/// by ratio. Neither is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeRatio {
    width: u32,
    height: u32,
}

impl ResizeRatio {
    /// Divides a Texture's width by `width` and its height by `height`.
    ///
    /// # Errors
    /// [`WorkChoiceError::ZeroResizeRatio`] when either ratio is zero.
    pub fn new(width: u32, height: u32) -> Result<Self, WorkChoiceError> {
        if width == 0 || height == 0 {
            return Err(WorkChoiceError::ZeroResizeRatio);
        }
        Ok(Self { width, height })
    }

    /// The width ratio, never zero.
    pub fn width(self) -> u32 {
        self.width
    }

    /// The height ratio, never zero.
    pub fn height(self) -> u32 {
        self.height
    }
}

/// The size a Texture is halved towards when resizing by size. Neither side
/// is zero or odd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeSize {
    width: u32,
    height: u32,
}

impl ResizeSize {
    /// Halves a Texture towards `width` by `height`.
    ///
    /// # Errors
    /// [`WorkChoiceError::OddResizeSize`] when either side is odd, checked
    /// first, then [`WorkChoiceError::ZeroResizeSize`] when either side is
    /// zero.
    pub fn new(width: u32, height: u32) -> Result<Self, WorkChoiceError> {
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(WorkChoiceError::OddResizeSize);
        }
        if width == 0 || height == 0 {
            return Err(WorkChoiceError::ZeroResizeSize);
        }
        Ok(Self { width, height })
    }

    /// The target width, never zero or odd.
    pub fn width(self) -> u32 {
        self.width
    }

    /// The target height, never zero or odd.
    pub fn height(self) -> u32 {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{ExecutionMode, RequestedWork};
    use crate::run::{ModSelection, RunRequest};

    fn request() -> RunRequest {
        RunRequest::new(
            "SSE",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot("C:\\mods\\Mod".into()),
            vec![RequestedWork::NativeTextureOptimization],
        )
    }

    #[test]
    fn a_new_run_request_carries_the_options_model_defaults() {
        // The options model's defaults, stated independently of the code.
        let choices = *request().work_choices();
        assert!(choices.textures.necessary);
        assert!(!choices.textures.compress);
        assert!(!choices.textures.mipmaps);
        assert_eq!(choices.textures.resize, TextureResizeChoice::None);
        assert_eq!(choices.meshes.level.get(), 0);
        assert!(choices.meshes.headparts);
        assert!(!choices.meshes.resave);
        assert_eq!(choices.archive_extraction, SourceCleanup::Backup);
        assert_eq!(
            choices.archive_finalization,
            ArchiveFinalizationChoices {
                compress: true,
                delete_sources: true,
                create_dummy_plugins: true,
                merge_incompressible: true,
                merge_textures: false,
            }
        );
        assert_eq!(choices, WorkChoices::default());
    }

    #[test]
    fn the_builder_replaces_the_run_requests_work_choices() {
        let choices = WorkChoices {
            textures: TextureChoices {
                necessary: false,
                compress: true,
                mipmaps: true,
                resize: TextureResizeChoice::Size(ResizeSize::new(1024, 512).unwrap()),
            },
            meshes: MeshChoices {
                level: MeshLevel::new(3).unwrap(),
                headparts: false,
                resave: true,
            },
            archive_extraction: SourceCleanup::Remove,
            archive_finalization: ArchiveFinalizationChoices {
                compress: false,
                delete_sources: false,
                create_dummy_plugins: false,
                merge_incompressible: false,
                merge_textures: true,
            },
        };

        let request = request().with_work_choices(choices);

        assert_eq!(request.work_choices(), &choices);
        // The builder leaves the rest of the request alone.
        assert_eq!(request.profile_identity(), "SSE");
        assert_eq!(
            request.requested_work(),
            [RequestedWork::NativeTextureOptimization]
        );
    }

    #[test]
    fn non_zero_even_resize_sizes_are_accepted() {
        let size = ResizeSize::new(1024, 512).unwrap();
        assert_eq!((size.width(), size.height()), (1024, 512));
    }

    #[test]
    fn a_zero_resize_size_is_rejected() {
        for (width, height) in [(0, 512), (512, 0), (0, 0)] {
            assert_eq!(
                ResizeSize::new(width, height),
                Err(WorkChoiceError::ZeroResizeSize),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn an_odd_resize_size_is_rejected() {
        for (width, height) in [(513, 512), (512, 513), (1, 1)] {
            assert_eq!(
                ResizeSize::new(width, height),
                Err(WorkChoiceError::OddResizeSize),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn non_zero_resize_ratios_are_accepted() {
        let ratio = ResizeRatio::new(2, 4).unwrap();
        assert_eq!((ratio.width(), ratio.height()), (2, 4));
    }

    #[test]
    fn a_zero_resize_ratio_is_rejected() {
        for (width, height) in [(0, 1), (1, 0), (0, 0)] {
            assert_eq!(
                ResizeRatio::new(width, height),
                Err(WorkChoiceError::ZeroResizeRatio),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn mesh_levels_zero_to_three_are_accepted() {
        for level in 0..=3 {
            assert_eq!(MeshLevel::new(level).map(MeshLevel::get), Ok(level as u8));
        }
    }

    #[test]
    fn a_mesh_level_above_three_or_below_zero_is_rejected() {
        for level in [4, -1, i32::MIN] {
            assert_eq!(
                MeshLevel::new(level),
                Err(WorkChoiceError::MeshLevel(level))
            );
        }
    }
}
