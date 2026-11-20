//! The xtask command line and the layouts its tasks write (WS-3).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use xtask::{
    EXE, Existing, HKXCMD, Hkxcmd, Install, Profile, Task, Workspace, copy_tree, lay_out_install,
    parse, prepare_dev,
};

/// A fresh, empty directory for one scenario.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("xtask")
        .join(name);
    // A missing directory is the expected case; anything else surfaces below.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn args(text: &[&str]) -> Vec<OsString> {
    text.iter().map(OsString::from).collect()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// A stand-in for the workspace's inputs: an exe, a profiles tree with a
/// nested profile, CAO's and nifly's licences and a converter.
struct Inputs {
    exe: PathBuf,
    profiles: PathBuf,
    license: PathBuf,
    nifly_license: PathBuf,
    hkxcmd: PathBuf,
}

fn inputs(dir: &Path) -> Inputs {
    let inputs = Inputs {
        exe: dir.join("build").join(EXE),
        profiles: dir.join("profiles"),
        license: dir.join("LICENSE"),
        nifly_license: dir.join("nifly").join("LICENSE"),
        hkxcmd: dir.join("bin").join(HKXCMD),
    };
    write(&inputs.exe, "exe");
    write(&inputs.profiles.join("common.ini"), "common");
    write(
        &inputs.profiles.join("SSE").join("settings.ini"),
        "sse settings",
    );
    write(&inputs.license, "MPL");
    write(&inputs.nifly_license, "GPL");
    write(&inputs.hkxcmd, "converter");
    inputs
}

#[test]
fn no_task_and_help_print_the_usage() {
    assert_eq!(parse(args(&[])), Ok(Task::Help));
    for help in ["help", "--help", "-h"] {
        assert_eq!(parse(args(&[help])), Ok(Task::Help));
    }
}

#[test]
fn dist_defaults_to_the_dev_profile_and_the_workspace_converter() {
    assert_eq!(
        parse(args(&["dist"])),
        Ok(Task::Dist {
            profile: Profile::Debug,
            hkxcmd: Hkxcmd::Default,
            out: None,
        })
    );
}

#[test]
fn dist_takes_a_profile_a_converter_and_a_folder() {
    assert_eq!(
        parse(args(&[
            "dist",
            "--release",
            "--hkxcmd",
            "C:/tools/hkxcmd.exe",
            "--out",
            "D:/CAO"
        ])),
        Ok(Task::Dist {
            profile: Profile::Release,
            hkxcmd: Hkxcmd::At(PathBuf::from("C:/tools/hkxcmd.exe")),
            out: Some(PathBuf::from("D:/CAO")),
        })
    );
    assert_eq!(
        parse(args(&["dist", "--without-hkxcmd"])),
        Ok(Task::Dist {
            profile: Profile::Debug,
            hkxcmd: Hkxcmd::Without,
            out: None,
        })
    );
}

#[test]
fn dev_takes_force_and_a_converter() {
    assert_eq!(
        parse(args(&["dev", "--force", "--hkxcmd", "h.exe"])),
        Ok(Task::Dev {
            profile: Profile::Debug,
            hkxcmd: Hkxcmd::At(PathBuf::from("h.exe")),
            force: true,
        })
    );
}

