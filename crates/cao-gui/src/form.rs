//! Moves a profile's models into the window's controls and back (C++
//! `OptionsCAO::saveToUi`/`readFromUi` and `Profiles::saveToUi`/`readFromUi`).
//!
//! Each direction was ported from C++ rule for rule, quirks included, so a
//! profile the C++ build saved reloads the same, and what the window saves
//! reloads the same. #509's UI rules (Dry Run, Several Mods, FO4 textures; `rules.rs`) are
//! not applied here; they run after [`show_options`] and [`show_settings`].

use std::num::NonZeroU64;

use cao_archive::Game;
use cao_core::routing::{ProfileCapabilities, ProfileCapability};
use cao_gui_ui::{AnimationsTab, BsaTab, MainWindow, MeshesTab, TexturesTab};
use cao_profiles::ArchiveSize;
use cao_profiles::{
    ClosedSet, MeshFileVersion, MeshStreamVersion, MeshUserVersion, OptimizationMode, Options,
    ProfileSettings, TextureFormat, UnwantedFormat,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// C++ `GigaByte`: the maximum archive size is stored in bytes, shown in GB.
const GIGABYTE: f64 = 1024.0 * 1024.0 * 1024.0;

/// The mode combo's modes, by index (C++ `modeChooserComboBox` item data).
const MODES: [OptimizationMode; 2] = [OptimizationMode::SingleMod, OptimizationMode::SeveralMods];
/// `TexturesTab.resize-mode` of the "By ratio" Downsizing radio.
const RESIZE_BY_RATIO: i32 = 0;
/// `TexturesTab.resize-mode` of the "By fixed size" Downsizing radio.
const RESIZE_BY_SIZE: i32 = 1;

/// QSpinBox ranges of the Downsizing spin boxes, which clamp what they are set to.
const RATIO_RANGE: (u32, u32) = (2, 10);
const SIZE_RANGE: (u32, u32) = (4, 4096);

/// The tab bar's tabs, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Bsa,
    Meshes,
    Textures,
    Animations,
    Log,
}

impl Tab {
    /// Every tab, in the bar's order.
    pub(crate) const ALL: [Tab; 5] = [
        Tab::Bsa,
        Tab::Meshes,
        Tab::Textures,
        Tab::Animations,
        Tab::Log,
    ];

    /// The id the tab bar's `TabInfo` carries, which `main.slint` matches
    /// against `current-tab` to show the tab's page.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Tab::Bsa => "bsa",
            Tab::Meshes => "meshes",
            Tab::Textures => "textures",
            Tab::Animations => "animations",
            Tab::Log => "log",
        }
    }

    /// The title the tab bar shows.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Tab::Bsa => "BSA",
            Tab::Meshes => "Meshes",
            Tab::Textures => "Textures",
            Tab::Animations => "Animations",
            Tab::Log => "Log",
        }
    }

    /// The tab whose [`Tab::id`] is `id`, if any.
    pub(crate) fn from_id(id: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|tab| tab.id() == id)
    }

    /// Whether the loaded Profile's Profile Capabilities enable this tab, as
    /// the tab bar shows it; the same answer gates the asset options
    /// [`read_options`] reads. `None` stands for a profile whose `profile.ini`
    /// could not be read, which enables no asset tab. The Log tab is always
    /// enabled.
    ///
    /// A tab is enabled by any capability its options drive. Mesh Reference
    /// Maintenance does not enable the Meshes tab: Texture conversion drives
    /// it, and nothing on that tab affects it.
    pub(crate) fn enabled_by(self, capabilities: Option<&ProfileCapabilities>) -> bool {
        let supports_any = |wanted: &[ProfileCapability]| {
            capabilities.is_some_and(|capabilities| {
                wanted
                    .iter()
                    .any(|capability| capabilities.supports(*capability))
            })
        };
        match self {
            Tab::Bsa => supports_any(&[
                ProfileCapability::ArchiveExtraction,
                ProfileCapability::ArchiveCreation,
            ]),
            Tab::Meshes => supports_any(&[
                ProfileCapability::StandardMeshOptimization,
                ProfileCapability::TerrainMeshOptimization,
            ]),
            Tab::Textures => supports_any(&[ProfileCapability::NativeTextureOptimization]),
            Tab::Animations => supports_any(&[ProfileCapability::AnimationOptimization]),
            Tab::Log => true,
        }
    }
}

