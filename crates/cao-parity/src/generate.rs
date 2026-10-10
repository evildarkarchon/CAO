//! The corpus generator (#473, #500): the generated cases of a `corpus` run.
//!
//! Generated cases come from a deterministic [`crate::pairwise`] covering array
//! over the GUI's options, the GUI-reachable profile overrides and a few tree
//! features. Each row of the array becomes one [`CaseFile`], named from the
//! fixed [`crate::names`] vocabulary so no generated case holds a deviation
//! trigger. A handful of Start Error cases, which select a missing folder,
//! join them, because the comparator checks Start Errors too.
//!
//! - **Never committed.** The corpus is regenerated on every run, and
//!   `cao-parity case <id>` rebuilds a generated case from its id. Each case
//!   records [`GENERATOR_VERSION`], which reports print.
//! - **Faults are not dimensions.** One fault would flood the array with
//!   Completed With Failures outcomes; faults live in the fault seeds (#507).
//! - **Constraints** keep out what the spec excludes: Mesh work under FO4,
//!   Animations outside SSE, FO4 textures merged into `GNRL`, and the GUI's own
//!   rules (a Dry Run clears Archive work; Several Mods allows only mesh levels
//!   0 and 1).
//! - **Not yet dimensions:** LE and SSE input Meshes (the local asset pool's
//!   `local_asset` entry, #501, and synthetic Meshes, #503) and a headpart
//!   plugin (#529). Each joins the array with a [`GENERATOR_VERSION`] bump once
//!   its recipe entry exists; the pool's entry does, but the pinned list holds
//!   no LE Meshes yet. LE Animations are pinned (#502), and the seeds cover
//!   them; generated Animation cases still hold no `.hkx` files.

use cao_archive::Settings;

use crate::case::{ArchiveOptions, CaseFile, CaseSpec, MeshOptions, ModSelection, TextureOptions};
use crate::materialise::encode_base64;
use crate::names::{FOLDERS, MOD_ROOTS, PARENTS, STEMS};
use crate::pairwise::{Level, covering_array};
use crate::recipe::{
    ArchiveGame, ArchiveKind, ArchiveRecipe, ContentEntry, DdsHeader, MeshTarget, OutputFormat,
    Pattern, ProfileOverrides, TextureEntry, TextureFormat, TreeRecipe,
};

/// The generator's version. Bump it whenever a change makes any id build a
/// different case: a dimension, a level, a constraint, a name or a recipe.
pub const GENERATOR_VERSION: u32 = 1;

/// The array's dimensions, in column order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dimension {
    Profile,
    DryRun,
    SeveralMods,
    TexturesNecessary,
    TexturesCompress,
    TexturesMipmaps,
    TexturesResize,
    MeshLevel,
    MeshHeadparts,
    MeshResave,
    Animations,
    ArchiveExtract,
    ArchiveCreate,
    ArchiveDeleteBackup,
    ArchiveCompress,
    ArchiveDummies,
    ArchiveMergeIncompressible,
    ArchiveMergeTextures,
    ArchiveDeleteSources,
    OutputFormatOverride,
    UnwantedFormatsOverride,
    CompressInterfaceOverride,
    ConvertTgaOverride,
    MeshTargetOverride,
    InputArchives,
    TextureMix,
    LoadingPlugin,
}

use Dimension as D;

/// Each dimension with its number of levels, in column order. Level 0 of
/// every dimension is allowed beside every level of every other, so the
/// greedy builder can always complete a row.
const DIMENSIONS: [(Dimension, usize); 27] = [
    (D::Profile, PROFILES.len()),
    (D::DryRun, 2),
    (D::SeveralMods, 2),
    (D::TexturesNecessary, 2),
    (D::TexturesCompress, 2),
    (D::TexturesMipmaps, 2),
    // None, by ratio, by size.
    (D::TexturesResize, 3),
    (D::MeshLevel, 4),
    (D::MeshHeadparts, 2),
    (D::MeshResave, 2),
    (D::Animations, 2),
    (D::ArchiveExtract, 2),
    (D::ArchiveCreate, 2),
    (D::ArchiveDeleteBackup, 2),
    (D::ArchiveCompress, 2),
    (D::ArchiveDummies, 2),
    (D::ArchiveMergeIncompressible, 2),
    (D::ArchiveMergeTextures, 2),
    (D::ArchiveDeleteSources, 2),
    (D::OutputFormatOverride, OUTPUT_FORMATS.len()),
    // The profile's own list, none, or the alternative set.
    (D::UnwantedFormatsOverride, 3),
    // The profile's own value, on, off.
    (D::CompressInterfaceOverride, 3),
    (D::ConvertTgaOverride, 3),
    (D::MeshTargetOverride, MESH_TARGETS.len()),
    (D::InputArchives, 4),
    (D::TextureMix, 6),
    (D::LoadingPlugin, 4),
];

