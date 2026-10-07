// PROTOTYPE — style-independent pieces: CLI config and relaunch, Win32 modal helpers, mock
// profile data, and a simulated run worker. Nothing here touches disk.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use windows_sys::Win32::Foundation::HWND;

pub const STYLES: &[&str] = &["fluent", "material", "cosmic"];

/// Prototype variant selection. Every switcher click relaunches the process with new args,
/// because the widget style is fixed at compile time and muda is chosen at window creation.
#[derive(Clone, Debug)]
pub struct Config {
    pub style: String,
    pub menu: String,
    pub log: String,
    pub light: bool,
    pub pos: Option<(i32, i32)>,
    // Screenshot helpers only; not carried across relaunches.
    pub tab: Option<String>,
    pub advanced: bool,
    pub path: Option<String>,
}

impl Config {
    /// Parses `--style`, `--menu native|drawn`, `--log rows|plain`, `--light`, `--pos x,y`.
    pub fn from_args() -> Self {
        let mut cfg = Config { style: "fluent".into(), menu: "native".into(), log: "rows".into(), light: false, pos: None, tab: None, advanced: false, path: None };
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--style" => cfg.style = args.next().unwrap_or_default(),
                "--menu" => cfg.menu = args.next().unwrap_or_default(),
                "--log" => cfg.log = args.next().unwrap_or_default(),
                "--light" => cfg.light = true,
                "--tab" => cfg.tab = args.next(),
                "--advanced" => cfg.advanced = true,
                "--path" => cfg.path = args.next(),
                "--pos" => {
                    cfg.pos = args.next().and_then(|v| {
                        let (x, y) = v.split_once(',')?;
                        Some((x.parse().ok()?, y.parse().ok()?))
                    })
                }
                _ => {}
            }
        }
        if !STYLES.contains(&cfg.style.as_str()) {
            cfg.style = "fluent".into();
        }
        cfg
    }

    /// Returns the style `delta` steps away in STYLES, wrapping around.
    pub fn cycled_style(&self, delta: isize) -> String {
        let i = STYLES.iter().position(|s| *s == self.style).unwrap_or(0) as isize;
        let n = STYLES.len() as isize;
        STYLES[((i + delta).rem_euclid(n)) as usize].to_string()
    }

    /// Starts a fresh copy of this exe with this config. The caller quits its own event loop.
    pub fn relaunch(&self) {
        let mut cmd = std::process::Command::new(std::env::current_exe().expect("current_exe"));
        cmd.args(["--style", &self.style, "--menu", &self.menu, "--log", &self.log]);
        if self.light {
            cmd.arg("--light");
        }
        if let Some((x, y)) = self.pos {
            cmd.args(["--pos", &format!("{x},{y}")]);
        }
        let _ = cmd.spawn();
    }
}

// ---------------------------------------------------------------------------------------------
// Win32 helpers

/// The HWND behind a Slint window, once the winit window exists.
pub fn hwnd_of(w: &slint::Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = w.window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

/// Makes `owner` the Win32 owner of `child`: the child stays above it, minimises with it and
/// gets no taskbar button. Slint has no API for owned windows.
pub fn set_owner(child: HWND, owner: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, SetWindowLongPtrW};
    // SAFETY: both handles come from live winit windows on this (the UI) thread.
    unsafe {
        SetWindowLongPtrW(child, GWLP_HWNDPARENT, owner as isize);
    }
}

fn set_window_enabled(w: &slint::Window, enabled: bool) {
    use slint::winit_030::winit::platform::windows::WindowExtWindows;
    // No focus_window() on re-enable: on Windows winit fakes an Alt keypress to win the
    // foreground lock, which puts the native menu bar into keyboard mode and the user's next
    // click only dismisses it. Windows already re-activates an enabled owner when its owned
    // dialog hides, as long as the owner is enabled before the hide (see close_modal).
    w.with_winit_window(|ww| ww.set_enable(enabled));
}

thread_local! {
    // Nesting depth of open Slint dialogs. A counter, not a bool: the New Profile flow closes
    // one dialog and opens the next in the same turn, and a bool guard dropped late would
    // re-enable the main window under the second dialog.
    static MODAL_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Disables the main window while alive (Slint has no modal windows — slint#6607).
/// Drop it *before* hiding the dialog: if the owner is still disabled when its owned window
/// hides, Windows activates some other application instead of returning focus to CAO.
pub struct ModalGuard<T: ComponentHandle + 'static> {
    main: slint::Weak<T>,
}

impl<T: ComponentHandle + 'static> ModalGuard<T> {
    pub fn new(main: &T) -> Self {
        let depth = MODAL_DEPTH.with(|d| {
            d.set(d.get() + 1);
            d.get()
        });
        if depth == 1 {
            set_window_enabled(main.window(), false);
        }
        ModalGuard { main: main.as_weak() }
    }
}

