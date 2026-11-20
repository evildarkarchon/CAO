//! `cargo xtask`: see the library for the tasks.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use xtask::{EXE, Hkxcmd, Install, Profile, Task, USAGE, Workspace};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Parses the command line and runs its task.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = workspace();
    match xtask::parse(std::env::args_os().skip(1))? {
        Task::Help => print!("{USAGE}"),
        Task::Dist {
            profile,
            hkxcmd,
            out,
        } => dist(&workspace, profile, &hkxcmd, out)?,
        Task::Dev {
            profile,
            hkxcmd,
            force,
        } => dev(&workspace, profile, &hkxcmd, force)?,
    }
    Ok(())
}

/// The workspace this binary was built from, and the target directory Cargo
/// uses for it: `CARGO_TARGET_DIR` when set, otherwise `target/`.
fn workspace() -> Workspace {
    // This crate is `crates/xtask`, two levels below the root.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask sits two levels below the workspace root")
        .to_path_buf();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map_or_else(|| root.join("target"), |dir| root.join(dir));
    Workspace { root, target }
}

/// Builds `cao-gui` for `profile` and lays out its install folder.
fn dist(
    workspace: &Workspace,
    profile: Profile,
    hkxcmd: &Hkxcmd,
    out: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let converter = workspace.hkxcmd(hkxcmd);
    // Checked before the build, which takes minutes, so a missing converter
    // fails at once.
    if let Some(path) = converter.as_deref().filter(|path| !path.is_file()) {
        return Err(format!(
            "{} does not exist. Download hkxcmd.exe from \
             https://github.com/figment/hkxcmd/releases and pass --hkxcmd <path> \
             (or put it at bin/hkxcmd.exe), or pass --without-hkxcmd for an \
             install that cannot convert Animations",
            path.display()
        )
        .into());
    }
    build(profile)?;
    let folder = match out {
        Some(folder) => folder,
        None => {
            let folder = workspace.default_dist(profile);
            // dist owns its default folder under target/, so a stale install
            // there is replaced rather than refused.
            if folder.exists() {
                std::fs::remove_dir_all(&folder)?;
            }
            folder
        }
    };
    let exe = workspace.exe_directory(profile).join(EXE);
    let (profiles, license) = (workspace.profiles(), workspace.license());
    xtask::lay_out_install(
        &folder,
        &Install {
            exe: &exe,
            profiles: &profiles,
            license: &license,
            hkxcmd: converter.as_deref(),
        },
    )?;
    println!("Laid out {}", folder.display());
    if converter.is_none() {
        println!("  without bin/hkxcmd.exe: this install cannot convert Animations");
    }
    println!("  CAO links the dynamic CRT: users need the Visual C++ redistributable (x64).");
    Ok(())
}

/// Prepares `target/<profile>/` as a dev app directory.
fn dev(
    workspace: &Workspace,
    profile: Profile,
    hkxcmd: &Hkxcmd,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let app_dir = workspace.exe_directory(profile);
    let converter = workspace.hkxcmd(hkxcmd);
    if let Hkxcmd::At(path) = hkxcmd
        && !path.is_file()
    {
        return Err(format!("{} does not exist", path.display()).into());
    }
    let report = xtask::prepare_dev(&app_dir, &workspace.profiles(), converter.as_deref(), force)?;
    println!(
        "Prepared {}: {} profile files copied, {} kept",
        app_dir.display(),
        report.copied,
        report.kept
    );
    if report.kept > 0 {
        println!("  pass --force to replace the kept files with the tracked ones");
    }
    if !report.hkxcmd {
        println!(
            "  no bin/hkxcmd.exe was found, so Animations cannot be converted; \
             see https://github.com/figment/hkxcmd/releases"
        );
    }
    let release = if profile == Profile::Release {
        " --release"
    } else {
        ""
    };
    println!("Run it with: cargo run -p cao-gui{release}");
    Ok(())
}

/// Runs `cargo build -p cao-gui` for `profile` with the Cargo that runs this task.
fn build(profile: Profile) -> Result<(), Box<dyn std::error::Error>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.args(["build", "--package", "cao-gui"]);
    if profile == Profile::Release {
        command.arg("--release");
    }
    let status = command.status()?;
    if !status.success() {
        return Err(format!("cargo build -p cao-gui failed ({status})").into());
    }
    Ok(())
}
