//! The deviation guard (#473, #499): keeps every corpus case free of deviation
//! triggers, so a fix is never reported as a regression.
//!
//! The spec's deviation list (#476) names the C++ behaviours the Rust port
//! fixes rather than copies. Each one is pinned by a Rust-only test instead, so
//! a corpus case that would make one observable must never be compared: both
//! builds would be right, and the comparator would still call it Different.
//!
//! - [`DEVIATIONS`] transcribes the list, and [`RULES`] holds exactly one rule
//!   per entry; a unit test fails when an entry lacks its rule.
//! - Every rule mirrors the **C++** condition, because the Rust predicates all
//!   implement the fixed behaviour. A rule fires only where the two builds
//!   would disagree, or conservatively wider when the exact C++ condition
//!   depends on run state the recipe cannot fix (such as whether Archive
//!   creation reaches pruning).
//! - A trigger is a [`HarnessError::DeviationTrigger`], never a verdict.
//! - Rules read the case's whole environment, not just its recipe: the absolute
//!   paths of the side roots under the work directory (C++ matches several
//!   rules against absolute paths), and the profile files both sides are
//!   provisioned from.
//! - Separately, a profile value no GUI widget can produce is a
//!   [`HarnessError::UnreachableProfileValue`].
//!
//! [`check`] runs before anything of the case is written, so a rejected case
//! never reaches either build.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cao_archive::{FilePath, Game, NameKind, Settings};
use cao_profiles::{IniFile, Value};
use directxtex::DXGI_FORMAT;

use crate::HarnessError;
use crate::case::{CaseFile, ModSelection, Side};
use crate::materialise::{is_device_name, raw_bytes};
use crate::recipe::{
    ArchiveGame, ArchiveKind, ArchiveRecipe, ContentEntry, FsShape, OutputFormat, TextureFormat,
};

/// The spec's deviation list (#476, "Deviation list (fix, don't copy)"), in
/// order: entry `n` is at index `n - 1`.
pub const DEVIATIONS: [&str; 25] = [
    "`profiles/`, `logs/`, `bin/hkxcmd.exe` and `translations/` resolve relative to the exe",
    "Cancel in the unwanted-formats dialog reverts edits",
    "The Dry Run and Several Mods UI rules also apply on INI load",
    "The translation loader bug",
    "No fixed-size path buffer overflow in the texture loader",
    "The texture metadata check really compares",
    "FO4 DX10 cubemaps in BC7, BC6H and sRGB extract successfully",
    "An Archive over 4 GiB errors instead of being written corrupt",
    "All-digit plugin stems are handled",
    "Dead data files are left alone",
    "`bsaGame` outside 3/4/5 makes the profile unreadable",
    "A scalar where a list is expected reads as a one-element list",
    "A `#` line is a comment",
    "A profile INI without a BOM decodes as UTF-8 when valid, Latin-1 otherwise",
    "Device-name detection trims trailing spaces and dots from the stem",
    "Packing Exclusions match within the Mod Root",
    "Headpart Meshes: facegen in Dry Run, matching within the Mod Root, no staging scan, a safe HDPT parser, compressed records",
    "Resize target width and height are validated only when resizing by size is enabled",
    "A Several Mods child named `.cao-staging…` is never a Mod Root",
    "A separator Mod Exclusion is a child name ending in `_separator`; pruning has no separator rule",
    "FO4 never merges textures into `GNRL`, and \"create texture archive\" is forced on",
    "Application Log format: `{module::path@line}`, `fatal` as `ERROR`, HTML-escaped text, `\\n` per record",
    "Toggling debug logging between runs no longer drops the log",
    "The Log tab is fed by the sink, isn't truncated, and shows startup records",
    "Replacing the staging manifest waits out a destination a scanner briefly holds",
];

/// One guard rule: the trigger of one deviation-list entry.
pub struct Rule {
    /// The entry's number on [`DEVIATIONS`].
    pub deviation: u8,
    /// A short name, reported with every trigger.
    pub name: &'static str,
    /// What in the case triggers the deviation, if anything does.
    check: fn(&Context<'_>) -> Option<String>,
}

/// One rule per [`DEVIATIONS`] entry, in list order.
pub const RULES: [Rule; 25] = [
    Rule {
        deviation: 1,
        name: "app-directory resources",
        check: app_directory_resources,
    },
    Rule {
        deviation: 2,
        name: "formats dialog commit",
        check: formats_dialog_commit,
    },
    Rule {
        deviation: 3,
        name: "UI rules on load",
        check: ui_rules_on_load,
    },
    Rule {
        deviation: 4,
        name: "translations folder",
        check: translations_folder,
    },
    Rule {
        deviation: 5,
        name: "texture path buffer",
        check: texture_path_buffer,
    },
    Rule {
        deviation: 6,
        name: "texture metadata check",
        check: texture_metadata_check,
    },
    Rule {
        deviation: 7,
        name: "FO4 DX10 cubemap",
        check: fo4_dx10_cubemap,
    },
    Rule {
        deviation: 8,
        name: "Archive over 4 GiB",
        check: archive_over_4_gib,
    },
    Rule {
        deviation: 9,
        name: "all-digit plugin stem",
        check: all_digit_stem,
    },
    Rule {
        deviation: 10,
        name: "dead data",
        check: dead_data,
    },
    Rule {
        deviation: 11,
        name: "bsaGame",
        check: bsa_game,
    },
    Rule {
        deviation: 12,
        name: "scalar list",
        check: scalar_list,
    },
    Rule {
        deviation: 13,
        name: "`#` comment",
        check: hash_comment,
    },
    Rule {
        deviation: 14,
        name: "INI encoding",
        check: ini_encoding,
    },
    Rule {
        deviation: 15,
        name: "device stem trailing space",
        check: device_stem_trailing_space,
    },
    Rule {
        deviation: 16,
        name: "Packing Exclusion scope",
        check: packing_exclusion_scope,
    },
    Rule {
        deviation: 17,
        name: "Headpart Meshes",
        check: headpart_meshes,
    },
    Rule {
        deviation: 18,
        name: "resize validation",
        check: resize_validation,
    },
    Rule {
        deviation: 19,
        name: "staging Mod Root",
        check: staging_mod_root,
    },
    Rule {
        deviation: 20,
        name: "separator",
        check: separator,
    },
    Rule {
        deviation: 21,
        name: "FO4 merged textures",
        check: fo4_merged_textures,
    },
    Rule {
        deviation: 22,
        name: "log escaping",
        check: log_escaping,
    },
    Rule {
        deviation: 23,
        name: "debug log",
        check: debug_log,
    },
    Rule {
        deviation: 24,
        name: "logs folder",
        check: logs_folder,
    },
    Rule {
        deviation: 25,
        name: "held staging manifest",
        check: held_staging_manifest,
    },
];

/// What the guard checks a case against.
pub struct GuardInput<'a> {
    pub case: &'a CaseFile,
    /// The case directory, `<work>/<case-id>`, whose `oracle/` and `rust/`
    /// folders the builds run in. Nothing need exist there yet.
    pub case_root: &'a Path,
    /// The `profiles/` folder both sides are provisioned from.
    pub profiles: &'a Path,
    /// Where `raw` entries' fixture files live.
    pub fixtures: &'a Path,
}

