//! The deviation guard (#499): one rule per deviation-list entry, each with a
//! positive and a negative case, plus the guard's place in the pipeline: a
//! case it rejects is never materialised, so neither build can run it.
//!
//! The guard reads the case recipe, the absolute side roots under the work
//! directory, and the profile files both sides are provisioned from. None of
//! the case directory needs to exist, so most tests point it at a path that is
//! never created.

mod common;

use std::path::PathBuf;

use cao_parity::HarnessError;
use cao_parity::case::{CaseFile, CaseLayout, Side, SideResources};
use cao_parity::cases::{fixtures_dir, seeds};
use cao_parity::guard::{GuardInput, rejections};
use cao_parity::materialise::{Environment, materialise};
use cao_profiles::Profiles;
use common::{TempDir, copy_tree, shipped_profiles};
use serde_json::{Value, json};

/// A plain SSE Apply case over one Mod Root, which triggers nothing.
fn plain() -> Value {
    json!({
        "spec": {
            "profile": "SSE",
            "mod_selection": {"kind": "one_mod", "folder": "mods/Mod"},
            "dry_run": false,
            "textures": {
                "necessary": true, "compress": false, "mipmaps": false,
                "resize_by_ratio": false, "ratio_width": 2, "ratio_height": 2,
                "resize_by_size": false, "target_width": 64, "target_height": 64
            },
            "meshes": {"level": 0, "headparts": false, "resave": false},
            "animations": false,
            "archives": {
                "extract": false, "create": false, "delete_backup": false,
                "compress": true, "create_dummies": true, "merge_incompressible": true,
                "merge_textures": false, "delete_sources": true
            }
        },
        "tree": {"content": [
            {"kind": "texture", "path": "mods/Mod/textures/plain.dds",
             "format": "R8G8B8A8_UNORM", "width": 16, "height": 16}
        ]}
    })
}

/// [`plain`] after `edit`, as a case file.
fn case(edit: impl FnOnce(&mut Value)) -> CaseFile {
    let mut value = plain();
    edit(&mut value);
    serde_json::from_value(value).unwrap()
}

/// [`plain`] with `entries` added to its content.
fn with_content(entries: Value) -> CaseFile {
    case(|case| {
        let content = case["tree"]["content"].as_array_mut().unwrap();
        content.extend(entries.as_array().unwrap().iter().cloned());
    })
}

/// A text content entry.
fn text(path: &str) -> Value {
    json!({"kind": "text", "path": path, "text": "text"})
}

/// A raw content entry holding `bytes`.
fn raw(path: &str, bytes: &[u8]) -> Value {
    json!({"kind": "raw", "path": path, "base64": base64(bytes)})
}

