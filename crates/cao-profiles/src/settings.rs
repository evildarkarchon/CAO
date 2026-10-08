//! The settings model of a profile's `profile.ini`: the game it targets and what
//! each Asset Kind may do under it.

use crate::error::ProfileError;
use crate::ini::{IniFile, Value};

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

impl BsaGame {
    /// The `bsaGame` number written to `profile.ini`.
    pub fn number(self) -> i32 {
        match self {
            Self::Tes5 => 3,
            Self::Sse => 4,
            Self::Fo4 => 5,
        }
    }

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
/// Fields read with `QVariant`'s conversions, as C++ reads them: a missing key is
/// `false` or `0`, never a default. Keys this model does not hold, such as the dead
/// `[Animations] animationFormat` (deviation 10), are never read and survive a save.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileSettings {
    /// The Profile Capability for Archive extraction and creation
    /// (`[BSA] bsaEnabled`); the GUI enables its Archives tab from it.
    pub bsa_enabled: bool,
    /// The largest uncompressed Archive to create, in bytes. C++ raised it to the
    /// game's archive-table maximum when that is larger; those tables live in
    /// `cao-archive`, so the composition root applies that floor.
    pub max_bsa_uncompressed_size: f64,
    /// The game whose archive naming, versions and Loading Plugin rules apply
    /// (`[BSA] bsaGame`).
    pub bsa_game: BsaGame,
    /// The Profile Capability for Mesh optimization (`[Meshes] meshesEnabled`).
    pub meshes_enabled: bool,
    /// The NIF file version Meshes are written as (`nifly::NiFileVersion`).
    pub meshes_file_version: u32,
    /// The NIF stream version Meshes are written with (`[Meshes] meshesStream`).
    pub meshes_stream: u32,
    /// The NIF user version Meshes are written with (`[Meshes] meshesUser`).
    pub meshes_user: u32,
    /// The Profile Capability for Animation optimization
    /// (`[Animations] animationsEnabled`).
    pub animations_enabled: bool,
    /// The Profile Capability for Texture optimization and conversion
    /// (`[Textures] texturesEnabled`).
    pub textures_enabled: bool,
    /// The `DXGI_FORMAT` Textures are compressed to.
    pub textures_format: u32,
    /// Whether TGA Textures are converted to DDS.
    pub textures_convert_tga: bool,
    /// The `DXGI_FORMAT`s a Texture is converted away from, in the user's order.
    pub textures_unwanted_formats: Vec<u32>,
    /// Whether interface Textures are compressed.
    pub textures_compress_interface: bool,
}

impl ProfileSettings {
    /// Reads the model from `profile.ini` as C++'s `Profiles::readFromIni` does.
    ///
    /// # Errors
    /// [`ProfileError::UnsupportedBsaGame`] when `bsaGame` is not 3, 4 or 5.
    pub fn read(ini: &IniFile) -> Result<Self, ProfileError> {
        // `meshesFileVersion` and `texturesFormat` are `toInt()` cast to unsigned enums.
        let enum_value = |key| ini.value(key).to_i32() as u32;
        Ok(Self {
            bsa_enabled: ini.value("BSA/bsaEnabled").to_bool(),
            max_bsa_uncompressed_size: ini.value("BSA/maxBsaUncompressedSize").to_f64(),
            bsa_game: BsaGame::read(ini.value("BSA/bsaGame"))?,
            meshes_enabled: ini.value("Meshes/meshesEnabled").to_bool(),
            meshes_file_version: enum_value("Meshes/meshesFileVersion"),
            meshes_stream: ini.value("Meshes/meshesStream").to_u32(),
            meshes_user: ini.value("Meshes/meshesUser").to_u32(),
            animations_enabled: ini.value("Animations/animationsEnabled").to_bool(),
            textures_enabled: ini.value("Textures/texturesEnabled").to_bool(),
            textures_format: enum_value("Textures/texturesFormat"),
            textures_convert_tga: ini.value("Textures/texturesConvertTga").to_bool(),
            textures_unwanted_formats: ini
                .value("Textures/texturesUnwantedFormats")
                .to_int_list()
                .into_iter()
                .map(|format| format as u32)
                .collect(),
            textures_compress_interface: ini.value("Textures/texturesCompressInterface").to_bool(),
        })
    }

    /// Writes the model into `profile.ini` as C++'s `Profiles::saveToIni` does, in the
    /// same key order, so missing keys are appended where Qt appends them. Keys the
    /// model does not hold are left alone.
    pub fn write(&self, ini: &mut IniFile) {
        ini.set("BSA/bsaEnabled", self.bsa_enabled);
        ini.set("BSA/maxBsaUncompressedSize", self.max_bsa_uncompressed_size);
        ini.set("BSA/bsaGame", self.bsa_game.number());

        ini.set("Meshes/meshesEnabled", self.meshes_enabled);
        ini.set("Meshes/meshesFileVersion", self.meshes_file_version);
        ini.set("Meshes/meshesStream", self.meshes_stream);
        ini.set("Meshes/meshesUser", self.meshes_user);

        ini.set("Animations/animationsEnabled", self.animations_enabled);

        // `DXGI_FORMAT` has no fixed underlying type, so C++ stores it as an `int`.
        let formats: Vec<i32> = self
            .textures_unwanted_formats
            .iter()
            .map(|&format| format as i32)
            .collect();
        ini.set("Textures/texturesEnabled", self.textures_enabled);
        ini.set("Textures/texturesFormat", self.textures_format as i32);
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