/// Rejects a case holding any deviation trigger or GUI-unreachable profile
/// value, with the first one found.
///
/// # Errors
/// [`HarnessError::DeviationTrigger`] or
/// [`HarnessError::UnreachableProfileValue`] for a rejected case;
/// [`HarnessError::Io`] when a profile file exists but cannot be read.
pub fn check(input: &GuardInput<'_>) -> Result<(), HarnessError> {
    match rejections(input)?.into_iter().next() {
        Some(rejection) => Err(rejection),
        None => Ok(()),
    }
}

/// Every reason the guard rejects a case: one
/// [`HarnessError::DeviationTrigger`] per rule that fires, in [`RULES`]
/// order, then one [`HarnessError::UnreachableProfileValue`] per such value.
///
/// # Errors
/// [`HarnessError::Io`] when a profile file exists but cannot be read.
pub fn rejections(input: &GuardInput<'_>) -> Result<Vec<HarnessError>, HarnessError> {
    let context = Context::new(input)?;
    let mut found: Vec<HarnessError> = RULES
        .iter()
        .filter_map(|rule| {
            (rule.check)(&context).map(|detail| HarnessError::DeviationTrigger {
                deviation: rule.deviation,
                rule: rule.name,
                detail,
            })
        })
        .collect();
    found.extend(unreachable_profile_values(&context));
    Ok(found)
}

/// The formats the unwanted-formats dialog lists, in its order
/// (`src/texturesformats.h`): no typeless, depth/stencil or paletted formats.
const DIALOG_FORMATS: [&str; 75] = [
    "R32G32B32A32_FLOAT",
    "R32G32B32A32_UINT",
    "R32G32B32A32_SINT",
    "R32G32B32_FLOAT",
    "R32G32B32_UINT",
    "R32G32B32_SINT",
    "R16G16B16A16_FLOAT",
    "R16G16B16A16_UNORM",
    "R16G16B16A16_UINT",
    "R16G16B16A16_SNORM",
    "R16G16B16A16_SINT",
    "R32G32_FLOAT",
    "R32G32_UINT",
    "R32G32_SINT",
    "R10G10B10A2_UNORM",
    "R10G10B10A2_UINT",
    "R11G11B10_FLOAT",
    "R8G8B8A8_UNORM",
    "R8G8B8A8_UNORM_SRGB",
    "R8G8B8A8_UINT",
    "R8G8B8A8_SNORM",
    "R8G8B8A8_SINT",
    "R16G16_FLOAT",
    "R16G16_UNORM",
    "R16G16_UINT",
    "R16G16_SNORM",
    "R16G16_SINT",
    "R32_FLOAT",
    "R32_UINT",
    "R32_SINT",
    "R8G8_UNORM",
    "R8G8_UINT",
    "R8G8_SNORM",
    "R8G8_SINT",
    "R16_FLOAT",
    "R16_UNORM",
    "R16_UINT",
    "R16_SNORM",
    "R16_SINT",
    "R8_UNORM",
    "R8_UINT",
    "R8_SNORM",
    "R8_SINT",
    "A8_UNORM",
    "R9G9B9E5_SHAREDEXP",
    "R8G8_B8G8_UNORM",
    "G8R8_G8B8_UNORM",
    "BC1_UNORM",
    "BC1_UNORM_SRGB",
    "BC2_UNORM",
    "BC2_UNORM_SRGB",
    "BC3_UNORM",
    "BC3_UNORM_SRGB",
    "BC4_UNORM",
    "BC4_SNORM",
    "BC5_UNORM",
    "BC5_SNORM",
    "B5G6R5_UNORM",
    "B5G5R5A1_UNORM",
    "B8G8R8A8_UNORM",
    "B8G8R8X8_UNORM",
    "R10G10B10_XR_BIAS_A2_UNORM",
    "B8G8R8A8_UNORM_SRGB",
    "B8G8R8X8_UNORM_SRGB",
    "BC6H_UF16",
    "BC6H_SF16",
    "BC7_UNORM",
    "BC7_UNORM_SRGB",
    "AYUV",
    "Y410",
    "Y416",
    "YUY2",
    "Y210",
    "Y216",
    "B4G4R4A4_UNORM",
];

/// A format's short JSON name, such as `BC7_UNORM`; an unknown number reads as
/// `0x…`, which no list holds.
fn format_name(format: u32) -> String {
    String::from(TextureFormat(DXGI_FORMAT::from(format)))
}

/// One path the case's tree holds once materialised and, for Archive
/// content, once extracted beside its Archive.
struct Item<'a> {
    /// Relative to a side's case root, `/`-separated.
    path: String,
    directory: bool,
    /// The content entry that writes it, when one does.
    entry: Option<&'a ContentEntry>,
}

