//! The options model of a profile's `settings.ini`: the user's choices for a run,
//! as C++ held them in `OptionsCAO`.

use crate::ini::{IniFile, Value};

/// How the selected folder becomes the Mod Selection (`mode`). The name is C++'s
/// `OptionsCAO::OptimizationMode`, kept so the INI key and the code line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptimizationMode {
    /// The folder is one mod, `mode=0`.
    #[default]
    SingleMod,
    /// Each child of the folder is a mod (Several Mods), `mode=1`.
    SeveralMods,
    /// Any other number, from a hand-edited file. It is kept so a save writes it
    /// back unchanged; C++ rejected it only when a run started ("This mode does not
    /// exist").
    Unsupported(i32),
}

impl OptimizationMode {
    /// The `mode` number written to `settings.ini`.
    pub fn number(self) -> i32 {
        match self {
            Self::SingleMod => 0,
            Self::SeveralMods => 1,
            Self::Unsupported(number) => number,
        }
    }

    /// The mode a `mode` number stands for.
    fn from_number(number: i32) -> Self {
        match number {
            0 => Self::SingleMod,
            1 => Self::SeveralMods,
            _ => Self::Unsupported(number),
        }
    }
}

/// A profile's `settings.ini`: the user's choices for a run.
///
/// [`Options::default`] holds each option's default. A profile without the file
/// starts from them, and a key missing from the file reads as its default, so an
/// option added in a later version starts at its intended value for every
/// existing user rather than off. A key that is present reads with `QVariant`'s
/// lenient conversions. Keys this model does not hold, such as the dead
/// `[BSA] bBsaLeastBSA` (deviation 10), are never read and survive a save.
///
/// The model holds what the file holds; whether the values make a valid run is
/// decided when a run starts, not here. It is not a Run Request: the composition root
/// builds one from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Dry Run (`bDryRun`).
    pub dry_run: bool,
    /// Debug logging (`bDebugLog`).
    pub debug_log: bool,
    /// Whether the selected folder is one Mod Root or a mods directory for Several
    /// Mods selection (`mode`).
    pub mode: OptimizationMode,
    /// The folder the user selected, from which the Mod Selection is made
    /// (`userPath`).
    pub user_path: String,

    /// `[BSA] bBsaExtract`.
    pub bsa_extract: bool,
    /// `[BSA] bBsaCreate`.
    pub bsa_create: bool,
    /// `[BSA] bBsaDeleteBackup`.
    pub bsa_delete_backup: bool,
    /// `[BSA] bBsaMergeIncomp`: incompressible files go into the main Archive.
    pub bsa_merge_incompressible: bool,
    /// `[BSA] bBsaMergeTexture`: Textures go into the main Archive.
    pub bsa_merge_textures: bool,
    /// `[BSA] bBsaProcessContent`.
    pub bsa_process_content: bool,
    /// `[BSA] bBsaCreateDummies`.
    pub bsa_create_dummies: bool,
    /// `[BSA] bBsaCompress`.
    pub bsa_compress: bool,
    /// `[BSA] bBsaDeleteSource`.
    pub bsa_delete_source: bool,

    /// `[Textures] bTexturesNecessary`.
    pub textures_necessary: bool,
    /// `[Textures] bTexturesCompress`.
    pub textures_compress: bool,
    /// `[Textures] bTexturesMipmaps`.
    pub textures_mipmaps: bool,
    /// `[Textures] bTexturesResizeSize`.
    pub textures_resize_size: bool,
    /// `[Textures] iTexturesTargetWidth`.
    pub textures_target_width: u32,
    /// `[Textures] iTexturesTargetHeight`.
    pub textures_target_height: u32,
    /// `[Textures] bTexturesResizeRatio`.
    pub textures_resize_ratio: bool,
    /// `[Textures] iTexturesTargetWidthRatio`.
    pub textures_target_width_ratio: u32,
    /// `[Textures] iTexturesTargetHeightRatio`.
    pub textures_target_height_ratio: u32,

    /// `[Meshes] iMeshesOptimizationLevel`: 0 (off) to 3 (full). Other numbers are
    /// kept and rejected when a run starts, as in C++.
    pub meshes_optimization_level: i32,
    /// `[Meshes] bMeshesHeadparts`.
    pub meshes_headparts: bool,
    /// `[Meshes] bMeshesResave`.
    pub meshes_resave: bool,

    /// `[Animations] bAnimationsOptimization`.
    pub animations_optimization: bool,
}

