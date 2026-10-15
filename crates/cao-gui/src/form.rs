//! Moves a profile's models into the window's controls and back (C++
//! `OptionsCAO::saveToUi`/`readFromUi` and `Profiles::saveToUi`/`readFromUi`).
//!
//! Each direction follows C++ rule for rule, including its quirks, so a profile
//! the C++ build saved reloads the same, and what the window saves reloads the
//! same. #509's UI rules (Dry Run, Several Mods, FO4 textures; `rules.rs`) are
//! not applied here; they run after [`show_options`] and [`show_settings`].

use cao_gui_ui::{AnimationsTab, BsaTab, MainWindow, MeshesTab, TexturesTab};
use cao_profiles::{BsaGame, OptimizationMode, Options, ProfileSettings};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::formats::{format_name, format_number};

/// C++ `GigaByte`: the maximum archive size is stored in bytes, shown in GB.
const GIGABYTE: f64 = 1024.0 * 1024.0 * 1024.0;

/// The Game combo's games, by index (C++ `MainWindow` item data).
const GAMES: [BsaGame; 3] = [BsaGame::Tes5, BsaGame::Sse, BsaGame::Fo4];
/// The Version combo's NIF file versions: nifly's `V20_0_0_5` and `V20_2_0_7`.
const MESH_VERSIONS: [u32; 2] = [0x1400_0005, 0x1402_0007];
/// The User combo's NIF user versions.
const MESH_USERS: [u32; 2] = [11, 12];
/// The Stream combo's NIF stream versions.
const MESH_STREAMS: [u32; 4] = [82, 83, 100, 130];
/// The Output format combo's `DXGI_FORMAT`s: BC7, BC5, BC3, BC1 and R8G8B8A8,
/// all `_UNORM`.
const OUTPUT_FORMATS: [u32; 5] = [98, 83, 77, 71, 28];

/// QSpinBox ranges of the Downsizing spin boxes, which clamp what they are set to.
const RATIO_RANGE: (u32, u32) = (2, 10);
const SIZE_RANGE: (u32, u32) = (4, 4096);

/// Which tabs the profile enables, as the tab bar shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnabledTabs {
    pub bsa: bool,
    pub meshes: bool,
    pub textures: bool,
    pub animations: bool,
}

impl EnabledTabs {
    /// Reads the tab bar's per-tab enable.
    pub(crate) fn of(ui: &MainWindow) -> Self {
        let tabs = ui.get_tabs();
        let enabled = |id: &str| tabs.iter().any(|tab| tab.id == id && tab.enabled);
        Self {
            bsa: enabled("bsa"),
            meshes: enabled("meshes"),
            textures: enabled("textures"),
            animations: enabled("animations"),
        }
    }
}

/// Shows the options model (`settings.ini`) in the window.
pub(crate) fn show_options(ui: &MainWindow, options: &Options) {
    let bsa = ui.global::<BsaTab>();
    bsa.set_extract(options.bsa_extract);
    bsa.set_create(options.bsa_create);
    bsa.set_delete_backups(options.bsa_delete_backup);
    bsa.set_separate_incompressible(!options.bsa_merge_incompressible);
    bsa.set_separate_textures(!options.bsa_merge_textures);
    bsa.set_create_dummies(options.bsa_create_dummies);
    bsa.set_compress(options.bsa_compress);
    bsa.set_delete_source(options.bsa_delete_source);

    let textures = ui.global::<TexturesTab>();
    // C++ leaves the three boxes as they were when it unchecks the group.
    if options.textures_mipmaps || options.textures_compress || options.textures_necessary {
        textures.set_process(true);
        textures.set_necessary(options.textures_necessary);
        textures.set_compress(options.textures_compress);
        textures.set_mipmaps(options.textures_mipmaps);
    } else {
        textures.set_process(false);
    }

    textures.set_resize(options.textures_resize_size || options.textures_resize_ratio);
    // The radios are auto-exclusive, and Qt cannot uncheck the checked one, so
    // C++'s `setChecked(size)` then `setChecked(ratio)` lets ratio win and keeps
    // the previous choice when neither is set.
    if options.textures_resize_ratio {
        textures.set_resize_mode(0);
    } else if options.textures_resize_size {
        textures.set_resize_mode(1);
    }
    textures.set_size_width(clamp(options.textures_target_width, SIZE_RANGE));
    textures.set_size_height(clamp(options.textures_target_height, SIZE_RANGE));
    textures.set_ratio_width(clamp(options.textures_target_width_ratio, RATIO_RANGE));
    textures.set_ratio_height(clamp(options.textures_target_height_ratio, RATIO_RANGE));

    let meshes = ui.global::<MeshesTab>();
    meshes.set_process(true);
    match options.meshes_optimization_level {
        0 => meshes.set_process(false),
        // All three, so the global stays exclusive while the Meshes page (whose
        // RadioGroup would uncheck the others) is not shown. When it is shown,
        // the group's change handlers settle on the same choice in any order.
        level @ 1..=3 => {
            meshes.set_necessary(level == 1);
            meshes.set_medium(level == 2);
            meshes.set_full(level == 3);
        }
        // C++ checks the group and leaves the radios for any other number.
        _ => {}
    }
    meshes.set_resave(options.meshes_resave);
    meshes.set_headparts(options.meshes_headparts);

    ui.global::<AnimationsTab>()
        .set_necessary(options.animations_optimization);

    ui.set_debug_log(options.debug_log);
    ui.set_dry_run(options.dry_run);
    // `findData` finds no item for an unsupported mode, which selects nothing.
    ui.set_mode_index(match options.mode {
        OptimizationMode::SingleMod => 0,
        OptimizationMode::SeveralMods => 1,
        OptimizationMode::Unsupported(_) => -1,
    });
    ui.set_user_path(options.user_path.as_str().into());
}