/// Everything the rules read, gathered once.
struct Context<'a> {
    case: &'a CaseFile,
    fixtures: &'a Path,
    /// Each side's case root, absolute.
    side_roots: Vec<PathBuf>,
    /// The same roots, `/`-separated and lowercased, for C++'s
    /// case-insensitive matching against absolute paths.
    side_texts: Vec<String>,
    items: Vec<Item<'a>>,
    /// The Mod Roots C++ would process, relative to a side's case root.
    mod_roots: Vec<String>,
    /// The Several Mods selection's directory children, before exclusions.
    children: Vec<String>,
    /// The INI files the builds read, by their path under `profiles/`.
    ini_files: Vec<(String, Vec<u8>)>,
    profile_ini: IniFile,
    settings_ini: IniFile,
    /// The `FilesToNotPack.txt` rules, as C++ reads them, `/`-separated and
    /// lowercased.
    packing_rules: Vec<String>,
}

impl<'a> Context<'a> {
    /// Gathers what every rule reads: the absolute side roots under
    /// `input.case_root`, the tree's paths and Mod Roots, and the profile's
    /// INI files and Packing Exclusions as both sides will be provisioned with
    /// them. A missing profile file reads as empty.
    ///
    /// # Errors
    /// [`HarnessError::Io`] when a side root cannot be made absolute, or a
    /// profile file exists but cannot be read.
    fn new(input: &GuardInput<'a>) -> Result<Self, HarnessError> {
        let case = input.case;
        let mut side_roots = Vec::new();
        for side in [Side::Oracle, Side::Rust] {
            let root = input.case_root.join(side.name());
            side_roots.push(std::path::absolute(&root).map_err(|error| {
                HarnessError::io(format!("resolving {}", root.display()), error)
            })?);
        }
        let side_texts = side_roots.iter().map(|root| normalised(root)).collect();

        let items = items(case);
        let (mod_roots, children) = mod_roots(&case.spec.mod_selection, &items);

        let profile = input.profiles.join(&case.spec.profile);
        let mut ini_files = Vec::new();
        for (name, path) in [
            ("common.ini".to_owned(), input.profiles.join("common.ini")),
            (
                format!("{}/profile.ini", case.spec.profile),
                profile.join("profile.ini"),
            ),
            (
                format!("{}/settings.ini", case.spec.profile),
                profile.join("settings.ini"),
            ),
        ] {
            if let Some(bytes) = read_optional(&path)? {
                ini_files.push((name, bytes));
            }
        }
        let parsed = |suffix: &str| {
            ini_files
                .iter()
                .find(|(name, _)| name.ends_with(suffix))
                .map_or_else(IniFile::new, |(_, bytes)| IniFile::parse(bytes))
        };
        let (profile_ini, settings_ini) = (parsed("/profile.ini"), parsed("/settings.ini"));

        // The profile's own file, else SSE's, as both builds fall back.
        let mut packing_rules = Vec::new();
        for directory in [profile, input.profiles.join("SSE")] {
            if let Some(bytes) = read_optional(&directory.join("FilesToNotPack.txt"))? {
                packing_rules = packing_rule_lines(&bytes);
                break;
            }
        }

        Ok(Self {
            case,
            fixtures: input.fixtures,
            side_roots,
            side_texts,
            items,
            mod_roots,
            children,
            ini_files,
            profile_ini,
            settings_ini,
            packing_rules,
        })
    }

    /// The bytes a `text` or `raw` entry writes, when they can be known
    /// without building anything. A broken `raw` entry has none; the
    /// materialiser reports it.
    fn bytes(&self, entry: &ContentEntry) -> Option<Vec<u8>> {
        match entry {
            ContentEntry::Text { text, .. } => Some(text.as_bytes().to_vec()),
            ContentEntry::Raw {
                base64, fixture, ..
            } => raw_bytes(base64.as_deref(), fixture.as_deref(), self.fixtures).ok(),
            _ => None,
        }
    }

    /// The items below each Mod Root: the Mod Root and the path within it.
    fn in_mod_roots(&self) -> impl Iterator<Item = (&str, &str, &Item<'a>)> {
        self.items.iter().flat_map(move |item| {
            self.mod_roots.iter().filter_map(move |root| {
                within(&item.path, root).map(|relative| (root.as_str(), relative, item))
            })
        })
    }

    /// The `[BSA] bsaGame` number, as both builds convert it.
    fn bsa_game(&self) -> i32 {
        self.profile_ini.value("BSA/bsaGame").to_i32()
    }
}

/// `path` as an absolute, `/`-separated, lowercased string with no trailing `/`.
fn normalised(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Reads a file that may be missing.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, HarnessError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(HarnessError::io(
            format!("reading {}", path.display()),
            error,
        )),
    }
}

/// `FilesToNotPack.txt` as C++ reads it (`QString::simplified`, skipping empty
/// and `#` lines), with `\` turned into `/` and lowercased for C++'s
/// case-insensitive match.
fn packing_rule_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.replace('\\', "/").to_lowercase())
        .collect()
}

/// Every path the tree will hold: content entries, the files Archives extract
/// beside themselves, and the paths filesystem-shape operations create.
fn items(case: &CaseFile) -> Vec<Item<'_>> {
    let mut items = Vec::new();
    for entry in &case.tree.content {
        items.push(Item {
            path: entry.path().to_owned(),
            directory: matches!(entry, ContentEntry::Directory { .. }),
            entry: Some(entry),
        });
        if let ContentEntry::Archive(archive) = entry {
            for packed in &archive.content {
                items.push(Item {
                    path: extracted_path(archive, packed.path()),
                    directory: false,
                    entry: Some(packed),
                });
            }
        }
    }
    for operation in &case.tree.fs_shape {
        if matches!(operation, FsShape::Readonly { .. }) {
            continue;
        }
        items.push(Item {
            path: operation.path().to_owned(),
            directory: matches!(operation, FsShape::Junction { .. }),
            entry: None,
        });
    }
    items
}