impl Default for Options {
    /// Each option's default: what a profile without `settings.ini` starts with,
    /// and what a key missing from the file reads as. They match core's default
    /// Work Choices, which `cao-optimizers`' tests pin to this form. The
    /// shipped profiles set the keys that differ, such as keeping incompressible
    /// files out of the main Archive, in their own files.
    fn default() -> Self {
        Self {
            dry_run: false,
            debug_log: false,
            mode: OptimizationMode::SingleMod,
            user_path: String::new(),
            bsa_extract: false,
            bsa_create: false,
            bsa_delete_backup: false,
            bsa_merge_incompressible: true,
            bsa_merge_textures: false,
            bsa_process_content: false,
            bsa_create_dummies: true,
            bsa_compress: true,
            bsa_delete_source: true,
            textures_necessary: true,
            textures_compress: false,
            textures_mipmaps: false,
            textures_resize_size: false,
            textures_target_width: 2048,
            textures_target_height: 2048,
            textures_resize_ratio: false,
            textures_target_width_ratio: 1,
            textures_target_height_ratio: 1,
            meshes_optimization_level: 0,
            meshes_headparts: true,
            meshes_resave: false,
            animations_optimization: false,
        }
    }
}

impl Options {
    /// Reads the model from an existing `settings.ini`. Each key that is missing
    /// reads as its [`Options::default`] value.
    ///
    /// `carried_path` is the mod folder in use before this profile was selected.
    /// An empty `userPath` keeps it, which is how the selected folder follows the
    /// user between profiles: every shipped `settings.ini` has `userPath=`.
    /// Nothing else carries over; see [`crate::Profile::load_options`].
    pub(crate) fn read(ini: &IniFile, carried_path: &str) -> Self {
        let defaults = Self::default();
        let flag = |key, default| ini.get(key).map_or(default, Value::to_bool);
        let number = |key, default| ini.get(key).map_or(default, Value::to_u32);
        let user_path = ini.value("userPath").to_qstring();
        Self {
            dry_run: flag("bDryRun", defaults.dry_run),
            debug_log: flag("bDebugLog", defaults.debug_log),
            mode: ini.get("mode").map_or(defaults.mode, |mode| {
                OptimizationMode::from_number(mode.to_i32())
            }),
            user_path: if user_path.is_empty() {
                carried_path.to_owned()
            } else {
                user_path
            },
            bsa_extract: flag("BSA/bBsaExtract", defaults.bsa_extract),
            bsa_create: flag("BSA/bBsaCreate", defaults.bsa_create),
            bsa_delete_backup: flag("BSA/bBsaDeleteBackup", defaults.bsa_delete_backup),
            bsa_merge_incompressible: flag(
                "BSA/bBsaMergeIncomp",
                defaults.bsa_merge_incompressible,
            ),
            bsa_merge_textures: flag("BSA/bBsaMergeTexture", defaults.bsa_merge_textures),
            bsa_process_content: flag("BSA/bBsaProcessContent", defaults.bsa_process_content),
            // On by default: without a Dummy Plugin a packed Archive may have
            // no plugin to load it, so Archive creation skips it.
            bsa_create_dummies: flag("BSA/bBsaCreateDummies", defaults.bsa_create_dummies),
            bsa_compress: flag("BSA/bBsaCompress", defaults.bsa_compress),
            bsa_delete_source: flag("BSA/bBsaDeleteSource", defaults.bsa_delete_source),
            textures_necessary: flag("Textures/bTexturesNecessary", defaults.textures_necessary),
            textures_compress: flag("Textures/bTexturesCompress", defaults.textures_compress),
            textures_mipmaps: flag("Textures/bTexturesMipmaps", defaults.textures_mipmaps),
            textures_resize_size: flag(
                "Textures/bTexturesResizeSize",
                defaults.textures_resize_size,
            ),
            textures_target_width: number(
                "Textures/iTexturesTargetWidth",
                defaults.textures_target_width,
            ),
            textures_target_height: number(
                "Textures/iTexturesTargetHeight",
                defaults.textures_target_height,
            ),
            textures_resize_ratio: flag(
                "Textures/bTexturesResizeRatio",
                defaults.textures_resize_ratio,
            ),
            textures_target_width_ratio: number(
                "Textures/iTexturesTargetWidthRatio",
                defaults.textures_target_width_ratio,
            ),
            textures_target_height_ratio: number(
                "Textures/iTexturesTargetHeightRatio",
                defaults.textures_target_height_ratio,
            ),
            meshes_optimization_level: ini
                .get("Meshes/iMeshesOptimizationLevel")
                .map_or(defaults.meshes_optimization_level, Value::to_i32),
            meshes_headparts: flag("Meshes/bMeshesHeadparts", defaults.meshes_headparts),
            meshes_resave: flag("Meshes/bMeshesResave", defaults.meshes_resave),
            animations_optimization: flag(
                "Animations/bAnimationsOptimization",
                defaults.animations_optimization,
            ),
        }
    }

