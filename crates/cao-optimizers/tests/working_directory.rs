//! Nothing the composition root does resolves against the working directory:
//! the app directory is a parameter all the way down (deviation 1).
//!
//! This is its own test binary with one test, because it changes the process's
//! working directory, which every thread of a test binary shares.

mod common;

use cao_core::run::RunOutcome;
use cao_optimizers::composition::ApplicationRun;
use common::{app_dir, dry_run_textures, edit_profile, write_dds};
use directxtex::DXGI_FORMAT_R8G8B8A8_UNORM;

#[test]
fn profiles_resolve_against_the_app_directory_not_the_working_directory() {
    let app = app_dir("working-directory-app");
    let mod_root = app.join("mods").join("Mod");
    write_dds(
        &mod_root.join("textures/plain.dds"),
        DXGI_FORMAT_R8G8B8A8_UNORM,
        16,
    );
    // A decoy install as the working directory, whose SSE profile would fail
    // the run's Preparing if it were ever read.
    let decoy = app_dir("working-directory-decoy");
    edit_profile(&decoy, "SSE", "texturesEnabled", "false");
    edit_profile(&decoy, "SSE", "bsaGame", "7");
    std::env::set_current_dir(&decoy).unwrap();

    let run = ApplicationRun::new(&app, "SSE", &dry_run_textures(&app, &mod_root)).unwrap();
    let result = run.start(None).unwrap().wait();

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
}