/// Where an Archive's packed `game_path` lands when it is extracted.
fn extracted_path(archive: &ArchiveRecipe, game_path: &str) -> String {
    match archive.path.rsplit_once('/') {
        Some((directory, _)) => format!("{directory}/{game_path}"),
        None => game_path.to_owned(),
    }
}

/// The part of `path` strictly below the folder `root`, matched per component
/// ignoring ASCII case, as Windows matches names.
fn within<'p>(path: &'p str, root: &str) -> Option<&'p str> {
    let head = path.get(..root.len())?;
    let rest = path.get(root.len()..)?.strip_prefix('/')?;
    (head.eq_ignore_ascii_case(root) && !rest.is_empty()).then_some(rest)
}

/// Every directory of the tree, as written: each item's ancestors and the
/// items that are directories themselves.
fn directories(items: &[Item<'_>]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for item in items {
        let ancestors = item.path.match_indices('/').map(|(index, _)| index);
        for end in ancestors.chain(item.directory.then_some(item.path.len())) {
            let directory = &item.path[..end];
            if !found
                .iter()
                .any(|known| known.eq_ignore_ascii_case(directory))
            {
                found.push(directory.to_owned());
            }
        }
    }
    found
}

/// The Mod Roots C++ processes, and the Several Mods selection's directory
/// children before any exclusion.
///
/// C++ excludes a Several Mods child whose name contains `separator`
/// (case-sensitive); deviation 20's rule makes sure Rust excludes the same
/// children, so they are left out here. Ignored mods are kept: a rule that
/// fires on one is only conservative.
fn mod_roots(selection: &ModSelection, items: &[Item<'_>]) -> (Vec<String>, Vec<String>) {
    match selection {
        ModSelection::OneMod { folder } => (vec![folder.clone()], Vec::new()),
        ModSelection::SeveralMods { folder } => {
            let children: Vec<String> = directories(items)
                .into_iter()
                .filter(|directory| {
                    within(directory, folder).is_some_and(|rest| !rest.contains('/'))
                })
                .collect();
            let roots = children
                .iter()
                .filter(|child| !last_component(child).contains(SEPARATOR_MARKER))
                .cloned()
                .collect();
            (roots, children)
        }
    }
}

fn last_component(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether `path`'s extension is one of `extensions`, ignoring ASCII case.
fn has_extension(path: &str, extensions: &[&str]) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        extensions
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// The reserved staging namespace, which both builds match ignoring ASCII case.
const STAGING_PREFIX: &str = ".cao-staging";
/// The marker C++ finds in Several Mods child names (case-sensitive) and in
/// pruned paths (any case).
const SEPARATOR_MARKER: &str = "separator";

const PLUGIN_EXTENSIONS: [&str; 3] = ["esp", "esm", "esl"];
pub(crate) const MESH_EXTENSIONS: [&str; 3] = ["nif", "btr", "bto"];
const TEXTURE_EXTENSIONS: [&str; 2] = ["dds", "tga"];

/// The first item whose top-level component is one of `names`, ignoring ASCII
/// case.
fn top_level(context: &Context<'_>, names: &[&str]) -> Option<String> {
    context.items.iter().find_map(|item| {
        let first = item.path.split('/').next().unwrap_or_default();
        names
            .iter()
            .any(|name| first.eq_ignore_ascii_case(name))
            .then(|| {
                format!(
                    "the case tree holds `{}`, beside the side's own `{first}/`",
                    item.path
                )
            })
    })
}

/// Deviation 1. C++ resolves `profiles/` and `bin/hkxcmd.exe` against the
/// working directory, Rust against the app directory; the harness makes both
/// the side root. A case tree using those names at the top would put case
/// content where one build looks for its own resources.
fn app_directory_resources(context: &Context<'_>) -> Option<String> {
    top_level(context, &["profiles", "bin"])
}

/// Deviation 2. The dialog is the only GUI route into the unwanted formats,
/// and C++'s Cancel keeps the edits it reverts in Rust. So the corpus only
/// holds lists an OK commit leaves: each of the dialog's formats at most once,
/// and, for an override (which stands for a dialog edit), in the dialog's
/// order. The profile file's own list keeps its order, as a GUI save without
/// the dialog keeps it.
fn formats_dialog_commit(context: &Context<'_>) -> Option<String> {
    let (formats, from_dialog): (Vec<String>, bool) =
        match &context.case.profile_overrides.unwanted_formats {
            Some(formats) => (
                formats.iter().map(|format| String::from(*format)).collect(),
                true,
            ),
            None => (
                context
                    .profile_ini
                    .value("Textures/texturesUnwantedFormats")
                    .to_int_list()
                    .into_iter()
                    .map(|format| format_name(format as u32))
                    .collect(),
                false,
            ),
        };
    let mut last = None;
    for (index, name) in formats.iter().enumerate() {
        let Some(position) = DIALOG_FORMATS.iter().position(|known| known == name) else {
            return Some(format!(
                "unwanted format `{name}` is not in the dialog's list"
            ));
        };
        if formats[..index].contains(name) {
            return Some(format!("unwanted format `{name}` is listed twice"));
        }
        if from_dialog && last.is_some_and(|last| position < last) {
            return Some(format!(
                "the unwanted formats override puts `{name}` out of the dialog's order"
            ));
        }
        last = Some(position);
    }
    None
}

/// Deviation 3. Rust applies the Dry Run and Several Mods rules whenever
/// settings load; C++ only on a click, so it can run settings Rust never
/// would. Dry Run clears Archive extraction, creation and backup deletion;
/// Several Mods allows only mesh levels 0 and 1 ("Necessary").
fn ui_rules_on_load(context: &Context<'_>) -> Option<String> {
    let spec = &context.case.spec;
    let archives = &spec.archives;
    if spec.dry_run && (archives.extract || archives.create || archives.delete_backup) {
        return Some(
            "a Dry Run with Archive extraction, creation or backup deletion requested".into(),
        );
    }
    if matches!(spec.mod_selection, ModSelection::SeveralMods { .. }) && spec.meshes.level > 1 {
        return Some(format!(
            "Several Mods with mesh level {}, above \"Necessary\"",
            spec.meshes.level
        ));
    }
    None
}

/// Deviation 4. C++ loads `translations/` from the working directory, into
/// the wrong translator; only English ships, so the trigger is a case tree
/// that puts a `translations/` folder where either build looks for one.
fn translations_folder(context: &Context<'_>) -> Option<String> {
    top_level(context, &["translations"])
}

/// The C++ texture loader's `wchar_t fileName[1024]` buffer, which a path of
/// this many UTF-16 units or more overflows.
const TEXTURE_PATH_BUFFER: usize = 1024;

/// Deviation 5. C++ copies a Texture's absolute path into a fixed 1024-unit
/// buffer without a bounds check. Checked against the real side roots, so a
/// deep work directory is caught too.
fn texture_path_buffer(context: &Context<'_>) -> Option<String> {
    context
        .items
        .iter()
        .filter(|item| !item.directory && has_extension(&item.path, &TEXTURE_EXTENSIONS))
        .find_map(|item| {
            context.side_roots.iter().find_map(|root| {
                let absolute = item
                    .path
                    .split('/')
                    .fold(root.clone(), |path, part| path.join(part));
                let units = absolute.as_os_str().encode_wide().count();
                (units >= TEXTURE_PATH_BUFFER).then(|| {
                    format!(
                        "{} is {units} UTF-16 units long, which overflows C++'s \
                         {TEXTURE_PATH_BUFFER}-unit texture path buffer",
                        absolute.display()
                    )
                })
            })
        })
}

/// Deviation 6. C++'s metadata check is a no-op, Rust's compares. No
/// DirectXTex step CAO calls changes a field it does not track for the 2D,
/// array and cube Textures a recipe builds, so only a DDS the recipe did not
/// build can tell the two apart: a `raw` one, which may be a volume texture
/// or carry flags no step keeps. Raw bytes that are not a DDS fail to load in
/// both builds, before any check.
fn texture_metadata_check(context: &Context<'_>) -> Option<String> {
    context.items.iter().find_map(|item| {
        let entry @ ContentEntry::Raw { .. } = item.entry? else {
            return None;
        };
        (has_extension(&item.path, &["dds"]) && context.bytes(entry)?.starts_with(b"DDS ")).then(
            || {
                format!(
                    "`{}` is a DDS written as `raw` bytes, not built by the recipe",
                    item.path
                )
            },
        )
    })
}

/// The formats rsm-bsa can still write a cubemap's DDS header for: those with a
/// legacy `DDS_PIXELFORMAT`. Every other format takes DirectXTex's DX10 path,
/// which rejects the array size rsm-bsa gives a cubemap.
const LEGACY_CUBEMAP_FORMATS: [&str; 10] = [
    "BC1_UNORM",
    "BC2_UNORM",
    "BC3_UNORM",
    "BC4_UNORM",
    "BC4_SNORM",
    "BC5_UNORM",
    "BC5_SNORM",
    "R8G8B8A8_UNORM",
    "B8G8R8A8_UNORM",
    "B8G8R8X8_UNORM",
];

/// Deviation 7. C++ cannot extract an FO4 DX10 cubemap in BC7, BC6H, sRGB or
/// any other format without a legacy DDS header. Conservatively wider than
/// the failing set: only the formats known to extract are allowed.
fn fo4_dx10_cubemap(context: &Context<'_>) -> Option<String> {
    context.case.tree.content.iter().find_map(|entry| {
        let ContentEntry::Archive(archive) = entry else {
            return None;
        };
        if archive.game != ArchiveGame::Fo4 || archive.archive_type != ArchiveKind::Textures {
            return None;
        }
        archive.content.iter().find_map(|packed| {
            let ContentEntry::Texture(texture) = packed else {
                return None;
            };
            let name = String::from(texture.format);
            (texture.cubemap && !LEGACY_CUBEMAP_FORMATS.contains(&name.as_str())).then(|| {
                format!(
                    "`{}` packs the {name} cubemap `{}` into an FO4 DX10 BA2",
                    archive.path, texture.path
                )
            })
        })
    })
}

/// The largest per-game Archive maximum (FO4's 4000 MiB). A profile limit
/// above it is the only way an Archive can approach 4 GiB.
const LARGEST_TABLE_MAXIMUM: f64 = 4000.0 * 1024.0 * 1024.0;

/// Deviation 8. C++ writes an Archive over 4 GiB corrupt, Rust errors. Only a
/// profile limit above every game's own maximum lets an Archive grow that far.
fn archive_over_4_gib(context: &Context<'_>) -> Option<String> {
    let limit = context
        .profile_ini
        .value("BSA/maxBsaUncompressedSize")
        .to_f64();
    (limit > LARGEST_TABLE_MAXIMUM).then(|| {
        format!(
            "maxBsaUncompressedSize {limit} is above {LARGEST_TABLE_MAXIMUM}, so an Archive \
             could exceed 4 GiB"
        )
    })
}

/// Deviation 9. bethutil's name parser walks off the front of an all-digit or
/// empty stem. C++ parses the plugins and Archives directly in each Mod Root,
/// including the ones Archive creation names after the Mod Root itself.
fn all_digit_stem(context: &Context<'_>) -> Option<String> {
    let all_digits = |text: &str| text.chars().all(|c| c.is_ascii_digit());
    if let Some(root) = context
        .mod_roots
        .iter()
        .find(|root| all_digits(last_component(root)))
    {
        return Some(format!(
            "Mod Root `{root}` has an all-digit name, which its Archives' names would carry"
        ));
    }
    let games = [Game::Tes5, Game::Sse, Game::Fo4].map(Settings::get);
    context.in_mod_roots().find_map(|(_, relative, item)| {
        if relative.contains('/') {
            return None;
        }
        games
            .iter()
            .flat_map(|settings| {
                [NameKind::Plugin, NameKind::Archive]
                    .map(|kind| FilePath::make(Path::new(relative), settings, kind))
            })
            .flatten()
            .any(|name| all_digits(&name.name))
            .then(|| {
                format!(
                    "`{}` has a stem bethutil would parse as all digits",
                    item.path
                )
            })
    })
}

/// Deviation 10. Neither build reads `bBsaLeastBSA`, but no widget sets it
/// either: a case that turns it on is testing an option that no longer exists.
fn dead_data(context: &Context<'_>) -> Option<String> {
    context
        .settings_ini
        .value("BSA/bBsaLeastBSA")
        .to_bool()
        .then(|| "settings.ini turns on the dead `bBsaLeastBSA`".into())
}

/// Deviation 11. C++ runs any `bsaGame` (falling back to SSE's tables); Rust
/// makes a profile outside 3, 4 and 5 unreadable.
fn bsa_game(context: &Context<'_>) -> Option<String> {
    let game = context.bsa_game();
    (!(3..=5).contains(&game)).then(|| format!("bsaGame reads as {game}, outside 3, 4 and 5"))
}

/// Deviation 12. A plain scalar `texturesUnwantedFormats` is an empty list to
/// C++ and a one-element list to Rust.
fn scalar_list(context: &Context<'_>) -> Option<String> {
    let value = context
        .profile_ini
        .value("Textures/texturesUnwantedFormats");
    let scalar = match value {
        Value::String(_) => true,
        Value::Encoded(raw) => !raw.starts_with("@Variant("),
        Value::Invalid | Value::List(_) => false,
    };
    (scalar && !value.to_int_list().is_empty()).then(|| {
        format!(
            "texturesUnwantedFormats is the scalar `{}`",
            value.to_qstring()
        )
    })
}

/// Deviation 13. A `#` line is a comment to Rust, but a key or a format error
/// to C++.
fn hash_comment(context: &Context<'_>) -> Option<String> {
    context.ini_files.iter().find_map(|(name, bytes)| {
        bytes
            .split(|&byte| byte == b'\n')
            .position(|line| line.trim_ascii_start().starts_with(b"#"))
            .map(|index| format!("profiles/{name} line {} starts with `#`", index + 1))
    })
}

/// Deviation 14. Without a BOM, C++ reads Latin-1 and Rust reads valid UTF-8
/// as UTF-8; with one, Rust skips it and C++ keeps it in the first key. A
/// file is only safe when it is plain ASCII with no BOM.
fn ini_encoding(context: &Context<'_>) -> Option<String> {
    context.ini_files.iter().find_map(|(name, bytes)| {
        if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            Some(format!("profiles/{name} starts with a UTF-8 BOM"))
        } else if !bytes.is_ascii() && std::str::from_utf8(bytes).is_ok() {
            Some(format!("profiles/{name} is non-ASCII UTF-8 without a BOM"))
        } else {
            None
        }
    })
}

/// Deviation 15. C++ takes a name's stem up to its first dot untrimmed, so
/// `NUL .txt` is not a device to it; Rust trims trailing spaces and dots.
fn device_stem_trailing_space(context: &Context<'_>) -> Option<String> {
    let sources = context
        .case
        .tree
        .fs_shape
        .iter()
        .filter_map(FsShape::source);
    context
        .items
        .iter()
        .map(|item| item.path.as_str())
        .chain(sources)
        .find_map(|path| {
            path.split('/').find_map(|name| {
                let stem = name.split('.').next().unwrap_or(name);
                (is_device_name(name) && stem != stem.trim_end_matches([' ', '.'])).then(|| {
                    format!("`{path}`: `{name}` is a device name only once its stem is trimmed")
                })
            })
        })
}

/// Deviation 16. C++ matches each Packing Exclusion as a case-insensitive
/// substring of a file's absolute path, Rust of its path within the Mod Root.
/// They disagree when a rule matches only with the work directory, the
/// selection or the Mod Root's own name in front.
fn packing_exclusion_scope(context: &Context<'_>) -> Option<String> {
    context.in_mod_roots().find_map(|(root, relative, item)| {
        // Files directly in a Mod Root are never packed.
        if item.directory || !relative.contains('/') {
            return None;
        }
        let relative = relative.to_lowercase();
        context.side_texts.iter().find_map(|side| {
            let absolute = format!("{side}/{}/{relative}", root.to_lowercase());
            context
                .packing_rules
                .iter()
                .find(|rule| absolute.contains(rule.as_str()) && !relative.contains(rule.as_str()))
                .map(|rule| {
                    format!(
                        "Packing Exclusion `{rule}` matches `{absolute}` only outside its Mod Root"
                    )
                })
        })
    })
}

/// Deviation 17, Headpart Meshes. C++:
/// - finds a Mesh's game path from the first `/meshes/` in its absolute path,
///   so a `meshes` folder at or above a Mod Root shifts it (and the facegen
///   rule reads that game path);
/// - skips the facegen rule in Dry Run;
/// - scans every plugin under the selection, staging included;
/// - reads HDPT records with a parser that overflows or hangs on bad framing
///   and misreads compressed records.
fn headpart_meshes(context: &Context<'_>) -> Option<String> {
    for root in &context.mod_roots {
        for side in &context.side_texts {
            let above = format!("{side}/{}/", root.to_lowercase());
            if above.contains("/meshes/") || above.contains("facegen") {
                return Some(format!(
                    "`{above}` holds `meshes` or `facegen` at or above the Mod Root"
                ));
            }
        }
    }
    let spec = &context.case.spec;
    if spec.dry_run && spec.meshes.level >= 1 {
        let facegen = context.in_mod_roots().find(|(_, relative, item)| {
            !item.directory
                && has_extension(relative, &MESH_EXTENSIONS)
                && relative.to_lowercase().contains("facegen")
        });
        if let Some((_, _, item)) = facegen {
            return Some(format!(
                "a Dry Run with Mesh work over the facegen Mesh `{}`",
                item.path
            ));
        }
    }
    for item in &context.items {
        if item.directory || !has_extension(&item.path, &PLUGIN_EXTENSIONS) {
            continue;
        }
        if item
            .path
            .split('/')
            .any(|part| part.to_ascii_lowercase().starts_with(STAGING_PREFIX))
        {
            return Some(format!("the plugin `{}` is inside CAO staging", item.path));
        }
        if let Some(problem) = item
            .entry
            .and_then(|entry| context.bytes(entry))
            .and_then(|bytes| hdpt_problem(&bytes))
        {
            return Some(format!("the plugin `{}`: {problem}", item.path));
        }
    }
    None
}

/// The record flag marking a compressed record.
const COMPRESSED_RECORD: u32 = 0x0004_0000;
/// The C++ parser's `char buffer[1024]` for a MODL field.
const MODL_BUFFER: usize = 1024;
/// The size of a plugin's record and group headers.
const RECORD_HEADER: usize = 24;
/// The size of a plugin field's header: its type and a `u16` size.
const FIELD_HEADER: usize = 6;

/// The little-endian `u32` at `at`, if `bytes` holds all four of its bytes.
fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes(word.try_into().expect("four bytes")))
}