/// The mode the mode combo shows at `index`; `None` for no selection.
pub(crate) fn mode_at(index: i32) -> Option<OptimizationMode> {
    item(&MODES, index).copied()
}

/// The game the Game combo shows at `index`; `None` for no selection.
pub(crate) fn game_at(index: i32) -> Option<Game> {
    item(Game::ALL, index).copied()
}

/// Fills the Game, Version, User, Stream and Output format combos with every
/// member of the typed Profile model's sets, in [`ClosedSet::ALL`] order, so a
/// combo index is an index into that set and every value a Profile can hold has
/// an item.
pub(crate) fn show_choices(ui: &MainWindow) {
    ui.global::<BsaTab>().set_games(choices(game_label));
    let meshes = ui.global::<MeshesTab>();
    meshes.set_versions(choices(version_label));
    meshes.set_users(choices(user_label));
    meshes.set_streams(choices(stream_label));
    ui.global::<TexturesTab>()
        .set_output_formats(choices(output_format_label));
}

/// A combo model holding `label` of each member of `T`, in [`ClosedSet::ALL`]
/// order.
fn choices<T: ClosedSet, L: Into<SharedString>>(label: fn(T) -> L) -> ModelRc<SharedString> {
    let labels: Vec<SharedString> = T::ALL.iter().map(|&member| label(member).into()).collect();
    ModelRc::new(VecModel::from(labels))
}

/// The Game combo's text for a game.
fn game_label(game: Game) -> &'static str {
    match game {
        Game::Tes5 => "Skyrim LE",
        Game::Sse => "SSE",
        Game::Fo4 => "FO4",
    }
}

/// The Version combo's text for a NIF file version: the games that use it,
/// then the version as nifly names it (`V20_2_0_7`).
fn version_label(version: MeshFileVersion) -> String {
    let games = match version {
        MeshFileVersion::V20_0_0_5 => "Oblivion",
        MeshFileVersion::V20_2_0_7 => "Fallout 3 / Skyrim / Skyrim SE / Fallout 4",
    };
    format!("{games} (V{})", version.dotted().replace('.', "_"))
}

/// The User combo's text for a NIF user version: the games that use it.
fn user_label(user: MeshUserVersion) -> &'static str {
    match user {
        MeshUserVersion::V11 => "Oblivion (11)",
        MeshUserVersion::V12 => "Fallout 3 / Skyrim / Skyrim SE / Fallout 4 (12)",
    }
}

/// The Stream combo's text for a NIF stream version: the games that use it.
fn stream_label(stream: MeshStreamVersion) -> &'static str {
    match stream {
        MeshStreamVersion::V82 => "Oblivion / Fallout 3 (82)",
        MeshStreamVersion::V83 => "Skyrim (83)",
        MeshStreamVersion::V100 => "Skyrim SE (100)",
        MeshStreamVersion::V130 => "Fallout 4 (130)",
    }
}

/// The Output format combo's text for a Texture format: its kind, then its
/// `DXGI_FORMAT` name.
fn output_format_label(format: TextureFormat) -> String {
    let kind = match format {
        TextureFormat::Bc7Unorm => "BC7",
        TextureFormat::Bc5Unorm => "BC5",
        TextureFormat::Bc3Unorm => "BC3",
        TextureFormat::Bc1Unorm => "BC1",
        TextureFormat::R8G8B8A8Unorm => "Uncompressed",
    };
    format!("{kind} ({})", format.name())
}

/// The combo index of `member`, which [`show_choices`] gave an item.
fn choice_index<T: ClosedSet>(member: T) -> i32 {
    index_of(T::ALL, &member).expect("every member of a closed set is in its `ALL`")
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
        textures.set_resize_mode(RESIZE_BY_RATIO);
    } else if options.textures_resize_size {
        textures.set_resize_mode(RESIZE_BY_SIZE);
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
    ui.set_mode_index(index_of(&MODES, &options.mode).unwrap_or(-1));
    ui.set_user_path(options.user_path.as_str().into());
}

