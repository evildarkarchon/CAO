//! The settings model of a profile's `profile.ini`: the game it targets and what
//! each Asset Kind may do under it.

use crate::archive_size::ArchiveSize;
use crate::closed::{self, ClosedSet};
use crate::error::ProfileError;
use crate::ini::{IniFile, Value};
use crate::mesh::{MeshFileVersion, MeshStreamVersion, MeshUserVersion};
use crate::texture::{TextureFormat, UnwantedFormat};

/// The game whose archive rules a profile uses (`[BSA] bsaGame`), numbered as
/// bethutil's `btu::Game` numbers them.
///
/// **Deviation 11:** only these three exist. C++ also accepted TES3 (0, which is also
/// what text or a missing key reads as), TES4 (1) and FNV (2), and silently used SSE's
/// rules for any other number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BsaGame {
    /// Skyrim Legendary Edition, `bsaGame=3`.
    Tes5,
    /// Skyrim Special Edition, `bsaGame=4`.
    Sse,
    /// Fallout 4, `bsaGame=5`.
    Fo4,
}

/// The games the GUI offers. `bsaGame` keeps its own read and error
/// ([`ProfileError::UnsupportedBsaGame`]) rather than [`closed::read`]'s.
impl ClosedSet for BsaGame {
    const ALL: &'static [Self] = &[Self::Tes5, Self::Sse, Self::Fo4];

    /// The `bsaGame` number written to `profile.ini`.
    fn number(self) -> u32 {
        match self {
            Self::Tes5 => 3,
            Self::Sse => 4,
            Self::Fo4 => 5,
        }
    }
}

impl BsaGame {
    /// Reads `bsaGame` as C++ does (`toInt()`), then rejects anything but 3, 4 or 5.
    fn read(value: &Value) -> Result<Self, ProfileError> {
        match value.to_i32() {
            3 => Ok(Self::Tes5),
            4 => Ok(Self::Sse),
            5 => Ok(Self::Fo4),
            _ => Err(ProfileError::UnsupportedBsaGame {
                value: value.to_qstring(),
            }),
        }
    }
}

/// A profile's `profile.ini`: the per-game settings C++ held in `Profiles`.
///
/// Bools read with `QVariant`'s conversions, so a missing one is `false`, never a
/// default, and a missing maximum Archive size is `0`; a Mesh Target axis and the Texture format are
/// closed sets ([`crate::ClosedSet`]) that `0` is outside, so a missing one is
/// unreadable. Keys this model does not hold, such as the dead
/// `[Animations] animationFormat` (deviation 10), are never read and survive a save.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileSettings {
    /// The Profile Capability for Archive extraction and creation
    /// (`[BSA] bsaEnabled`); the GUI enables its Archives tab from it.
    pub bsa_enabled: bool,
    /// The largest uncompressed Archive to create, in whole bytes
    /// (`[BSA] maxBsaUncompressedSize`). The packer raises it to the game's
    /// archive-table maximum when that is larger, so a Profile can only allow
    /// larger Archives; those tables live in `cao-archive`, so the composition
    /// root applies that floor.
    pub max_bsa_uncompressed_size: ArchiveSize,
    /// The game whose archive naming, versions and Loading Plugin rules apply
    /// (`[BSA] bsaGame`).
    pub bsa_game: BsaGame,
    /// The Profile Capability for Mesh optimization (`[Meshes] meshesEnabled`).
    pub meshes_enabled: bool,
    /// The NIF file version Meshes are written as (`[Meshes] meshesFileVersion`),
    /// one axis of the Mesh Target.
    pub meshes_file_version: MeshFileVersion,
    /// The NIF stream version Meshes are written with (`[Meshes] meshesStream`),
    /// one axis of the Mesh Target.
    pub meshes_stream: MeshStreamVersion,
    /// The NIF user version Meshes are written with (`[Meshes] meshesUser`), one
    /// axis of the Mesh Target.
    pub meshes_user: MeshUserVersion,
    /// The Profile Capability for Animation optimization
    /// (`[Animations] animationsEnabled`).
    pub animations_enabled: bool,
    /// The Profile Capability for Texture optimization and conversion
    /// (`[Textures] texturesEnabled`).
    pub textures_enabled: bool,
    /// The format Textures are compressed to (`[Textures] texturesFormat`), one
    /// part of the Texture Target.
    pub textures_format: TextureFormat,
    /// Whether TGA Textures are converted to DDS.
    pub textures_convert_tga: bool,
    /// The formats a Texture is converted away from
    /// (`[Textures] texturesUnwantedFormats`), the other part of the Texture
    /// Target: in the user's order, each listed once.
    pub textures_unwanted_formats: Vec<UnwantedFormat>,
    /// Whether interface Textures are compressed. Not part of the Texture
    /// Target: it decides which Textures are compressed, not what they become.
    pub textures_compress_interface: bool,
}