/// What in a plugin's framing C++'s HDPT parser would mishandle, if anything.
///
/// Walks the layout the parser reads (24-byte record and group headers, 6-byte
/// field headers) up to and through the `HDPT` group. Only that group is
/// deviation 17's: inside it, framing that runs past the end makes C++ loop
/// forever (and Rust report the plugin as unreadable), a compressed record is
/// misread, and a long or unterminated MODL overflows C++'s buffer. Framing
/// cut short before the group (a short `TES4` header, a truncated group of
/// another type) only stops C++'s read early, and a file that does not start
/// with `TES4` is skipped; neither is on the deviation list.
fn hdpt_problem(bytes: &[u8]) -> Option<String> {
    const HEADER: usize = RECORD_HEADER;
    if !bytes.starts_with(b"TES4") || bytes.len() < HEADER {
        return None;
    }
    let mut at = HEADER + u32_le(bytes, 4)? as usize;
    // Each top-level group needs a whole header; C++ stops at the first one it
    // cannot read, or that is not a group.
    while let Some(header) = bytes.get(at..at + HEADER)
        && &header[..4] == b"GRUP"
    {
        let size = u32_le(header, 4)? as usize;
        if &header[8..12] != b"HDPT" {
            if size < HEADER {
                // C++'s unsigned skip underflows, which ends its read.
                return None;
            }
            at += size;
            continue;
        }
        if size < HEADER {
            return Some(format!(
                "its HDPT group declares {size} bytes, less than its header"
            ));
        }
        let Some(group) = bytes.get(at + HEADER..at + size) else {
            return Some("its HDPT group runs past the end of the file".into());
        };
        return hdpt_group_problem(group);
    }
    None
}