    /// Writes the model into `settings.ini`. Keys already in `ini` keep their
    /// place; the order of the calls below is the order missing keys are
    /// appended in, the order of the shipped files. Keys the model does not hold
    /// are left alone.
    pub(crate) fn write(&self, ini: &mut IniFile) {
        ini.set("bDryRun", self.dry_run);
        ini.set("bDebugLog", self.debug_log);
        ini.set("mode", self.mode.number());
        ini.set("userPath", self.user_path.as_str());

        ini.set("BSA/bBsaExtract", self.bsa_extract);
        ini.set("BSA/bBsaCreate", self.bsa_create);
        ini.set("BSA/bBsaDeleteBackup", self.bsa_delete_backup);
        ini.set("BSA/bBsaMergeIncomp", self.bsa_merge_incompressible);
        ini.set("BSA/bBsaMergeTexture", self.bsa_merge_textures);
        ini.set("BSA/bBsaProcessContent", self.bsa_process_content);
        ini.set("BSA/bBsaCreateDummies", self.bsa_create_dummies);
        ini.set("BSA/bBsaCompress", self.bsa_compress);
        ini.set("BSA/bBsaDeleteSource", self.bsa_delete_source);

        ini.set("Textures/bTexturesNecessary", self.textures_necessary);
        ini.set("Textures/bTexturesCompress", self.textures_compress);
        ini.set("Textures/bTexturesMipmaps", self.textures_mipmaps);
        ini.set("Textures/bTexturesResizeSize", self.textures_resize_size);
        ini.set("Textures/iTexturesTargetWidth", self.textures_target_width);
        ini.set(
            "Textures/iTexturesTargetHeight",
            self.textures_target_height,
        );
        ini.set("Textures/bTexturesResizeRatio", self.textures_resize_ratio);
        // Height before width, as C++ writes them.
        ini.set(
            "Textures/iTexturesTargetHeightRatio",
            self.textures_target_height_ratio,
        );
        ini.set(
            "Textures/iTexturesTargetWidthRatio",
            self.textures_target_width_ratio,
        );

        ini.set(
            "Meshes/iMeshesOptimizationLevel",
            self.meshes_optimization_level,
        );
        ini.set("Meshes/bMeshesHeadparts", self.meshes_headparts);
        ini.set("Meshes/bMeshesResave", self.meshes_resave);

        ini.set(
            "Animations/bAnimationsOptimization",
            self.animations_optimization,
        );
    }
}
