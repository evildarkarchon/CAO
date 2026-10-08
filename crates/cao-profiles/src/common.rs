//! The model of `profiles/common.ini`: the settings shared by every profile,
//! including which profile is selected.

use crate::ini::IniFile;

/// `profiles/common.ini`, the layer above the profiles: the profile the GUI last
/// selected, and the GUI's own preferences. Every key is a root key under
/// `[General]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonSettings {
    /// The selected profile's name (`profile`). It may name a profile that no longer
    /// exists; [`crate::Profiles::resolve`] falls back to SSE for that.
    pub profile: String,
    /// `bShowAdvancedSettings`.
    pub show_advanced_settings: bool,
    /// `bDarkMode`.
    pub dark_mode: bool,
    /// `showTutorial`. Unlike every other key, C++ reads a missing one as `true`.
    pub show_tutorial: bool,
    /// `notFirstStart`: whether the welcome message has been shown.
    pub not_first_start: bool,
}

impl CommonSettings {
    /// Reads the model from `common.ini` as C++'s GUI reads it. A missing file reads
    /// like an empty one.
    pub fn read(ini: &IniFile) -> Self {
        Self {
            profile: ini.value("profile").to_qstring(),
            show_advanced_settings: ini.value("bShowAdvancedSettings").to_bool(),
            dark_mode: ini.value("bDarkMode").to_bool(),
            // `value("showTutorial", true)`: the default applies only to a missing key.
            show_tutorial: ini.get("showTutorial").is_none_or(|value| value.to_bool()),
            not_first_start: ini.value("notFirstStart").to_bool(),
        }
    }

    /// Writes the model into `common.ini`, in the order C++ first writes the keys, so
    /// a new file matches the shipped one. Keys the model does not hold are left
    /// alone.
    pub fn write(&self, ini: &mut IniFile) {
        ini.set("profile", self.profile.as_str());
        ini.set("bShowAdvancedSettings", self.show_advanced_settings);
        ini.set("bDarkMode", self.dark_mode);
        ini.set("showTutorial", self.show_tutorial);
        ini.set("notFirstStart", self.not_first_start);
    }
}