/// What in an HDPT group's records C++ would mishandle, if anything.
fn hdpt_group_problem(group: &[u8]) -> Option<String> {
    const HEADER: usize = RECORD_HEADER;
    const FIELD: usize = FIELD_HEADER;
    let truncated = |what: &str| Some(format!("an HDPT {what} runs past the end of its group"));
    let mut at = 0;
    while at < group.len() {
        let Some(header) = group.get(at..at + HEADER) else {
            return truncated("record header");
        };
        let size = u32_le(header, 4)? as usize;
        let flags = u32_le(header, 8)?;
        if flags & COMPRESSED_RECORD != 0 {
            return Some("it has a compressed HDPT record".into());
        }
        let end = at + HEADER + size;
        let Some(record) = group.get(at + HEADER..end) else {
            return truncated("record");
        };
        let mut field_at = 0;
        while field_at < record.len() {
            let Some(field) = record.get(field_at..field_at + FIELD) else {
                return truncated("field header");
            };
            let size = usize::from(u16::from_le_bytes([field[4], field[5]]));
            let Some(data) = record.get(field_at + FIELD..field_at + FIELD + size) else {
                return truncated("field");
            };
            if &field[..4] == b"MODL" {
                if size >= MODL_BUFFER {
                    return Some(format!(
                        "a MODL field of {size} bytes overflows C++'s buffer"
                    ));
                }
                if !data.contains(&0) {
                    return Some("a MODL field has no terminating NUL".into());
                }
            }
            field_at += FIELD + size;
        }
        at = end;
    }
    None
}