/// Reads the options model back from the window, over `current`: the options
/// last loaded, which keep what the window does not show (`bBsaProcessContent`,
/// and the mesh level when no level radio is checked).
///
/// `capabilities` are the loaded Profile's Profile Capabilities, or `None`
/// when its `profile.ini` could not be read. Known capabilities gate the asset
/// options: those on a tab the profile does not support read as off. Unknown
/// capabilities keep every BSA, Texture, Mesh and Animation option of `current`,
/// and only Dry Run, the mod path, the mode and the debug log are read (#580).
pub(crate) fn read_options(
    ui: &MainWindow,
    current: &Options,
    capabilities: Option<&ProfileCapabilities>,
) -> Options {
    let mut options = current.clone();

    options.dry_run = ui.get_dry_run();
    options.user_path = clean_path(&ui.get_user_path());
    // No selection reads as Single Mod, as an invalid `QVariant` converts to 0.
    options.mode = mode_at(ui.get_mode_index()).unwrap_or(OptimizationMode::SingleMod);
    options.debug_log = ui.get_debug_log();

    // An unreadable profile shows every asset tab disabled, which the gating
    // below would read as "every option off" and so wipe `settings.ini`. Not
    // knowing what the profile can do is no reason to forget what it had.
    let Some(capabilities) = capabilities else {
        return options;
    };
    let tab = |tab: Tab| tab.enabled_by(Some(capabilities));

    // The base group is disabled while Dry Run is on (`rules.rs`), and C++
    // reads every BSA option as off while the tab or that group is disabled.
    let bsa = ui.global::<BsaTab>();
    let bsa_enabled = tab(Tab::Bsa) && !ui.get_dry_run();
    options.bsa_extract = bsa_enabled && bsa.get_extract();
    options.bsa_create = bsa_enabled && bsa.get_create();
    options.bsa_delete_backup = bsa_enabled && bsa.get_delete_backups();
    options.bsa_merge_incompressible = bsa_enabled && !bsa.get_separate_incompressible();
    options.bsa_merge_textures = bsa_enabled && !bsa.get_separate_textures();
    options.bsa_create_dummies = bsa_enabled && bsa.get_create_dummies();
    options.bsa_compress = bsa_enabled && bsa.get_compress();
    options.bsa_delete_source = bsa_enabled && bsa.get_delete_source();

    let textures = ui.global::<TexturesTab>();
    let textures_enabled = textures.get_process() && tab(Tab::Textures);
    options.textures_necessary = textures_enabled && textures.get_necessary();
    options.textures_mipmaps = textures_enabled && textures.get_mipmaps();
    options.textures_compress = textures_enabled && textures.get_compress();

    let resizing = textures.get_resize() && tab(Tab::Textures);
    options.textures_resize_size = resizing && textures.get_resize_mode() == RESIZE_BY_SIZE;
    options.textures_target_width = unsigned(textures.get_size_width());
    options.textures_target_height = unsigned(textures.get_size_height());
    options.textures_resize_ratio = resizing && textures.get_resize_mode() == RESIZE_BY_RATIO;
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
    if !meshes.get_process() || !tab(Tab::Meshes) {
        options.meshes_optimization_level = 0;
    }
    options.meshes_headparts = tab(Tab::Meshes) && meshes.get_headparts();
    options.meshes_resave = tab(Tab::Meshes) && meshes.get_resave();

    options.animations_optimization =
        tab(Tab::Animations) && ui.global::<AnimationsTab>().get_necessary();
    options
}

/// Shows the settings model (`profile.ini`) in the advanced groups. The Game,
/// Mesh Target and Output format combos hold every value the typed model can,
/// so each always selects the loaded value, and every unwanted format has the
/// name the window lists it by.
pub(crate) fn show_settings(ui: &MainWindow, settings: &ProfileSettings) {
    let bsa = ui.global::<BsaTab>();
    let requested = settings.max_bsa_uncompressed_size.requested();
    bsa.set_archive_limit_given(requested.is_some());
    // A size not given shows the game's default, which the Archive Limit rule
    // (`rules.rs`) fills in from the Game combo; 0 only holds its place.
    bsa.set_archive_limit(requested.map_or(0.0, gigabytes_of));
    bsa.set_game_index(choice_index(settings.bsa_game));

    let meshes = ui.global::<MeshesTab>();
    meshes.set_user_index(choice_index(settings.meshes_user));
    meshes.set_stream_index(choice_index(settings.meshes_stream));
    meshes.set_version_index(choice_index(settings.meshes_file_version));

    let textures = ui.global::<TexturesTab>();
    textures.set_output_format_index(choice_index(settings.textures_format));
    textures.set_tga_conversion(settings.textures_convert_tga);
    textures.set_compress_interface(settings.textures_compress_interface);
    let names: Vec<SharedString> = settings
        .textures_unwanted_formats
        .iter()
        .map(|format| format.name().into())
        .collect();
    textures.set_unwanted(ModelRc::new(VecModel::from(names)));
}