/// The profiles generated cases run under. TES4 is out of the port's scope.
const PROFILES: [&str; 3] = ["SSE", "TES5", "FO4"];
const SSE: Level = 0;
const FO4: Level = 2;

/// The output format overrides: the profile's own, then the combo box's.
const OUTPUT_FORMATS: [Option<OutputFormat>; 6] = [
    None,
    Some(OutputFormat::Bc7),
    Some(OutputFormat::Bc5),
    Some(OutputFormat::Bc3),
    Some(OutputFormat::Bc1),
    Some(OutputFormat::R8G8B8A8),
];

/// The mesh target overrides: the profile's own, then the three presets.
const MESH_TARGETS: [Option<MeshTarget>; 4] = [
    None,
    Some(MeshTarget::Le),
    Some(MeshTarget::Sse),
    Some(MeshTarget::Fo4),
];

/// The one alternative unwanted-formats set, in the dialog's order: both
/// uncompressed formats the `uncompressed` texture mix writes.
const ALTERNATIVE_UNWANTED: [&str; 2] = ["R8G8B8A8_UNORM", "B8G8R8A8_UNORM"];

/// Whether `x` and `y` may share a row; symmetric.
fn allowed(x: (Dimension, Level), y: (Dimension, Level)) -> bool {
    !(forbids(x, y) || forbids(y, x))
}

/// The one-way form of [`allowed`]'s constraints.
fn forbids((x, a): (Dimension, Level), (y, b): (Dimension, Level)) -> bool {
    match (x, y) {
        // FO4 has Mesh work disabled, and never merges textures into `GNRL`
        // (deviation 21).
        (D::Profile, D::MeshLevel) => a == FO4 && b > 0,
        (D::Profile, D::MeshResave | D::ArchiveMergeTextures) => a == FO4 && b == 1,
        // Only SSE has Animations.
        (D::Profile, D::Animations) => a != SSE && b == 1,
        // A Dry Run clears Archive work (deviation 3's rule on load).
        (D::DryRun, D::ArchiveExtract | D::ArchiveCreate | D::ArchiveDeleteBackup) => {
            a == 1 && b == 1
        }
        // Several Mods allows mesh levels 0 and 1 only.
        (D::SeveralMods, D::MeshLevel) => a == 1 && b > 1,
        _ => false,
    }
}

/// Every generated case with its id, in id order: the pairwise cases, then the
/// Start Error cases.
pub fn generated_cases() -> Vec<(String, CaseFile)> {
    let levels: Vec<usize> = DIMENSIONS.iter().map(|&(_, count)| count).collect();
    let rows = covering_array(&levels, &|i, a, j, b| {
        allowed((DIMENSIONS[i].0, a), (DIMENSIONS[j].0, b))
    })
    // The constraints keep level 0 of every dimension open to every other
    // level, so every allowed pair completes; a unit test pins that.
    .expect("the generator's constraints always complete a row");
    let mut cases: Vec<(String, CaseFile)> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| pairwise_case(index + 1, &Row(row)))
        .collect();
    cases.extend(start_error_cases());
    cases
}

/// The generated case named `id`, if this generator version produces one.
pub fn generated_case(id: &str) -> Option<CaseFile> {
    generated_cases()
        .into_iter()
        .find(|(case_id, _)| case_id == id)
        .map(|(_, case)| case)
}

/// One row of the array, read by dimension.
struct Row<'a>(&'a [Level]);

impl Row<'_> {
    fn level(&self, dimension: Dimension) -> Level {
        self.0[dimension as usize]
    }

    fn on(&self, dimension: Dimension) -> bool {
        self.level(dimension) == 1
    }
}

/// The id a generated case carries: `<prefix>-<nnn>-<profile>-<om|sm>-<apply|dry>`.
fn case_id(prefix: &str, number: usize, spec: &CaseSpec) -> String {
    let selection = match spec.mod_selection {
        ModSelection::OneMod { .. } => "om",
        ModSelection::SeveralMods { .. } => "sm",
    };
    let run = if spec.dry_run { "dry" } else { "apply" };
    format!(
        "{prefix}-{number:03}-{}-{selection}-{run}",
        spec.profile.to_ascii_lowercase()
    )
}