/// Deviation 18. C++ rejects odd resize targets even when resizing by size is
/// off; Rust validates them only when it is on.
fn resize_validation(context: &Context<'_>) -> Option<String> {
    let textures = &context.case.spec.textures;
    (!textures.resize_by_size
        && !(textures.target_width.is_multiple_of(2) && textures.target_height.is_multiple_of(2)))
    .then(|| {
        format!(
            "resizing by size is off with the odd target {}x{}",
            textures.target_width, textures.target_height
        )
    })
}

/// Deviation 19. C++ makes a Several Mods child named `.cao-staging…` a Mod
/// Root (and plans its finalization); Rust skips it silently.
fn staging_mod_root(context: &Context<'_>) -> Option<String> {
    context
        .children
        .iter()
        .find(|child| {
            last_component(child)
                .to_ascii_lowercase()
                .starts_with(STAGING_PREFIX)
        })
        .map(|child| format!("the Several Mods child `{child}` is in the `.cao-staging` namespace"))
}

/// Deviation 20. C++ excludes a Several Mods child whose name contains
/// `separator` (case-sensitive), Rust one whose name ends in `_separator`.
/// C++'s pruning also keeps any directory whose absolute path contains
/// `separator` in any case; Rust's has no such rule. Pruning only runs in
/// Apply.
fn separator(context: &Context<'_>) -> Option<String> {
    if let Some(child) = context.children.iter().find(|child| {
        let name = last_component(child);
        name.contains(SEPARATOR_MARKER) && !name.ends_with("_separator")
    }) {
        return Some(format!(
            "the Several Mods child `{child}` contains `separator` without ending in `_separator`"
        ));
    }
    if context.case.spec.dry_run {
        return None;
    }
    for root in &context.mod_roots {
        for side in &context.side_texts {
            let absolute = format!("{side}/{}", root.to_lowercase());
            if absolute.contains(SEPARATOR_MARKER) {
                return Some(format!(
                    "the Mod Root path `{absolute}` contains `separator`"
                ));
            }
        }
    }
    context.in_mod_roots().find_map(|(_, relative, item)| {
        let directory = if item.directory {
            relative
        } else {
            relative
                .rsplit_once('/')
                .map_or("", |(directory, _)| directory)
        };
        directory
            .to_lowercase()
            .contains(SEPARATOR_MARKER)
            .then(|| format!("the directory of `{}` contains `separator`", item.path))
    })
}

