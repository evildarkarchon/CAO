//! The options model of a profile's `settings.ini`: the user's choices for a run,
//! as C++ held them in `OptionsCAO`.

use crate::ini::IniFile;

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

/// A profile's `settings.ini`: the options C++ held in `OptionsCAO`.
///
/// [`Options::default`] is C++'s member defaults, which apply only when the first
/// profile loaded has no file. Otherwise every field reads with `QVariant`'s conversions, so a missing
/// key is `false` or `0`: the shipped files have no `bBsaMergeIncomp`, so it loads as
/// `false`. Keys this model does not hold, such as the dead `[BSA] bBsaLeastBSA`
/// (deviation 10), are never read and survive a save.
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
    /// C++'s `OptionsCAO` member defaults. C++ left `mode` uninitialized; here it is
    /// Single Mod.
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
    /// Reads the model from an existing `settings.ini` over `current`, as C++'s
    /// `OptionsCAO::readFromIni` reads into the GUI's one live `OptionsCAO`.
    ///
    /// Every key is read and replaces `current`'s value, except an empty `userPath`,
    /// which keeps `current.user_path`. That is how the selected folder follows the
    /// user between profiles: every shipped `settings.ini` has `userPath=`. When the
    /// file does not exist, C++ keeps all of `current` instead; see
    /// [`crate::Profile::load_options`].
    pub fn read(ini: &IniFile, current: &Self) -> Self {
        let flag = |key| ini.value(key).to_bool();
        let number = |key| ini.value(key).to_u32();
        let user_path = ini.value("userPath").to_qstring();
        Self {
            dry_run: flag("bDryRun"),
            debug_log: flag("bDebugLog"),
            mode: OptimizationMode::from_number(ini.value("mode").to_i32()),
            user_path: if user_path.is_empty() {
                current.user_path.clone()
            } else {
                user_path
            },
            bsa_extract: flag("BSA/bBsaExtract"),
            bsa_create: flag("BSA/bBsaCreate"),
            bsa_delete_backup: flag("BSA/bBsaDeleteBackup"),
            bsa_merge_incompressible: flag("BSA/bBsaMergeIncomp"),
            bsa_merge_textures: flag("BSA/bBsaMergeTexture"),
            bsa_process_content: flag("BSA/bBsaProcessContent"),
            bsa_create_dummies: flag("BSA/bBsaCreateDummies"),
            bsa_compress: flag("BSA/bBsaCompress"),
            bsa_delete_source: flag("BSA/bBsaDeleteSource"),
            textures_necessary: flag("Textures/bTexturesNecessary"),
            textures_compress: flag("Textures/bTexturesCompress"),
            textures_mipmaps: flag("Textures/bTexturesMipmaps"),
            textures_resize_size: flag("Textures/bTexturesResizeSize"),
            textures_target_width: number("Textures/iTexturesTargetWidth"),
            textures_target_height: number("Textures/iTexturesTargetHeight"),
            textures_resize_ratio: flag("Textures/bTexturesResizeRatio"),
            textures_target_width_ratio: number("Textures/iTexturesTargetWidthRatio"),
            textures_target_height_ratio: number("Textures/iTexturesTargetHeightRatio"),
            meshes_optimization_level: ini.value("Meshes/iMeshesOptimizationLevel").to_i32(),
            meshes_headparts: flag("Meshes/bMeshesHeadparts"),
            meshes_resave: flag("Meshes/bMeshesResave"),
            animations_optimization: flag("Animations/bAnimationsOptimization"),
        }
    }

    /// Writes the model into `settings.ini` as C++'s `OptionsCAO::saveToIni` does.
    /// Keys already in `ini` keep their place; the order of the calls below is the
    /// order Qt appends missing keys in, so it follows C++ exactly. Keys the model
    /// does not hold are left alone.
    pub fn write(&self, ini: &mut IniFile) {
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