impl ProfileSettings {
    /// Reads the model from `profile.ini`. Bools keep `QVariant`'s lenient
    /// reading (a missing key is `false`), while every value with a closed set
    /// or a unit is checked, so a value CAO cannot act on makes the Profile
    /// unreadable instead of being replaced on the next save.
    ///
    /// # Errors
    /// [`ProfileError::UnsupportedBsaGame`] when `bsaGame` is not 3, 4 or 5, and
    /// [`ProfileError::UnsupportedValue`] when a Mesh Target axis or the Texture
    /// format is outside its closed set or missing, or an unwanted format is
    /// outside its closed set.
    ///
    /// The maximum Archive size is also [`ProfileError::UnsupportedValue`] when
    /// it is neither a number of bytes nor a size with a unit, or does not fit
    /// in whole bytes: see [`ArchiveSize`].
    pub fn read(ini: &IniFile) -> Result<Self, ProfileError> {
        Ok(Self {
            bsa_enabled: ini.value("BSA/bsaEnabled").to_bool(),
            max_bsa_uncompressed_size: ArchiveSize::read(ini, "BSA/maxBsaUncompressedSize")?,
            bsa_game: BsaGame::read(ini.value("BSA/bsaGame"))?,
            meshes_enabled: ini.value("Meshes/meshesEnabled").to_bool(),
            meshes_file_version: closed::read(ini, "Meshes/meshesFileVersion")?,
            meshes_stream: closed::read(ini, "Meshes/meshesStream")?,
            meshes_user: closed::read(ini, "Meshes/meshesUser")?,
            animations_enabled: ini.value("Animations/animationsEnabled").to_bool(),
            textures_enabled: ini.value("Textures/texturesEnabled").to_bool(),
            textures_format: closed::read(ini, "Textures/texturesFormat")?,
            textures_convert_tga: ini.value("Textures/texturesConvertTga").to_bool(),
            textures_unwanted_formats: closed::read_list(ini, "Textures/texturesUnwantedFormats")?,
            textures_compress_interface: ini.value("Textures/texturesCompressInterface").to_bool(),
        })
    }

    /// Writes the model into `profile.ini` in the shipped files' key order, so a
    /// missing key is appended in its section where QSettings appends it. Keys
    /// the model does not hold are left alone.
    pub fn write(&self, ini: &mut IniFile) {
        ini.set("BSA/bsaEnabled", self.bsa_enabled);
        ini.set(
            "BSA/maxBsaUncompressedSize",
            self.max_bsa_uncompressed_size.to_written(),
        );
        ini.set("BSA/bsaGame", self.bsa_game.number());

        ini.set("Meshes/meshesEnabled", self.meshes_enabled);
        ini.set(
            "Meshes/meshesFileVersion",
            self.meshes_file_version.number(),
        );
        ini.set("Meshes/meshesStream", self.meshes_stream.number());
        ini.set("Meshes/meshesUser", self.meshes_user.number());

        ini.set("Animations/animationsEnabled", self.animations_enabled);

        // `Value::int_list` writes the `Int` list QSettings reads back, including
        // the `@Variant(…)` shape of a one-element list. Every unwanted format's
        // number is at most 115, so it fits an `i32`.
        let formats: Vec<i32> = self
            .textures_unwanted_formats
            .iter()
            .map(|format| format.number() as i32)
            .collect();
        ini.set("Textures/texturesEnabled", self.textures_enabled);
        ini.set("Textures/texturesFormat", self.textures_format.number());
        ini.set("Textures/texturesConvertTga", self.textures_convert_tga);
        ini.set(
            "Textures/texturesUnwantedFormats",
            Value::int_list(&formats),
        );
        ini.set(
            "Textures/texturesCompressInterface",
            self.textures_compress_interface,
        );
    }
}