impl<T: ComponentHandle + 'static> Drop for ModalGuard<T> {
    fn drop(&mut self) {
        let depth = MODAL_DEPTH.with(|d| {
            d.set(d.get().saturating_sub(1));
            d.get()
        });
        if depth == 0
            && let Some(main) = self.main.upgrade()
        {
            set_window_enabled(main.window(), true);
        }
    }
}

/// Shows `dlg` as an owned, centred, app-modal dialog over `main`. Keep the returned guard
/// alongside the dialog; see [`close_modal`].
pub fn show_modal<M: ComponentHandle + 'static, D: ComponentHandle + 'static>(main: &M, dlg: &D) -> ModalGuard<M> {
    let guard = ModalGuard::new(main);
    dlg.show().expect("show dialog");
    let (dw, mw) = (dlg.as_weak(), main.as_weak());
    // The winit window is created asynchronously, so owner and position are fixed up once it exists.
    slint::spawn_local(async move {
        let Some(d) = dw.upgrade() else { return };
        if d.window().winit_window().await.is_err() {
            return;
        }
        let Some(m) = mw.upgrade() else { return };
        if let (Some(c), Some(o)) = (hwnd_of(d.window()), hwnd_of(m.window())) {
            set_owner(c, o);
        }
        let (mp, ms, ds) = (m.window().position(), m.window().size(), d.window().size());
        d.window().set_position(slint::PhysicalPosition::new(
            mp.x + (ms.width as i32 - ds.width as i32) / 2,
            mp.y + (ms.height as i32 - ds.height as i32) / 3,
        ));
    })
    .expect("spawn_local");
    guard
}

/// Closes a dialog opened by [`show_modal`]. The slot is emptied now (so a follow-up dialog can
/// take it), but the teardown runs on the next turn: re-enable the owner first, then hide,
/// then drop. Deferring keeps the component alive until its own click handler has returned.
pub fn close_modal<M: ComponentHandle + 'static, D: ComponentHandle + 'static>(
    slot: &std::cell::RefCell<Option<(D, ModalGuard<M>)>>,
) {
    let Some((dlg, guard)) = slot.borrow_mut().take() else { return };
    slint::Timer::single_shot(Duration::ZERO, move || {
        drop(guard);
        let _ = dlg.hide();
        drop(dlg);
    });
}

/// Opens a URL or file in the default handler.
pub fn shell_open(target: &str) {
    let _ = std::process::Command::new("explorer").arg(target).spawn();
}

// ---------------------------------------------------------------------------------------------
// Mock profile data (mirrors profiles/*/profile.ini; never read or written)

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub base: bool,
    pub bsa: bool,
    pub meshes: bool,
    pub textures: bool,
    pub animations: bool,
    pub bsa_game_index: i32,
    pub max_size_gb: f32,
    pub stream_index: i32,
    pub output_format_index: i32,
    pub unwanted: Vec<String>,
}

pub fn base_profiles() -> Vec<Profile> {
    let p = |name: &str, meshes, animations, game, size, stream, fmt, unwanted: &[&str]| Profile {
        name: name.into(),
        base: true,
        bsa: true,
        meshes,
        textures: true,
        animations,
        bsa_game_index: game,
        max_size_gb: size,
        stream_index: stream,
        output_format_index: fmt,
        unwanted: unwanted.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        p("FO4", false, false, 2, 3.90, 3, 0, &["B5G5R5A1_UNORM", "B5G6R5_UNORM", "B4G4R4A4_UNORM"]),
        p("SSE", true, true, 1, 1.96, 2, 0, &["B5G6R5_UNORM", "B5G5R5A1_UNORM", "B4G4R4A4_UNORM"]),
        p("TES5", true, false, 0, 1.96, 1, 2, &["BC7_UNORM", "BC7_UNORM_SRGB"]),
    ]
}