/// Reads the settings model back from the advanced groups, over `current`, the
/// settings last loaded, which keep the Profile Capabilities the window does not
/// edit.
pub(crate) fn read_settings(ui: &MainWindow, current: &ProfileSettings) -> ProfileSettings {
    let mut settings = current.clone();

    let bsa = ui.global::<BsaTab>();
    if let Some(&game) = item(Game::ALL, bsa.get_game_index()) {
        settings.bsa_game = game;
    }
    // A size not given shows the game's default until the user edits it, and
    // is kept not given: saving the default would stop the Profile following
    // its game's default.
    //
    // The spin box's two decimals of a gigabyte cannot hold every size, and a
    // size written with a unit keeps its text only while it is unchanged, so a
    // size the user did not change is kept as loaded rather than re-derived.
    // Compared as the spin box displays them, so its own rounding of the
    // loaded value does not count as a change.
    if bsa.get_archive_limit_given() {
        let shown = gigabytes(bsa.get_archive_limit());
        let loaded = current
            .max_bsa_uncompressed_size
            .requested()
            .map(|bytes| gigabytes(gigabytes_of(bytes)));
        if loaded != Some(shown) {
            settings.max_bsa_uncompressed_size = archive_size(shown);
        }
    }

    let meshes = ui.global::<MeshesTab>();
    if let Some(&user) = item(MeshUserVersion::ALL, meshes.get_user_index()) {
        settings.meshes_user = user;
    }
    if let Some(&stream) = item(MeshStreamVersion::ALL, meshes.get_stream_index()) {
        settings.meshes_stream = stream;
    }
    if let Some(&version) = item(MeshFileVersion::ALL, meshes.get_version_index()) {
        settings.meshes_file_version = version;
    }

    let textures = ui.global::<TexturesTab>();
    if let Some(&format) = item(TextureFormat::ALL, textures.get_output_format_index()) {
        settings.textures_format = format;
    }
    settings.textures_convert_tga = textures.get_tga_conversion();
    settings.textures_compress_interface = textures.get_compress_interface();
    // The list only ever holds names `show_settings` and the dialog put there,
    // so every name maps back; a repeat is dropped by the same rule the read of
    // `profile.ini` applies, so the saved list loads back unchanged.
    settings.textures_unwanted_formats = UnwantedFormat::first_occurrences(
        textures
            .get_unwanted()
            .iter()
            .filter_map(|name| UnwantedFormat::from_name(&name)),
    );
    settings
}

/// The maximum size the spin box shows, as the `double` QDoubleSpinBox holds:
/// rounded to its two decimals. Slint's `float` is `f32`, so going through the
/// two-decimal text gives back the same `f64` C++ multiplied.
fn gigabytes(shown: f32) -> f64 {
    format!("{shown:.2}").parse().unwrap_or_default()
}

/// `bytes` in the spin box's gigabytes (1024³ bytes each).
pub(crate) fn gigabytes_of(bytes: u64) -> f32 {
    (bytes as f64 / GIGABYTE) as f32
}

/// The maximum Archive size `shown` gigabytes stand for, as the spin box
/// displays them ([`gigabytes`]), truncated to whole bytes as the read of
/// `profile.ini` truncates a fraction. The spin box holds 0.01 to 99.99, so
/// the bytes always fit and are never 0.
fn archive_size(shown: f64) -> ArchiveSize {
    // `profile.ini` refuses an explicit `0`. The spin box's 0.01 GB minimum
    // keeps an edited size well above it, so 1 byte only stands in for a
    // value the box could never have shown.
    let bytes = NonZeroU64::new((shown * GIGABYTE) as u64).unwrap_or(NonZeroU64::MIN);
    ArchiveSize::from_bytes(bytes)
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