/// Reads the options model back from the window, over `current`: the options
/// last loaded, which keep what the window does not show (`bBsaProcessContent`,
/// and the mesh level when no level radio is checked).
pub(crate) fn read_options(ui: &MainWindow, current: &Options) -> Options {
    let tabs = EnabledTabs::of(ui);
    let mut options = current.clone();

    // The base group is disabled while Dry Run is on (`rules.rs`), and C++
    // reads every BSA option as off while the tab or that group is disabled.
    let bsa = ui.global::<BsaTab>();
    let bsa_enabled = tabs.bsa && !ui.get_dry_run();
    options.bsa_extract = bsa_enabled && bsa.get_extract();
    options.bsa_create = bsa_enabled && bsa.get_create();
    options.bsa_delete_backup = bsa_enabled && bsa.get_delete_backups();
    options.bsa_merge_incompressible = bsa_enabled && !bsa.get_separate_incompressible();
    options.bsa_merge_textures = bsa_enabled && !bsa.get_separate_textures();
    options.bsa_create_dummies = bsa_enabled && bsa.get_create_dummies();
    options.bsa_compress = bsa_enabled && bsa.get_compress();
    options.bsa_delete_source = bsa_enabled && bsa.get_delete_source();

    let textures = ui.global::<TexturesTab>();
    let textures_enabled = textures.get_process() && tabs.textures;
    options.textures_necessary = textures_enabled && textures.get_necessary();
    options.textures_mipmaps = textures_enabled && textures.get_mipmaps();
    options.textures_compress = textures_enabled && textures.get_compress();

    let resizing = textures.get_resize() && tabs.textures;
    options.textures_resize_size = resizing && textures.get_resize_mode() == 1;
    options.textures_target_width = unsigned(textures.get_size_width());
    options.textures_target_height = unsigned(textures.get_size_height());
    options.textures_resize_ratio = resizing && textures.get_resize_mode() == 0;
    options.textures_target_width_ratio = unsigned(textures.get_ratio_width());
    options.textures_target_height_ratio = unsigned(textures.get_ratio_height());

    let meshes = ui.global::<MeshesTab>();
    // With no radio checked, C++ keeps the level it last read.
    if meshes.get_necessary() {
        options.meshes_optimization_level = 1;
    } else if meshes.get_medium() {
        options.meshes_optimization_level = 2;
    } else if meshes.get_full() {
        options.meshes_optimization_level = 3;
    }
    if !meshes.get_process() || !tabs.meshes {
        options.meshes_optimization_level = 0;
    }
    options.meshes_headparts = tabs.meshes && meshes.get_headparts();
    options.meshes_resave = tabs.meshes && meshes.get_resave();

    options.animations_optimization =
        tabs.animations && ui.global::<AnimationsTab>().get_necessary();

    options.dry_run = ui.get_dry_run();
    options.user_path = clean_path(&ui.get_user_path());
    // No selection reads as Single Mod, as an invalid `QVariant` converts to 0.
    options.mode = if ui.get_mode_index() == 1 {
        OptimizationMode::SeveralMods
    } else {
        OptimizationMode::SingleMod
    };
    options.debug_log = ui.get_debug_log();
    options
}

/// Shows the settings model (`profile.ini`) in the advanced groups. A value no
/// combo item holds leaves that combo as it was, as C++'s `iterateComboBox` does.
pub(crate) fn show_settings(ui: &MainWindow, settings: &ProfileSettings) {
    let bsa = ui.global::<BsaTab>();
    bsa.set_max_size((settings.max_bsa_uncompressed_size / GIGABYTE) as f32);
    if let Some(index) = index_of(&GAMES, &settings.bsa_game) {
        bsa.set_game_index(index);
    }

    let meshes = ui.global::<MeshesTab>();
    if let Some(index) = index_of(&MESH_USERS, &settings.meshes_user) {
        meshes.set_user_index(index);
    }
    if let Some(index) = index_of(&MESH_STREAMS, &settings.meshes_stream) {
        meshes.set_stream_index(index);
    }
    if let Some(index) = index_of(&MESH_VERSIONS, &settings.meshes_file_version) {
        meshes.set_version_index(index);
    }

    let textures = ui.global::<TexturesTab>();
    if let Some(index) = index_of(&OUTPUT_FORMATS, &settings.textures_format) {
        textures.set_output_format_index(index);
    }
    textures.set_tga_conversion(settings.textures_convert_tga);
    textures.set_compress_interface(settings.textures_compress_interface);
    let names: Vec<SharedString> = settings
        .textures_unwanted_formats
        .iter()
        .map(|&format| format_name(format).into())
        .collect();
    textures.set_unwanted(ModelRc::new(VecModel::from(names)));
}