/// The 75 names from src/texturesformats.h, in order.
pub const DXGI_FORMATS: &[&str] = &[
    "R32G32B32A32_FLOAT", "R32G32B32A32_UINT", "R32G32B32A32_SINT", "R32G32B32_FLOAT", "R32G32B32_UINT",
    "R32G32B32_SINT", "R16G16B16A16_FLOAT", "R16G16B16A16_UNORM", "R16G16B16A16_UINT", "R16G16B16A16_SNORM",
    "R16G16B16A16_SINT", "R32G32_FLOAT", "R32G32_UINT", "R32G32_SINT", "R10G10B10A2_UNORM", "R10G10B10A2_UINT",
    "R11G11B10_FLOAT", "R8G8B8A8_UNORM", "R8G8B8A8_UNORM_SRGB", "R8G8B8A8_UINT", "R8G8B8A8_SNORM",
    "R8G8B8A8_SINT", "R16G16_FLOAT", "R16G16_UNORM", "R16G16_UINT", "R16G16_SNORM", "R16G16_SINT", "R32_FLOAT",
    "R32_UINT", "R32_SINT", "R8G8_UNORM", "R8G8_UINT", "R8G8_SNORM", "R8G8_SINT", "R16_FLOAT", "R16_UNORM",
    "R16_UINT", "R16_SNORM", "R16_SINT", "R8_UNORM", "R8_UINT", "R8_SNORM", "R8_SINT", "A8_UNORM",
    "R9G9B9E5_SHAREDEXP", "R8G8_B8G8_UNORM", "G8R8_G8B8_UNORM", "BC1_UNORM", "BC1_UNORM_SRGB", "BC2_UNORM",
    "BC2_UNORM_SRGB", "BC3_UNORM", "BC3_UNORM_SRGB", "BC4_UNORM", "BC4_SNORM", "BC5_UNORM", "BC5_SNORM",
    "B5G6R5_UNORM", "B5G5R5A1_UNORM", "B8G8R8A8_UNORM", "B8G8R8X8_UNORM", "R10G10B10_XR_BIAS_A2_UNORM",
    "B8G8R8A8_UNORM_SRGB", "B8G8R8X8_UNORM_SRGB", "BC6H_UF16", "BC6H_SF16", "BC7_UNORM", "BC7_UNORM_SRGB",
    "AYUV", "Y410", "Y416", "YUY2", "Y210", "Y216", "B4G4R4A4_UNORM",
];

// ---------------------------------------------------------------------------------------------
// Simulated run

pub enum RunEvent {
    Phase { label: String, indeterminate: bool },
    Progress { completed: u64, total: u64, succeeded: u64, failed: u64, label: String },
    Log { text: String, level: i32 },
    Finished { label: String },
}

/// Like the C++ Run Handle: dropping it requests cancellation and waits for the worker.
pub struct RunHandle {
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RunHandle {
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

impl Drop for RunHandle {
    fn drop(&mut self) {
        self.request_cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Fakes a run on a worker thread: an indeterminate scan, then determinate per-file work.
/// Cancellation is only observed between files, like an uninterruptible nifly call.
pub fn start_run(mod_path: String, dry_run: bool, sink: impl Fn(RunEvent) + Send + 'static) -> RunHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let thread = std::thread::spawn(move || {
        let stamp = || {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
            format!("{:02}:{:02}:{:02}.{:03}", (t / 3_600_000) % 24, (t / 60_000) % 60, (t / 1000) % 60, t % 1000)
        };
        let log = |level: i32, sev: &str, msg: String| sink(RunEvent::Log { text: format!("{} {sev:<5} {msg}", stamp()), level });
        log(1, "INFO", format!("Run started on '{mod_path}'{}", if dry_run { " (dry run)" } else { "" }));
        sink(RunEvent::Phase { label: "Scanning files".into(), indeterminate: true });
        for i in 0..25 {
            if flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            if i % 6 == 0 {
                log(0, "DEBUG", format!("Scanned directory batch {i}"));
            }
        }
        let total = 80u64;
        let (mut ok, mut failed) = (0u64, 0u64);
        if !flag.load(Ordering::SeqCst) {
            for n in 1..=total {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(70));
                if n % 17 == 0 {
                    failed += 1;
                    log(3, "ERROR", format!("textures/armor/piece{n:03}_n.dds: unsupported format R8G8_B8G8_UNORM"));
                } else {
                    ok += 1;
                    if n % 9 == 0 {
                        log(2, "WARN", format!("meshes/clutter/item{n:03}.nif: headpart skipped"));
                    } else if n % 4 == 0 {
                        log(1, "INFO", format!("Processed textures/clutter/item{n:03}.dds"));
                    }
                }
                sink(RunEvent::Progress {
                    completed: n,
                    total,
                    succeeded: ok,
                    failed,
                    label: "Optimizing".into(),
                });
            }
        }
        let label = if flag.load(Ordering::SeqCst) { "Cancelled" } else if failed > 0 { "Completed with failures" } else { "Completed" };
        sink(RunEvent::Log { text: format!("Run outcome: {label} ({ok} succeeded, {failed} failed)"), level: 4 });
        sink(RunEvent::Finished { label: label.into() });
    });
    RunHandle { cancel, thread: Some(thread) }
}