/// The names one case draws from the vocabulary, rotated by its number so the
/// corpus as a whole uses every name.
struct Names {
    parent: &'static str,
    roots: [&'static str; 2],
    folder: &'static str,
    stem: &'static str,
    /// A second stem, for Archive content the loose files do not shadow.
    other_stem: &'static str,
}

impl Names {
    fn for_case(number: usize) -> Self {
        Self {
            parent: PARENTS[number % PARENTS.len()],
            // Three apart, so the two roots always differ.
            roots: [
                MOD_ROOTS[number % MOD_ROOTS.len()],
                MOD_ROOTS[(number + 3) % MOD_ROOTS.len()],
            ],
            folder: FOLDERS[number % FOLDERS.len()],
            stem: STEMS[number % STEMS.len()],
            other_stem: STEMS[(number + 1) % STEMS.len()],
        }
    }
}

/// Builds the case of one array row.
fn pairwise_case(number: usize, row: &Row<'_>) -> (String, CaseFile) {
    let names = Names::for_case(number);
    let profile = PROFILES[row.level(D::Profile)];
    let several = row.on(D::SeveralMods);
    let mod_selection = if several {
        ModSelection::SeveralMods {
            folder: names.parent.into(),
        }
    } else {
        ModSelection::OneMod {
            folder: format!("{}/{}", names.parent, names.roots[0]),
        }
    };
    let resize = row.level(D::TexturesResize);
    let spec = CaseSpec {
        profile: profile.into(),
        mod_selection,
        dry_run: row.on(D::DryRun),
        textures: TextureOptions {
            necessary: row.on(D::TexturesNecessary),
            compress: row.on(D::TexturesCompress),
            mipmaps: row.on(D::TexturesMipmaps),
            resize_by_ratio: resize == 1,
            ratio_width: 2,
            ratio_height: 2,
            resize_by_size: resize == 2,
            // Even, so an unused target never trips deviation 18.
            target_width: 256,
            target_height: 256,
        },
        meshes: MeshOptions {
            level: u8::try_from(row.level(D::MeshLevel)).expect("mesh levels are 0 to 3"),
            headparts: row.on(D::MeshHeadparts),
            resave: row.on(D::MeshResave),
        },
        animations: row.on(D::Animations),
        archives: ArchiveOptions {
            extract: row.on(D::ArchiveExtract),
            create: row.on(D::ArchiveCreate),
            delete_backup: row.on(D::ArchiveDeleteBackup),
            compress: row.on(D::ArchiveCompress),
            create_dummies: row.on(D::ArchiveDummies),
            merge_incompressible: row.on(D::ArchiveMergeIncompressible),
            merge_textures: row.on(D::ArchiveMergeTextures),
            delete_sources: row.on(D::ArchiveDeleteSources),
        },
    };
    let profile_overrides = ProfileOverrides {
        output_format: OUTPUT_FORMATS[row.level(D::OutputFormatOverride)],
        unwanted_formats: match row.level(D::UnwantedFormatsOverride) {
            0 => None,
            1 => Some(Vec::new()),
            _ => Some(
                ALTERNATIVE_UNWANTED
                    .iter()
                    .map(|name| format(name))
                    .collect(),
            ),
        },
        compress_interface: optional_switch(row.level(D::CompressInterfaceOverride)),
        convert_tga: optional_switch(row.level(D::ConvertTgaOverride)),
        mesh_target: MESH_TARGETS[row.level(D::MeshTargetOverride)],
    };

    let roots: Vec<String> = if several {
        names
            .roots
            .iter()
            .map(|root| format!("{}/{root}", names.parent))
            .collect()
    } else {
        vec![format!("{}/{}", names.parent, names.roots[0])]
    };
    let mut content = Vec::new();
    for root in &roots {
        content.extend(texture_mix(row.level(D::TextureMix), root, &names));
        content.push(text(
            format!("{root}/scripts/{}.pex", names.stem),
            "loose compiled script",
        ));
        content.push(text(
            format!("{root}/sound/fx/{}.wav", names.stem),
            "loose sound bytes, incompressible",
        ));
    }
    // The Archive and plugin features live in the first Mod Root only.
    let game = game(profile);
    content.extend(input_archives(
        row.level(D::InputArchives),
        &roots[0],
        names.roots[0],
        game,
        &names,
    ));
    content.extend(loading_plugin(
        row.level(D::LoadingPlugin),
        &roots[0],
        names.roots[0],
        game,
    ));

    let id = case_id("pw", number, &spec);
    (
        id,
        CaseFile {
            spec,
            profile_overrides,
            tree: TreeRecipe {
                content,
                fs_shape: Vec::new(),
            },
            generator_version: Some(GENERATOR_VERSION),
        },
    )
}

/// `None` for the profile's own value, then on and off.
fn optional_switch(level: Level) -> Option<bool> {
    match level {
        0 => None,
        1 => Some(true),
        _ => Some(false),
    }
}

/// A format by its short DirectXTex name; the names here are all known.
fn format(name: &str) -> TextureFormat {
    TextureFormat::try_from(name.to_owned()).expect("a known DXGI_FORMAT name")
}

fn text(path: String, text: &str) -> ContentEntry {
    ContentEntry::Text {
        path,
        text: text.into(),
        note: None,
    }
}

/// A texture entry with the defaults the mixes share: legacy header, one image.
fn texture(
    path: String,
    name: &str,
    size: (u32, u32),
    mip_levels: u32,
    pattern: Pattern,
) -> TextureEntry {
    TextureEntry {
        path,
        format: format(name),
        width: size.0,
        height: size.1,
        mip_levels,
        array_size: 1,
        cubemap: false,
        header: DdsHeader::Legacy,
        pattern,
        fault: None,
        note: None,
    }
}

/// The Textures of one Mod Root, by texture-format mix: legacy BC, BC7 under
/// the DX10 header, uncompressed with odd sizes, TGA, one oversized
/// Texture above the resize targets, and a cubemap with an array.
fn texture_mix(mix: Level, root: &str, names: &Names) -> Vec<ContentEntry> {
    let base = format!("{root}/textures/{}/{}", names.folder, names.stem);
    let path = |suffix: &str| format!("{base}{suffix}");
    let textures = match mix {
        0 => vec![
            texture(
                path("_d.dds"),
                "BC1_UNORM",
                (256, 256),
                0,
                Pattern::Gradient,
            ),
            texture(
                path("_g.dds"),
                "BC3_UNORM",
                (128, 128),
                1,
                Pattern::AlphaRamp,
            ),
        ],
        1 => vec![
            // Small: DirectXTex's CPU BC7 encoder takes ~23 s for 256x256
            // with mips, and every run materialises it afresh.
            TextureEntry {
                header: DdsHeader::Dx10,
                ..texture(path("_d.dds"), "BC7_UNORM", (64, 64), 0, Pattern::Noise)
            },
            texture(path("_n.dds"), "BC5_UNORM", (128, 128), 0, Pattern::Normal),
        ],
        2 => vec![
            texture(
                path("_d.dds"),
                "R8G8B8A8_UNORM",
                // Odd and not a power of two.
                (201, 121),
                1,
                Pattern::Mask,
            ),
            texture(
                path("_e.dds"),
                "B8G8R8A8_UNORM",
                (64, 64),
                0,
                Pattern::Edges,
            ),
        ],
        3 => vec![
            texture(
                path("_d.tga"),
                "R8G8B8A8_UNORM",
                (128, 128),
                1,
                Pattern::Gradient,
            ),
            texture(path("_m.dds"), "BC1_UNORM", (64, 64), 0, Pattern::Edges),
        ],
        4 => vec![
            texture(path("_d.dds"), "BC1_UNORM", (2048, 2048), 0, Pattern::Noise),
            texture(path("_s.dds"), "BC3_UNORM", (64, 64), 0, Pattern::Mask),
        ],
        _ => vec![
            TextureEntry {
                cubemap: true,
                ..texture(path("_e.dds"), "BC1_UNORM", (64, 64), 0, Pattern::Gradient)
            },
            TextureEntry {
                array_size: 3,
                header: DdsHeader::Dx10,
                ..texture(
                    path("_p.dds"),
                    "R8G8B8A8_UNORM",
                    (64, 64),
                    1,
                    Pattern::Noise,
                )
            },
        ],
    };
    textures.into_iter().map(ContentEntry::Texture).collect()
}

/// The Archive game of a generated profile.
fn game(profile: &str) -> ArchiveGame {
    match profile {
        "TES5" => ArchiveGame::Tes5,
        "FO4" => ArchiveGame::Fo4,
        _ => ArchiveGame::Sse,
    }
}

/// The input Archives of the first Mod Root: none, one, one whose file a
/// loose file shadows, or two colliding on one game path.
fn input_archives(
    feature: Level,
    root: &str,
    root_name: &str,
    game: ArchiveGame,
    names: &Names,
) -> Vec<ContentEntry> {
    let archive = |suffix: &str, script_stem: &str, script: &str| {
        let name = match game {
            ArchiveGame::Fo4 => format!("{root_name}{suffix} - Main.ba2"),
            _ => format!("{root_name}{suffix}.bsa"),
        };
        let mut content = vec![text(format!("scripts/{script_stem}.pex"), script)];
        // A GNRL BA2 holds no Textures; the BSAs carry one to extract.
        if game != ArchiveGame::Fo4 {
            content.push(ContentEntry::Texture(texture(
                format!("textures/{}/{}_a.dds", names.folder, names.other_stem),
                "BC1_UNORM",
                (64, 64),
                0,
                Pattern::Edges,
            )));
        }
        ContentEntry::Archive(ArchiveRecipe {
            path: format!("{root}/{name}"),
            game,
            archive_type: ArchiveKind::Standard,
            compress: true,
            content,
            note: None,
        })
    };
    match feature {
        0 => Vec::new(),
        1 => vec![archive("", names.other_stem, "archived compiled script")],
        // The loose `scripts/<stem>.pex` shadows the archived one.
        2 => vec![archive("", names.stem, "archived compiled script")],
        _ => vec![
            archive("", names.other_stem, "archived compiled script"),
            archive(" - Patch", names.other_stem, "patched compiled script"),
        ],
    }
}

/// The existing Loading Plugin of the first Mod Root, named after it: none,
/// the game's exact Dummy Plugin, a near-dummy one byte off, or a full plugin.
fn loading_plugin(
    feature: Level,
    root: &str,
    root_name: &str,
    game: ArchiveGame,
) -> Vec<ContentEntry> {
    let settings = Settings::get(game.game());
    let path = format!("{root}/{root_name}.{}", settings.plugin_extensions[0]);
    let raw = |bytes: &[u8]| ContentEntry::Raw {
        path: path.clone(),
        base64: Some(encode_base64(bytes)),
        fixture: None,
        note: None,
    };
    match feature {
        0 => Vec::new(),
        1 => vec![raw(settings.dummy_plugin)],
        2 => {
            let mut near = *settings.dummy_plugin;
            let last = near.len() - 1;
            near[last] ^= 0x01;
            vec![raw(&near)]
        }
        _ => vec![text(path, "TES4 full plugin bytes")],
    }
}

/// The Start Error cases: a selection whose folder is missing, once per Mod
/// Selection kind. The tree still holds a Mod Root, beside the selection.
fn start_error_cases() -> Vec<(String, CaseFile)> {
    let selections = [
        ModSelection::OneMod {
            folder: format!("{}/{}", PARENTS[0], MOD_ROOTS[1]),
        },
        ModSelection::SeveralMods {
            folder: PARENTS[1].into(),
        },
    ];
    selections
        .into_iter()
        .enumerate()
        .map(|(index, mod_selection)| {
            let spec = CaseSpec {
                profile: PROFILES[SSE].into(),
                mod_selection,
                dry_run: false,
                textures: TextureOptions {
                    necessary: true,
                    compress: false,
                    mipmaps: false,
                    resize_by_ratio: false,
                    ratio_width: 2,
                    ratio_height: 2,
                    resize_by_size: false,
                    target_width: 256,
                    target_height: 256,
                },
                meshes: MeshOptions {
                    level: 0,
                    headparts: false,
                    resave: false,
                },
                animations: false,
                archives: ArchiveOptions {
                    extract: false,
                    create: false,
                    delete_backup: false,
                    compress: true,
                    create_dummies: true,
                    merge_incompressible: true,
                    merge_textures: false,
                    delete_sources: false,
                },
            };
            let root = format!("{}/{}", PARENTS[0], MOD_ROOTS[0]);
            let tree = TreeRecipe {
                content: vec![ContentEntry::Texture(texture(
                    format!("{root}/textures/{}/{}_d.dds", FOLDERS[0], STEMS[0]),
                    "BC1_UNORM",
                    (64, 64),
                    0,
                    Pattern::Gradient,
                ))],
                fs_shape: Vec::new(),
            };
            (
                case_id("se", index + 1, &spec),
                CaseFile {
                    spec,
                    profile_overrides: ProfileOverrides::default(),
                    tree,
                    generator_version: Some(GENERATOR_VERSION),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The greedy builder can always complete a row because level 0 of every
    /// dimension is allowed beside every level of every other.
    #[test]
    fn level_zero_of_every_dimension_is_allowed_with_everything() {
        for &(x, _) in &DIMENSIONS {
            for &(y, count) in &DIMENSIONS {
                if x == y {
                    continue;
                }
                for b in 0..count {
                    assert!(allowed((x, 0), (y, b)), "{x:?}=0 with {y:?}={b}");
                }
            }
        }
    }

    #[test]
    fn dimensions_are_listed_in_column_order() {
        for (column, &(dimension, _)) in DIMENSIONS.iter().enumerate() {
            assert_eq!(dimension as usize, column, "{dimension:?}");
        }
    }
}
