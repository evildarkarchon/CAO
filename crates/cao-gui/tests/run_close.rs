//! Closing the window during a run (#510): the close cancels the run, the
//! window stays open through Safety Cleanup until the run reports its outcome,
//! and then closes with the controls still locked. This carries the intent of
//! the dropped C++ `GuiRunDispatchTests::closeWaitsForCleanupAndTerminalDelivery`.
//!
//! Run events reach the window through the event loop, so this file starts the
//! event-loop test backend, which a process may do once: it holds one test.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use slint::ComponentHandle;
use slint::platform::WindowEvent;

/// How many TGAs the Mod Root holds: enough conversions that the run is still
/// converting when the close arrives, long after its first one staged a file.
const TEXTURES: usize = 40;

/// The staging control files a run leaves in `.cao-staging` once Safety
/// Cleanup has removed everything it staged.
const STAGING_CONTROL_FILES: [&str; 2] = ["owner.lock", "ownership.manifest"];

/// A `size`×`size` uncompressed 32-bit TGA with a gradient, which the SSE
/// profile converts to DDS (`texturesConvertTga`).
fn tga(size: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 18];
    // Uncompressed true-colour, no colour map, origin at the top left.
    bytes[2] = 2;
    bytes[12..14].copy_from_slice(&size.to_le_bytes());
    bytes[14..16].copy_from_slice(&size.to_le_bytes());
    bytes[16] = 32;
    bytes[17] = 0x28;
    let pixels = usize::from(size) * usize::from(size);
    for index in 0..pixels {
        let value = (index * 7) as u8;
        bytes.extend_from_slice(&[value, value.wrapping_mul(3), value.wrapping_add(90), 255]);
    }
    bytes
}

/// The run child Asset staging creates in `mod_root/.cao-staging`, if any.
fn run_child(mod_root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(mod_root.join(".cao-staging"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
}

/// Every path under `root` whose name marks it as staging, other than the
/// control files Safety Cleanup leaves in `.cao-staging`.
fn staging_debris(root: &Path) -> Vec<PathBuf> {
    let mut debris = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let in_staging = directory.file_name().is_some_and(|dir| dir == ".cao-staging");
            let control_file = in_staging && STAGING_CONTROL_FILES.contains(&name.as_str());
            if (in_staging && !control_file) || name.starts_with(".cao-staging-") {
                debris.push(path.clone());
            }
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            }
        }
    }
    debris
}

#[test]
fn closing_mid_run_cancels_it_and_closes_once_safety_cleanup_has_reported() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let app_dir = app_dir("run_close");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let mods = app_dir.join("mod");
    std::fs::create_dir_all(mods.join("textures")).unwrap();
    let texture = tga(256);
    for index in 0..TEXTURES {
        std::fs::write(mods.join(format!("textures/t{index:02}.tga")), &texture).unwrap();
    }
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    ui.set_user_path(mods.display().to_string().into());
    ui.set_dry_run(false);
    click(ui, "Run");
    assert!(ui.get_running());

    // The worker runs without the event loop; once it has staged its first
    // conversion, the run is mid-work with staging on disk to clean up.
    let deadline = Instant::now() + Duration::from_secs(60);
    while run_child(&mods).is_none() {
        assert!(Instant::now() < deadline, "the run never staged a file");
        std::thread::sleep(Duration::from_millis(1));
    }

    // The title-bar close, while the run's events are still undelivered.
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(ui.window().is_visible(), "the window waits for the run");
    assert!(
        ui.get_running() && ui.get_cancelling(),
        "the run is cancelled"
    );
    assert!(!enabled(ui, "Cancel"));
    assert!(
        ui.get_status_text().starts_with("Cancelling - "),
        "{}",
        ui.get_status_text()
    );

    run_event_loop_until(|| !ui.window().is_visible());
    // The terminal result, published only after Safety Cleanup, was delivered
    // before the window closed, and the run stopped short of its work.
    let details = detail_rows(ui);
    assert!(
        details.iter().any(|row| row.starts_with("Final Phase: ")),
        "{details:?}"
    );
    let status = ui.get_status_text();
    assert!(status.starts_with("Cancelled - "), "{status}");
    assert!(
        details.contains(&"Cancellation Observed: yes".to_owned()),
        "{details:?}"
    );
    // The controls stay locked until the window is gone, and no outcome box
    // pops up over a closing window.
    assert!(ui.get_running());
    assert!(desktop.message_titles().is_empty());
    // Safety Cleanup removed the run child and every staged sibling, leaving
    // only the control files.
    let debris = staging_debris(&mods);
    assert!(debris.is_empty(), "{debris:?}");
}