/// Standard, padded base64 (RFC 4648 §4).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (index, &byte)| {
            word | u32::from(byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                text.push(char::from(
                    ALPHABET[(word >> (18 - 6 * index) & 63) as usize],
                ));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// Where the guard runs a case from: a work directory, and the profiles both
/// sides are provisioned from.
struct Host {
    /// Keeps a copied `profiles/` alive; `None` for the shipped one.
    _temp: Option<TempDir>,
    profiles: PathBuf,
    case_root: PathBuf,
}

impl Host {
    /// The shipped profiles, and a case directory under the system temp dir
    /// that is never created.
    fn shipped() -> Self {
        Self {
            _temp: None,
            profiles: shipped_profiles(),
            case_root: std::env::temp_dir().join("cao-parity-guard").join("case"),
        }
    }

    /// A private copy of the shipped profiles, which the test may edit, at
    /// `<temp>/app/profiles`.
    fn copied(name: &str) -> Self {
        let temp = TempDir::new(name);
        let profiles = temp.path().join("app").join("profiles");
        copy_tree(&shipped_profiles(), &profiles);
        let case_root = temp.path().join("work").join("case");
        Self {
            _temp: Some(temp),
            profiles,
            case_root,
        }
    }

    /// The same host with the case directory at `case_root`.
    fn at(mut self, case_root: PathBuf) -> Self {
        self.case_root = case_root;
        self
    }

    /// Rewrites the profile file at `relative` (under `profiles/`) as text.
    fn edit(&self, relative: &str, edit: impl FnOnce(String) -> String) {
        let path = self.profiles.join(relative);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, edit(text)).unwrap();
    }

    /// Replaces the profile file at `relative` with `bytes`.
    fn write(&self, relative: &str, bytes: &[u8]) {
        std::fs::write(self.profiles.join(relative), bytes).unwrap();
    }

    /// Everything the guard rejects `case` for.
    fn rejections(&self, case: &CaseFile) -> Vec<HarnessError> {
        rejections(&GuardInput {
            case,
            case_root: &self.case_root,
            profiles: &self.profiles,
            fixtures: &fixtures_dir(),
        })
        .unwrap()
    }

    /// The deviations whose rules fire for `case`.
    fn deviations(&self, case: &CaseFile) -> Vec<u8> {
        self.rejections(case)
            .iter()
            .filter_map(|rejection| match rejection {
                HarnessError::DeviationTrigger { deviation, .. } => Some(*deviation),
                _ => None,
            })
            .collect()
    }

    /// Asserts that deviation `number`'s rule fires for `case`.
    fn fires(&self, number: u8, case: &CaseFile) {
        let deviations = self.deviations(case);
        assert!(
            deviations.contains(&number),
            "deviation {number} should fire; fired: {deviations:?}"
        );
    }

    /// Asserts that deviation `number`'s rule stays quiet for `case`.
    fn quiet(&self, number: u8, case: &CaseFile) {
        let rejections = self.rejections(case);
        assert!(
            !rejections.iter().any(|rejection| matches!(
                rejection,
                HarnessError::DeviationTrigger { deviation, .. } if *deviation == number
            )),
            "deviation {number} should stay quiet; rejected for: {}",
            rejections
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
}

// --- The guard as a whole ---

#[test]
fn a_plain_case_passes_under_every_shipped_profile() {
    let host = Host::shipped();
    for profile in ["SSE", "TES5", "FO4"] {
        let case = case(|case| case["spec"]["profile"] = json!(profile));
        let rejected = host.rejections(&case);
        assert!(rejected.is_empty(), "{profile}: {rejected:?}");
    }
}

#[test]
fn every_committed_seed_passes_the_guard() {
    let work = std::env::temp_dir().join("cao-parity-guard");
    for (id, case) in seeds().unwrap() {
        let rejected = Host::shipped().at(work.join(&id)).rejections(&case);
        assert!(rejected.is_empty(), "seed `{id}`: {rejected:?}");
    }
}

#[test]
fn a_triggering_recipe_is_rejected_before_anything_is_written() {
    let temp = TempDir::new("guard-materialise");
    let layout = CaseLayout::new(temp.path(), "fo4-merged-textures").unwrap();
    let triggering = case(|case| {
        case["spec"]["profile"] = json!("FO4");
        case["spec"]["archives"]["merge_textures"] = json!(true);
    });
    let profiles = shipped_profiles();
    let environment = Environment {
        resources: SideResources {
            profiles: &profiles,
            hkxcmd: None,
        },
        fixtures: &fixtures_dir(),
        symlink_rights: true,
        local_assets: common::empty_pool(),
    };

    let error = materialise(&layout, &triggering, &environment).unwrap_err();

    assert!(
        matches!(error, HarnessError::DeviationTrigger { deviation: 21, .. }),
        "{error}"
    );
    // Neither side was provisioned, so neither build has anything to run.
    for path in [
        layout.input(),
        layout.side(Side::Oracle),
        layout.side(Side::Rust),
    ] {
        assert!(!path.exists(), "{} exists", path.display());
    }
}

#[test]
fn a_profile_value_the_gui_cannot_produce_is_rejected() {
    let host = Host::copied("guard-gui-values");
    host.edit("SSE/profile.ini", |text| {
        text.replace("texturesFormat=98", "texturesFormat=99")
            .replace("meshesStream=100", "meshesStream=101")
            .replace(
                "maxBsaUncompressedSize=2104533975.04",
                "maxBsaUncompressedSize=-1",
            )
    });

    let keys: Vec<&str> = host
        .rejections(&case(|_| {}))
        .iter()
        .filter_map(|rejection| match rejection {
            HarnessError::UnreachableProfileValue { key, .. } => Some(*key),
            _ => None,
        })
        .collect();

    assert_eq!(
        keys,
        [
            "Textures/texturesFormat",
            "Meshes/meshesStream",
            "BSA/maxBsaUncompressedSize"
        ]
    );
}

#[test]
fn a_profile_value_an_override_replaces_is_not_checked() {
    let host = Host::copied("guard-gui-overridden");
    host.edit("SSE/profile.ini", |text| {
        text.replace("texturesFormat=98", "texturesFormat=99")
            .replace("meshesStream=100", "meshesStream=101")
    });
    let overridden = case(|case| {
        case["profile_overrides"] = json!({"output_format": "BC1_UNORM", "mesh_target": "sse"});
    });

    let rejected = host.rejections(&overridden);

    assert!(rejected.is_empty(), "{rejected:?}");
}

// --- One positive and one negative test per rule ---

#[test]
fn d01_fires_for_a_top_level_profiles_or_bin_folder() {
    let host = Host::shipped();
    host.fires(1, &with_content(json!([text("profiles/SSE/readme.txt")])));
    host.fires(1, &with_content(json!([text("Bin/hkxcmd.exe")])));
}

#[test]
fn d01_is_quiet_for_those_names_inside_a_mod_root() {
    Host::shipped().quiet(
        1,
        &with_content(json!([
            text("mods/Mod/profiles/readme.txt"),
            text("mods/Mod/bin/x.txt")
        ])),
    );
}

#[test]
fn d02_fires_for_unwanted_formats_the_dialog_cannot_commit() {
    let host = Host::shipped();
    let out_of_order = case(|case| {
        case["profile_overrides"] = json!({"unwanted_formats": ["B5G6R5_UNORM", "BC1_UNORM"]});
    });
    host.fires(2, &out_of_order);
    let twice = case(|case| {
        case["profile_overrides"] = json!({"unwanted_formats": ["BC1_UNORM", "BC1_UNORM"]});
    });
    host.fires(2, &twice);
    let typeless = case(|case| {
        case["profile_overrides"] = json!({"unwanted_formats": ["R8G8B8A8_TYPELESS"]});
    });
    host.fires(2, &typeless);
}

#[test]
fn d02_is_quiet_for_a_dialog_ordered_override_and_the_shipped_lists() {
    let host = Host::shipped();
    let ordered = case(|case| {
        case["profile_overrides"] = json!({"unwanted_formats": ["BC1_UNORM", "B5G6R5_UNORM"]});
    });
    host.quiet(2, &ordered);
    // FO4 ships `86, 85, 115`, out of the dialog's order: a GUI save that
    // never opens the dialog keeps it.
    host.quiet(2, &case(|case| case["spec"]["profile"] = json!("FO4")));
}

#[test]
fn d03_fires_for_settings_the_ui_rules_would_change_on_load() {
    let host = Host::shipped();
    host.fires(
        3,
        &case(|case| {
            case["spec"]["dry_run"] = json!(true);
            case["spec"]["archives"]["extract"] = json!(true);
        }),
    );
    host.fires(
        3,
        &case(|case| {
            case["spec"]["mod_selection"] = json!({"kind": "several_mods", "folder": "mods"});
            case["spec"]["meshes"]["level"] = json!(2);
        }),
    );
}

#[test]
fn d03_is_quiet_for_settings_both_ui_rules_allow() {
    let host = Host::shipped();
    // The archive options the Dry Run rule leaves alone do nothing without
    // extraction or creation.
    host.quiet(3, &case(|case| case["spec"]["dry_run"] = json!(true)));
    host.quiet(
        3,
        &case(|case| {
            case["spec"]["mod_selection"] = json!({"kind": "several_mods", "folder": "mods"});
            case["spec"]["meshes"]["level"] = json!(1);
        }),
    );
}

#[test]
fn d04_fires_for_a_top_level_translations_folder() {
    Host::shipped().fires(
        4,
        &with_content(json!([text("translations/AssetsOpt_de.qm")])),
    );
}

#[test]
fn d04_is_quiet_for_translations_inside_a_mod_root() {
    Host::shipped().quiet(
        4,
        &with_content(json!([text("mods/Mod/translations/x.txt")])),
    );
}

/// A case directory whose absolute path is more than 1024 UTF-16 units long.
fn deep_case_root() -> PathBuf {
    std::env::temp_dir().join("w".repeat(1030)).join("case")
}

#[test]
fn d05_fires_for_a_texture_path_that_overflows_the_cpp_buffer() {
    Host::shipped().at(deep_case_root()).fires(5, &case(|_| {}));
}

#[test]
fn d05_is_quiet_for_a_long_path_that_is_not_a_texture() {
    let readme_only = case(|case| {
        case["tree"]["content"] = json!([text("mods/Mod/readme.txt")]);
    });
    Host::shipped().at(deep_case_root()).quiet(5, &readme_only);
}

#[test]
fn d06_fires_for_a_dds_written_as_raw_bytes() {
    let mut dds = b"DDS ".to_vec();
    dds.extend([0u8; 124]);
    Host::shipped().fires(
        6,
        &with_content(json!([raw("mods/Mod/textures/raw.dds", &dds)])),
    );
}

#[test]
fn d06_is_quiet_for_raw_bytes_that_are_no_dds() {
    Host::shipped().quiet(
        6,
        &with_content(json!([raw("mods/Mod/textures/junk.dds", &[0, 1, 2, 255])])),
    );
}

/// An Archive recipe of `game` and `type` holding one cubemap in `format`.
fn cubemap_archive(game: &str, archive_type: &str, format: &str) -> CaseFile {
    with_content(json!([{
        "kind": "archive", "path": "mods/Mod/Mod - Textures.ba2",
        "game": game, "type": archive_type,
        "content": [{"kind": "texture", "path": "textures/sky.dds", "format": format,
                     "width": 8, "height": 8, "cubemap": true, "header": "dx10"}]
    }]))
}

#[test]
fn d07_fires_for_an_fo4_dx10_cubemap_in_a_format_cpp_cannot_extract() {
    let host = Host::shipped();
    for format in ["BC7_UNORM", "BC6H_UF16", "BC1_UNORM_SRGB"] {
        host.fires(7, &cubemap_archive("fo4", "textures", format));
    }
}

#[test]
fn d07_is_quiet_for_a_legacy_format_cubemap_or_another_game() {
    let host = Host::shipped();
    host.quiet(7, &cubemap_archive("fo4", "textures", "BC1_UNORM"));
    host.quiet(7, &cubemap_archive("sse", "textures", "BC7_UNORM"));
}

#[test]
fn d08_fires_for_a_profile_limit_above_every_games_maximum() {
    let host = Host::copied("guard-d08");
    host.edit("SSE/profile.ini", |text| {
        text.replace("2104533975.04", "5368709120")
    });
    host.fires(8, &case(|_| {}));
}

#[test]
fn d08_is_quiet_for_the_shipped_fo4_limit() {
    Host::shipped().quiet(8, &case(|case| case["spec"]["profile"] = json!("FO4")));
}

#[test]
fn d09_fires_for_all_digit_plugin_and_archive_stems_in_a_mod_root() {
    let host = Host::shipped();
    host.fires(9, &with_content(json!([text("mods/Mod/2.esp")])));
    host.fires(9, &with_content(json!([text("mods/Mod/7 - Textures.bsa")])));
    let digits_root = case(|case| {
        case["spec"]["mod_selection"]["folder"] = json!("mods/2");
        case["tree"]["content"] = json!([text("mods/2/textures/a.txt")]);
    });
    host.fires(9, &digits_root);
}

#[test]
fn d09_is_quiet_for_stems_with_letters_or_files_below_the_mod_root() {
    Host::shipped().quiet(
        9,
        &with_content(json!([
            text("mods/Mod/Mod2.esp"),
            text("mods/Mod/Mod - Textures.bsa"),
            text("mods/Mod/textures/2.esp")
        ])),
    );
}

#[test]
fn d10_fires_when_settings_turn_on_dead_data() {
    let host = Host::copied("guard-d10");
    host.edit("SSE/settings.ini", |text| {
        text.replace("bBsaLeastBSA=false", "bBsaLeastBSA=true")
    });
    host.fires(10, &case(|_| {}));
}

#[test]
fn d10_is_quiet_for_the_shipped_dead_data() {
    // The shipped profiles hold `animationFormat`, `DummyPlugin.esp` and
    // `customLandscape.txt`, which neither build reads.
    Host::shipped().quiet(10, &case(|_| {}));
}

#[test]
fn d11_fires_for_a_bsa_game_outside_3_4_and_5() {
    let host = Host::copied("guard-d11");
    host.edit("SSE/profile.ini", |text| {
        text.replace("bsaGame=4", "bsaGame=1")
    });
    host.fires(11, &case(|_| {}));
}

#[test]
fn d11_is_quiet_for_the_shipped_games() {
    let host = Host::shipped();
    for profile in ["SSE", "TES5", "FO4"] {
        host.quiet(11, &case(|case| case["spec"]["profile"] = json!(profile)));
    }
}

#[test]
fn d12_fires_for_a_scalar_unwanted_formats_list() {
    let host = Host::copied("guard-d12");
    host.edit("SSE/profile.ini", |text| {
        text.replace(
            "texturesUnwantedFormats=85, 86, 115",
            "texturesUnwantedFormats=85",
        )
    });
    host.fires(12, &case(|_| {}));
}

#[test]
fn d12_is_quiet_for_a_one_element_list_as_the_gui_saves_it() {
    let host = Host::copied("guard-d12-variant");
    let profile = Profiles::new(host.profiles.parent().unwrap()).open("SSE");
    let mut settings = profile.load_settings().unwrap();
    settings.textures_unwanted_formats = vec![85];
    profile.save_settings(&settings).unwrap();
    assert!(
        std::fs::read_to_string(profile.profile_ini())
            .unwrap()
            .contains("@Variant("),
        "the writer stores a one-element list as @Variant"
    );
    host.quiet(12, &case(|_| {}));
}

#[test]
fn d13_fires_for_a_hash_line_in_a_profile_ini() {
    let host = Host::copied("guard-d13");
    host.edit("SSE/profile.ini", |text| format!("# hand edit\r\n{text}"));
    host.fires(13, &case(|_| {}));
}

#[test]
fn d13_is_quiet_for_a_semicolon_comment() {
    let host = Host::copied("guard-d13-semicolon");
    host.edit("SSE/profile.ini", |text| format!("; hand edit\r\n{text}"));
    host.quiet(13, &case(|_| {}));
}

#[test]
fn d14_fires_for_a_bom_or_non_ascii_utf8_without_one() {
    let host = Host::copied("guard-d14");
    let original = std::fs::read(host.profiles.join("SSE/settings.ini")).unwrap();
    let mut with_bom = vec![0xEF, 0xBB, 0xBF];
    with_bom.extend(&original);
    host.write("SSE/settings.ini", &with_bom);
    host.fires(14, &case(|_| {}));

    let mut utf8 = original;
    utf8.extend("note=Ünïcode\r\n".as_bytes());
    host.write("SSE/settings.ini", &utf8);
    host.fires(14, &case(|_| {}));
}

#[test]
fn d14_is_quiet_for_latin1_that_is_not_valid_utf8() {
    let host = Host::copied("guard-d14-latin1");
    let mut latin1 = std::fs::read(host.profiles.join("SSE/settings.ini")).unwrap();
    // `Ü` in Latin-1: both builds decode it the same way.
    latin1.extend(b"note=\xDC\r\n");
    host.write("SSE/settings.ini", &latin1);
    host.quiet(14, &case(|_| {}));
}

#[test]
fn d15_fires_for_a_device_stem_with_a_trailing_space() {
    Host::shipped().fires(15, &with_content(json!([text("mods/Mod/NUL .txt")])));
}

#[test]
fn d15_is_quiet_for_device_names_both_builds_recognise() {
    // `NUL.txt` is a device to both builds; the materialiser rejects it.
    Host::shipped().quiet(
        15,
        &with_content(json!([
            text("mods/Mod/NUL.txt"),
            text("mods/Mod/Nullish .txt")
        ])),
    );
}

#[test]
fn d16_fires_for_a_packing_exclusion_matching_above_the_mod_root() {
    let host = Host::shipped();
    let named_root = case(|case| {
        case["spec"]["mod_selection"]["folder"] = json!("mods/CalienteTools");
        case["tree"]["content"] = json!([text("mods/CalienteTools/meshes/body.nif")]);
    });
    host.fires(16, &named_root);
    let work = std::env::temp_dir().join("dialogueviews").join("case");
    Host::shipped().at(work).fires(16, &case(|_| {}));
}

#[test]
fn d16_is_quiet_for_a_packing_exclusion_within_the_mod_root() {
    Host::shipped().quiet(
        16,
        &with_content(json!([
            text("mods/Mod/CalienteTools/bodyslide.txt"),
            text("mods/Mod/docs/readme.txt")
        ])),
    );
}

/// One plugin field: its four-letter type and its data.
type Field = (&'static [u8; 4], Vec<u8>);

/// A plugin: a `TES4` record, then an `HDPT` group of `records`, each a
/// `(flags, fields)` pair.
fn plugin(records: &[(u32, Vec<Field>)]) -> Vec<u8> {
    fn header(kind: &[u8; 4], size: u32, word: u32) -> Vec<u8> {
        let mut bytes = kind.to_vec();
        bytes.extend(size.to_le_bytes());
        bytes.extend(word.to_le_bytes());
        bytes.extend([0u8; 12]);
        bytes
    }
    let mut group = Vec::new();
    for (flags, fields) in records {
        let mut data = Vec::new();
        for (kind, field) in fields {
            data.extend(*kind);
            data.extend(u16::try_from(field.len()).unwrap().to_le_bytes());
            data.extend(field);
        }
        group.extend(header(b"HDPT", u32::try_from(data.len()).unwrap(), *flags));
        group.extend(data);
    }
    let mut bytes = header(b"TES4", 0, 0);
    let mut grup = b"GRUP".to_vec();
    grup.extend(u32::try_from(group.len() + 24).unwrap().to_le_bytes());
    grup.extend(b"HDPT");
    grup.extend([0u8; 12]);
    bytes.extend(grup);
    bytes.extend(group);
    bytes
}

/// A well-formed headpart plugin with one MODL path.
fn headpart_plugin() -> Vec<u8> {
    plugin(&[(
        0,
        vec![
            (b"EDID", b"Head\0".to_vec()),
            (b"MODL", b"actors\\character\\head.nif\0".to_vec()),
        ],
    )])
}

#[test]
fn d17_fires_for_meshes_or_facegen_at_or_above_the_mod_root() {
    let work = std::env::temp_dir().join("Meshes").join("case");
    Host::shipped().at(work).fires(17, &case(|_| {}));
    let facegen_root = case(|case| {
        case["spec"]["mod_selection"]["folder"] = json!("mods/FaceGen Fixes");
        case["tree"]["content"] = json!([text("mods/FaceGen Fixes/readme.txt")]);
    });
    Host::shipped().fires(17, &facegen_root);
}

/// C++ cut a Mesh's path at its first `/meshes/`, so a `meshes` folder below
/// the Mod Root's top level gave it another game path than Rust's: a listed
/// `meshes/hair.nif` matched there, and a `facegen` folder before the cut was
/// dropped.
#[test]
fn d17_fires_for_a_mesh_under_a_nested_meshes_folder() {
    let host = Host::shipped();
    for mesh in [
        "mods/Mod/extras/meshes/hair.nif",
        "mods/Mod/FaceGen/Meshes/hair.nif",
    ] {
        host.fires(17, &with_content(json!([text(mesh)])));
    }
    host.quiet(
        17,
        &with_content(json!([
            text("mods/Mod/meshes/actors/meshes/hair.nif"),
            text("mods/Mod/extras/meshes/readme.txt"),
            text("mods/Mod/extras/hair.nif"),
        ])),
    );
}

#[test]
fn d17_fires_for_a_dry_run_over_a_facegen_mesh() {
    let dry_facegen = case(|case| {
        case["spec"]["dry_run"] = json!(true);
        case["spec"]["meshes"]["level"] = json!(1);
        case["tree"]["content"] = json!([text(
            "mods/Mod/meshes/actors/character/facegendata/facegeom/Mod.esp/00001.nif"
        )]);
    });
    Host::shipped().fires(17, &dry_facegen);
}

#[test]
fn d17_fires_for_a_plugin_in_staging_or_one_the_cpp_parser_mishandles() {
    let host = Host::shipped();
    host.fires(
        17,
        &with_content(json!([raw(
            "mods/Mod/.cao-staging/a.esp",
            &headpart_plugin()
        )])),
    );

    let whole = headpart_plugin();
    let truncated = &whole[..whole.len() - 4];
    host.fires(
        17,
        &with_content(json!([raw("mods/Mod/Cut.esp", truncated)])),
    );

    let compressed = plugin(&[(0x0004_0000, vec![(b"MODL", b"head.nif\0".to_vec())])]);
    host.fires(
        17,
        &with_content(json!([raw("mods/Mod/Packed.esp", &compressed)])),
    );

    let unterminated = plugin(&[(0, vec![(b"MODL", b"head.nif".to_vec())])]);
    host.fires(
        17,
        &with_content(json!([raw("mods/Mod/Open.esp", &unterminated)])),
    );

    let mut long = vec![b'a'; 1024];
    long.push(0);
    let overflowing = plugin(&[(0, vec![(b"MODL", long)])]);
    host.fires(
        17,
        &with_content(json!([raw("mods/Mod/Long.esp", &overflowing)])),
    );
}

#[test]
fn d17_is_quiet_for_well_formed_plugins_and_an_apply_over_facegen() {
    let host = Host::shipped();
    let dummy = cao_archive::Settings::get(cao_archive::Game::Sse).dummy_plugin;
    host.quiet(
        17,
        &with_content(json!([
            raw("mods/Mod/Mod.esp", &headpart_plugin()),
            raw("mods/Mod/Dummy.esp", dummy),
            raw("mods/Mod/Garbage.esm", &[1, 2, 3]),
            // A short TES4 header only stops C++'s read before any HDPT group.
            raw("mods/Mod/Short.esp", b"TES4 full plugin bytes"),
            text("mods/Mod/meshes/actors/character/facegendata/facegeom/Mod.esp/00001.nif")
        ])),
    );
    let apply_facegen = case(|case| {
        case["spec"]["meshes"]["level"] = json!(1);
        case["tree"]["content"] = json!([text(
            "mods/Mod/meshes/actors/character/facegendata/facegeom/Mod.esp/00001.nif"
        )]);
    });
    host.quiet(17, &apply_facegen);
}

#[test]
fn d18_fires_for_an_odd_target_while_resizing_by_size_is_off() {
    Host::shipped().fires(
        18,
        &case(|case| case["spec"]["textures"]["target_width"] = json!(63)),
    );
}

#[test]
fn d18_is_quiet_for_an_odd_target_while_resizing_by_size_is_on() {
    // Both builds reject it with the same Start Error.
    Host::shipped().quiet(
        18,
        &case(|case| {
            case["spec"]["textures"]["resize_by_size"] = json!(true);
            case["spec"]["textures"]["target_height"] = json!(63);
        }),
    );
}

/// A Several Mods case over `mods`, whose tree is `content`.
fn several_mods(content: Value) -> CaseFile {
    case(|case| {
        case["spec"]["mod_selection"] = json!({"kind": "several_mods", "folder": "mods"});
        case["tree"]["content"] = content;
    })
}

#[test]
fn d19_fires_for_a_several_mods_child_in_the_staging_namespace() {
    Host::shipped().fires(
        19,
        &several_mods(json!([
            text("mods/.CAO-staging-old/readme.txt"),
            text("mods/Mod/a.txt")
        ])),
    );
}

#[test]
fn d19_is_quiet_for_staging_inside_a_mod_root() {
    Host::shipped().quiet(
        19,
        &with_content(json!([text("mods/Mod/.cao-staging/readme.txt")])),
    );
}

#[test]
fn d20_fires_where_the_separator_rules_disagree() {
    let host = Host::shipped();
    host.fires(20, &several_mods(json!([text("mods/My separator/a.txt")])));
    let apply_root = case(|case| {
        case["spec"]["mod_selection"]["folder"] = json!("mods/Group_separator");
        case["tree"]["content"] = json!([text("mods/Group_separator/a.txt")]);
    });
    host.fires(20, &apply_root);
    host.fires(
        20,
        &with_content(json!([text("mods/Mod/Separators/a.txt")])),
    );
    let work = std::env::temp_dir().join("Separator").join("case");
    Host::shipped().at(work).fires(20, &case(|_| {}));
}

#[test]
fn d20_is_quiet_for_an_mo2_separator_child_and_a_dry_run() {
    let host = Host::shipped();
    host.quiet(
        20,
        &several_mods(json!([
            text("mods/Group_separator/a.txt"),
            text("mods/Mod/a.txt")
        ])),
    );
    let dry = case(|case| {
        case["spec"]["dry_run"] = json!(true);
        case["tree"]["content"] = json!([text("mods/Mod/Separators/a.txt")]);
    });
    host.quiet(20, &dry);
}

#[test]
fn d21_fires_for_fo4_with_textures_merged_into_the_main_ba2() {
    Host::shipped().fires(
        21,
        &case(|case| {
            case["spec"]["profile"] = json!("FO4");
            case["spec"]["archives"]["merge_textures"] = json!(true);
        }),
    );
}

#[test]
fn d21_is_quiet_for_merged_textures_in_another_game() {
    Host::shipped().quiet(
        21,
        &case(|case| case["spec"]["archives"]["merge_textures"] = json!(true)),
    );
}

#[test]
fn d22_fires_for_an_ampersand_in_a_logged_path() {
    Host::shipped().fires(
        22,
        &with_content(json!([text("mods/Mod/textures/R&D.txt")])),
    );
    let work = std::env::temp_dir().join("Guns & Ammo").join("case");
    Host::shipped().at(work).fires(22, &case(|_| {}));
}

#[test]
fn d22_is_quiet_for_paths_without_html_metacharacters() {
    Host::shipped().quiet(
        22,
        &with_content(json!([text("mods/Mod/textures/R+D (old).txt")])),
    );
}

#[test]
fn d23_fires_when_settings_turn_on_debug_logging() {
    let host = Host::copied("guard-d23");
    host.edit("SSE/settings.ini", |text| {
        text.replace("bDebugLog=false", "bDebugLog=true")
    });
    host.fires(23, &case(|_| {}));
}

#[test]
fn d23_is_quiet_for_the_shipped_settings() {
    Host::shipped().quiet(23, &case(|_| {}));
}

#[test]
fn d24_fires_for_a_top_level_logs_folder() {
    Host::shipped().fires(24, &with_content(json!([text("Logs/SSE/old.html")])));
}

#[test]
fn d24_is_quiet_for_a_logs_folder_inside_a_mod_root() {
    Host::shipped().quiet(24, &with_content(json!([text("mods/Mod/logs/old.html")])));
}