/// Reads the settings model back from the advanced groups, over `current`, the
/// settings last loaded, which keep the Profile Capabilities the window does not
/// edit.
pub(crate) fn read_settings(ui: &MainWindow, current: &ProfileSettings) -> ProfileSettings {
    let mut settings = current.clone();

    let bsa = ui.global::<BsaTab>();
    if let Some(&game) = item(&GAMES, bsa.get_game_index()) {
        settings.bsa_game = game;
    }
    settings.max_bsa_uncompressed_size = gigabytes(bsa.get_max_size()) * GIGABYTE;

    let meshes = ui.global::<MeshesTab>();
    if let Some(&user) = item(&MESH_USERS, meshes.get_user_index()) {
        settings.meshes_user = user;
    }
    if let Some(&stream) = item(&MESH_STREAMS, meshes.get_stream_index()) {
        settings.meshes_stream = stream;
    }
    if let Some(&version) = item(&MESH_VERSIONS, meshes.get_version_index()) {
        settings.meshes_file_version = version;
    }

    let textures = ui.global::<TexturesTab>();
    if let Some(&format) = item(&OUTPUT_FORMATS, textures.get_output_format_index()) {
        settings.textures_format = format;
    }
    settings.textures_convert_tga = textures.get_tga_conversion();
    settings.textures_compress_interface = textures.get_compress_interface();
    // C++ maps the listed names back, dropping repeats and names it does not know.
    let mut unwanted = Vec::new();
    for name in textures.get_unwanted().iter() {
        if let Some(format) = format_number(&name)
            && !unwanted.contains(&format)
        {
            unwanted.push(format);
        }
    }
    settings.textures_unwanted_formats = unwanted;
    settings
}

/// The maximum size the spin box shows, as the `double` QDoubleSpinBox holds:
/// rounded to its two decimals. Slint's `float` is `f32`, so going through the
/// two-decimal text gives back the same `f64` C++ multiplied.
fn gigabytes(shown: f32) -> f64 {
    format!("{shown:.2}").parse().unwrap_or_default()
}

/// The index of `value` among a combo's item data.
fn index_of<T: PartialEq>(items: &[T], value: &T) -> Option<i32> {
    items
        .iter()
        .position(|item| item == value)
        .and_then(|index| i32::try_from(index).ok())
}

/// The item data at a combo index; `None` for no selection.
fn item<T>(items: &[T], index: i32) -> Option<&T> {
    usize::try_from(index)
        .ok()
        .and_then(|index| items.get(index))
}

/// A model number clamped into a spin box's range, as QSpinBox::setValue clamps.
fn clamp(value: u32, (minimum, maximum): (u32, u32)) -> i32 {
    // The ranges are far below `i32::MAX`.
    value.clamp(minimum, maximum) as i32
}

/// A spin box value as the unsigned model number; spin boxes never go negative.
fn unsigned(value: i32) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

/// Qt's `QDir::cleanPath`, which C++ applied to the typed folder: separators
/// become `/`, repeated separators and `.` segments go, `..` removes the segment
/// before it, and a trailing separator goes unless it is the root.
pub(crate) fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let path = path.replace('\\', "/");
    // A UNC path keeps its leading `//`; any other absolute path keeps one `/`.
    let (prefix, rest) = if let Some(rest) = path.strip_prefix("//") {
        ("//", rest)
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/", rest)
    } else {
        ("", path.as_str())
    };

    let mut segments: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => match segments.last() {
                // A drive (`C:`) is the root of a relative-looking path.
                Some(&last) if last != ".." && !(segments.len() == 1 && is_drive(last)) => {
                    segments.pop();
                }
                // Above the root of an absolute path, `..` has nowhere to go.
                _ if !prefix.is_empty() || segments.first().is_some_and(|s| is_drive(s)) => {}
                _ => segments.push(".."),
            },
            segment => segments.push(segment),
        }
    }

    let mut cleaned = format!("{prefix}{}", segments.join("/"));
    // `C:` alone is the drive's root, which Qt writes as `C:/` when the input had it.
    if segments.len() == 1 && is_drive(segments[0]) && rest.len() > 2 {
        cleaned.push('/');
    }
    if cleaned.is_empty() {
        // Only separators and `.`: Qt gives `/` for an absolute path, else `.`.
        return if prefix.is_empty() {
            ".".into()
        } else {
            prefix.into()
        };
    }
    cleaned
}

/// Whether a path segment is a drive, such as `C:`.
fn is_drive(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}