/// Deviation 21. C++ can merge FO4 textures into the `GNRL` Main BA2; Rust
/// forces "create texture archive" on under FO4, so it never does.
fn fo4_merged_textures(context: &Context<'_>) -> Option<String> {
    (context.bsa_game() == 5 && context.case.spec.archives.merge_textures)
        .then(|| "an FO4 profile with textures merged into the Main BA2".into())
}

/// Deviation 22. Rust HTML-escapes a record's text and C++ does not, so a
/// path holding `&`, the one HTML metacharacter Windows allows in names,
/// would log differently. Checked against the side roots too.
fn log_escaping(context: &Context<'_>) -> Option<String> {
    let path = context
        .items
        .iter()
        .map(|item| item.path.as_str())
        .chain(context.side_texts.iter().map(String::as_str))
        .find(|path| path.contains('&'))?;
    Some(format!(
        "`{path}` holds `&`, which the two logs escape differently"
    ))
}

/// Deviation 23. Debug logging is the setting whose toggling C++ mishandles.
/// The oracle only takes it from `-l`, which the harness never passes, while
/// the Rust driver reads `bDebugLog` from `settings.ini`; a case that turns it
/// on would run the two builds at different log levels.
fn debug_log(context: &Context<'_>) -> Option<String> {
    context
        .settings_ini
        .value("bDebugLog")
        .to_bool()
        .then(|| "settings.ini turns on `bDebugLog`".into())
}

/// Deviation 24. The Log tab shows what the sink writes under `logs/`. A case
/// tree that puts content where a side's `logs/` folder is would mix the
/// Application Log, whose format and rotation differ by design, into the case.
fn logs_folder(context: &Context<'_>) -> Option<String> {
    top_level(context, &["logs"])
}

/// Deviation 25. C++ replaced `ownership.manifest` with one `MoveFileExW`, so
/// a real-time scanner briefly holding the manifest a run had just written
/// failed the run with Access denied; Rust retries for about a second. The
/// trigger is that host race, which no recipe can produce, so this never
/// fires. A destination that stays unreplaceable, such as a read-only
/// manifest, fails in both builds alike: Rust only waits first.
fn held_staging_manifest(_context: &Context<'_>) -> Option<String> {
    None
}

/// The profile values in `profile.ini` that no GUI widget can produce
/// (`src/MainWindow.cpp`, `src/MainWindow.ui`). The unwanted formats are
/// deviation 2's rule, and `bsaGame` deviation 11's.
///
/// A value a profile override replaces is not checked: the override, a
/// GUI-reachable preset by its type, is what both builds read.
fn unreachable_profile_values(context: &Context<'_>) -> Vec<HarnessError> {
    let ini = &context.profile_ini;
    let overrides = &context.case.profile_overrides;
    let mut found = Vec::new();
    let mut check = |key: &'static str, reachable: bool| {
        if !reachable {
            found.push(HarnessError::UnreachableProfileValue {
                key,
                value: ini.value(key).to_qstring(),
            });
        }
    };
    let format = ini.value("Textures/texturesFormat").to_i32() as u32;
    // The output format combo box.
    check(
        "Textures/texturesFormat",
        overrides.output_format.is_some()
            || [
                OutputFormat::Bc7,
                OutputFormat::Bc5,
                OutputFormat::Bc3,
                OutputFormat::Bc1,
                OutputFormat::R8G8B8A8,
            ]
            .iter()
            .any(|output| u32::from(output.format()) == format),
    );
    // The mesh user, stream and version combo boxes. A mesh target override
    // replaces all three with one of its presets.
    let mesh_target = overrides.mesh_target.is_some();
    check(
        "Meshes/meshesUser",
        mesh_target || [11, 12].contains(&ini.value("Meshes/meshesUser").to_u32()),
    );
    check(
        "Meshes/meshesStream",
        mesh_target || [82, 83, 100, 130].contains(&ini.value("Meshes/meshesStream").to_u32()),
    );
    // nifly's `V20_0_0_5` and `V20_2_0_7`.
    check(
        "Meshes/meshesFileVersion",
        mesh_target
            || [0x1400_0005, 0x1402_0007]
                .contains(&(ini.value("Meshes/meshesFileVersion").to_i32() as u32)),
    );
    // The maximum Archive size spin box: Qt's default 0–99.99, in GiB.
    let size = ini.value("BSA/maxBsaUncompressedSize").to_f64();
    check(
        "BSA/maxBsaUncompressedSize",
        (0.0..=99.99 * 1024.0 * 1024.0 * 1024.0).contains(&size),
    );
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_deviation_list_entry_has_exactly_one_rule() {
        for (index, deviation) in DEVIATIONS.iter().enumerate() {
            let number = u8::try_from(index + 1).unwrap();
            let rules: Vec<&str> = RULES
                .iter()
                .filter(|rule| rule.deviation == number)
                .map(|rule| rule.name)
                .collect();
            assert_eq!(
                rules.len(),
                1,
                "deviation {number} ({deviation}) needs exactly one rule, has {rules:?}"
            );
        }
        for rule in &RULES {
            assert!(
                (1..=DEVIATIONS.len()).contains(&usize::from(rule.deviation)),
                "rule `{}` names deviation {}, which is not on the list",
                rule.name,
                rule.deviation
            );
        }
    }

    #[test]
    fn every_dialog_format_is_a_known_dxgi_name() {
        for name in DIALOG_FORMATS {
            assert!(
                TextureFormat::try_from(name.to_owned()).is_ok(),
                "{name} is not a DXGI_FORMAT name"
            );
        }
    }
}