#[test]
fn bad_command_lines_are_refused() {
    for bad in [
        &["build"][..],
        &["dist", "--force"],
        &["dev", "--out", "x"],
        &["dev", "--without-hkxcmd"],
        &["dist", "--hkxcmd"],
        &["dist", "--hkxcmd", "h.exe", "--without-hkxcmd"],
        &["dist", "--without-hkxcmd", "--hkxcmd", "h.exe"],
    ] {
        assert!(parse(args(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn the_workspace_names_its_folders_by_profile() {
    let workspace = Workspace {
        root: PathBuf::from("C:/CAO"),
        target: PathBuf::from("C:/CAO/target"),
    };
    assert_eq!(
        workspace.exe_directory(Profile::Release),
        Path::new("C:/CAO/target/release")
    );
    assert_eq!(
        workspace.default_dist(Profile::Debug),
        Path::new("C:/CAO/target/dist/debug")
    );
    assert_eq!(
        workspace.hkxcmd(&Hkxcmd::Default),
        Some(PathBuf::from("C:/CAO/bin").join(HKXCMD))
    );
    assert_eq!(workspace.hkxcmd(&Hkxcmd::Without), None);
}

#[test]
fn an_install_holds_the_exe_profiles_licence_and_converter() {
    let dir = scratch_dir("install");
    let inputs = inputs(&dir);
    let folder = dir.join("out");

    lay_out_install(
        &folder,
        &Install {
            exe: &inputs.exe,
            profiles: &inputs.profiles,
            license: &inputs.license,
            nifly_license: &inputs.nifly_license,
            hkxcmd: Some(&inputs.hkxcmd),
        },
    )
    .unwrap();

    assert_eq!(read(&folder.join(EXE)), "exe");
    assert_eq!(read(&folder.join("profiles/common.ini")), "common");
    assert_eq!(
        read(&folder.join("profiles/SSE/settings.ini")),
        "sse settings"
    );
    assert_eq!(read(&folder.join("LICENSE")), "MPL");
    assert_eq!(read(&folder.join("LICENSE-nifly")), "GPL");
    assert_eq!(read(&folder.join("bin").join(HKXCMD)), "converter");
}

#[test]
fn an_install_without_a_converter_has_no_bin_folder() {
    let dir = scratch_dir("install-without");
    let inputs = inputs(&dir);
    let folder = dir.join("out");

    lay_out_install(
        &folder,
        &Install {
            exe: &inputs.exe,
            profiles: &inputs.profiles,
            license: &inputs.license,
            nifly_license: &inputs.nifly_license,
            hkxcmd: None,
        },
    )
    .unwrap();

    assert!(folder.join(EXE).is_file());
    assert!(!folder.join("bin").exists());
}

/// An install never mixes with another folder's files, and a missing input
/// fails before anything is written.
#[test]
fn an_install_refuses_a_non_empty_folder_and_missing_inputs() {
    let dir = scratch_dir("install-refusals");
    let inputs = inputs(&dir);
    let install = Install {
        exe: &inputs.exe,
        profiles: &inputs.profiles,
        license: &inputs.license,
        nifly_license: &inputs.nifly_license,
        hkxcmd: Some(&inputs.hkxcmd),
    };
    let occupied = dir.join("occupied");
    write(&occupied.join("keep.txt"), "mine");

    let error = lay_out_install(&occupied, &install).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(read(&occupied.join("keep.txt")), "mine");
    assert!(!occupied.join(EXE).exists());

    let missing = dir.join("missing.exe");
    let fresh = dir.join("fresh");
    let error = lay_out_install(
        &fresh,
        &Install {
            hkxcmd: Some(&missing),
            ..install
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(!fresh.exists());

    let error = lay_out_install(
        &fresh,
        &Install {
            nifly_license: &dir.join("missing-licence"),
            ..install
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(!fresh.exists());
}

/// The exe links nifly statically, so dist must find nifly's GPL text where
/// the vendored source keeps it; a move of the vendor tree breaks this first.
#[test]
fn the_workspace_names_nifly_s_vendored_licence() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let workspace = Workspace {
        target: root.join("target"),
        root,
    };
    let text = std::fs::read_to_string(workspace.nifly_license()).unwrap();
    assert!(text.contains("GNU GENERAL PUBLIC LICENSE"), "{text:.80}");
}

/// The app edits its own profile files, so dev keeps them unless forced.
#[test]
fn dev_keeps_edited_profile_files_unless_forced() {
    let dir = scratch_dir("dev");
    let inputs = inputs(&dir);
    let app_dir = dir.join("target-debug");
    write(&app_dir.join("profiles/common.ini"), "edited by the app");

    let report = prepare_dev(&app_dir, &inputs.profiles, Some(&inputs.hkxcmd), false).unwrap();
    assert_eq!((report.copied, report.kept, report.hkxcmd), (1, 1, true));
    assert_eq!(
        read(&app_dir.join("profiles/common.ini")),
        "edited by the app"
    );
    assert_eq!(
        read(&app_dir.join("profiles/SSE/settings.ini")),
        "sse settings"
    );
    assert_eq!(read(&app_dir.join("bin").join(HKXCMD)), "converter");

    let report = prepare_dev(&app_dir, &inputs.profiles, None, true).unwrap();
    assert_eq!((report.copied, report.kept, report.hkxcmd), (2, 0, false));
    assert_eq!(read(&app_dir.join("profiles/common.ini")), "common");
}

/// Only Animation conversion needs the converter, so a missing one is not an
/// error for dev.
#[test]
fn dev_without_a_converter_still_copies_the_profiles() {
    let dir = scratch_dir("dev-without");
    let inputs = inputs(&dir);
    let app_dir = dir.join("target-debug");

    let report = prepare_dev(
        &app_dir,
        &inputs.profiles,
        Some(&dir.join("absent.exe")),
        false,
    )
    .unwrap();

    assert_eq!((report.copied, report.hkxcmd), (2, false));
    assert!(!app_dir.join("bin").exists());
}

#[test]
fn copy_tree_counts_copied_and_kept_files() {
    let dir = scratch_dir("copy-tree");
    let from = dir.join("from");
    write(&from.join("a.txt"), "a");
    write(&from.join("nested/b.txt"), "b");
    let to = dir.join("to");
    write(&to.join("a.txt"), "old");

    assert_eq!(copy_tree(&from, &to, Existing::Keep).unwrap(), (1, 1));
    assert_eq!(read(&to.join("a.txt")), "old");
    assert_eq!(copy_tree(&from, &to, Existing::Replace).unwrap(), (2, 0));
    assert_eq!(read(&to.join("a.txt")), "a");
}
