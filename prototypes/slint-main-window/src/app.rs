// PROTOTYPE — the window logic, textually included once per style module by main.rs.
// `super::*` is that style's generated Slint types (MainWindow, FormatsDialog, …).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, FilterModel, Model, ModelRc, SharedString, VecModel};

use crate::shared::{self, Config, ModalGuard, Profile, RunEvent, RunHandle};

type Slot<D> = RefCell<Option<(D, ModalGuard<MainWindow>)>>;

struct State {
    ui: slint::Weak<MainWindow>,
    cfg: RefCell<Config>,
    profiles: RefCell<Vec<Profile>>,
    names: Rc<VecModel<SharedString>>,
    tabs: Rc<VecModel<TabInfo>>,
    log: Rc<VecModel<LogRow>>,
    log_plain: RefCell<String>,
    unwanted: Rc<VecModel<SharedString>>,
    run: RefCell<Option<RunHandle>>,
    close_requested: Cell<bool>,
    formats: Slot<FormatsDialog>,
    text_dlg: Slot<TextInputDialog>,
    choice_dlg: Slot<ChoiceDialog>,
    about_dlg: Slot<AboutSlintDialog>,
}

thread_local! {
    // Run events arrive through upgrade_in_event_loop, whose closure only gets the MainWindow;
    // the non-Send State is reached from here instead.
    static STATE: RefCell<Option<Rc<State>>> = const { RefCell::new(None) };
}

fn state() -> Rc<State> {
    STATE.with(|s| s.borrow().clone().expect("state initialised"))
}

pub fn run(cfg: Config) -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let profiles = shared::base_profiles();
    let tab = |id: &str, title: &str| TabInfo { id: id.into(), title: title.into(), enabled: true };
    let st = Rc::new(State {
        ui: ui.as_weak(),
        names: Rc::new(VecModel::from(profiles.iter().map(|p| SharedString::from(p.name.as_str())).collect::<Vec<_>>())),
        profiles: RefCell::new(profiles),
        tabs: Rc::new(VecModel::from(vec![
            tab("bsa", "BSA"),
            tab("meshes", "Meshes"),
            tab("textures", "Textures"),
            tab("animations", "Animations"),
            tab("log", "Log"),
        ])),
        log: Rc::new(VecModel::default()),
        log_plain: RefCell::new(String::new()),
        unwanted: Rc::new(VecModel::default()),
        run: RefCell::new(None),
        close_requested: Cell::new(false),
        formats: RefCell::new(None),
        text_dlg: RefCell::new(None),
        choice_dlg: RefCell::new(None),
        about_dlg: RefCell::new(None),
        cfg: RefCell::new(cfg.clone()),
    });
    STATE.with(|s| *s.borrow_mut() = Some(st.clone()));

    ui.set_profiles(ModelRc::from(st.names.clone()));
    ui.set_tabs(ModelRc::from(st.tabs.clone()));
    ui.set_log_rows(ModelRc::from(st.log.clone()));
    ui.set_unwanted(ModelRc::from(st.unwanted.clone()));
    ui.set_dark(!cfg.light);
    ui.set_proto_style(cfg.style.clone().into());
    ui.set_proto_menu(cfg.menu.clone().into());
    ui.set_proto_log(cfg.log.clone().into());
    ui.set_progress_text("0%".into());
    if let Some((x, y)) = cfg.pos {
        ui.window().set_position(slint::PhysicalPosition::new(x, y));
    }

    // common.ini's remembered profile would be applied here; SSE stands in for it.
    ui.set_profile_index(1);
    apply_profile(&st, 1);
    ui.set_show_advanced(cfg.advanced);
    if let Some(path) = &cfg.path {
        ui.set_user_path(path.as_str().into());
    }
    if let Some(tab) = &cfg.tab {
        ui.set_current_tab(tab.as_str().into());
    }

    for (lvl, text) in [
        (1, "12:00:00.000 INFO  Cathedral Assets Optimizer (prototype) started"),
        (0, "12:00:00.004 DEBUG Profiles found: FO4, SSE, TES5"),
        (2, "12:00:00.010 WARN  This is what a warning line looks like"),
        (3, "12:00:00.011 ERROR This is what an error line looks like"),
    ] {
        push_log(&st, text.to_string(), lvl);
    }

    wire_callbacks(&ui);
    hook_folder_drop(&ui);

    ui.window().on_close_requested(|| {
        let st = state();
        let ui = st.ui.unwrap();
        if ui.get_running() {
            // Mirrors MainWindow::closeEvent: cancel, keep the window, close once the run ends.
            st.close_requested.set(true);
            cancel_run(&st);
            return slint::CloseRequestResponse::KeepWindowShown;
        }
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    ui.run()
}

fn wire_callbacks(ui: &MainWindow) {
    ui.on_profile_selected(|i| apply_profile(&state(), i as usize));
    ui.on_new_profile(|| new_profile(&state()));
    ui.on_open_directory(|| open_directory(&state()));
    ui.on_run_clicked(|| run_clicked(&state()));
    ui.on_edit_unwanted(|| edit_unwanted(&state()));
    ui.on_move_tab(|from, to| {
        let st = state();
        let t = st.tabs.remove(from as usize);
        st.tabs.insert(to as usize, t);
    });
    ui.on_mode_activated(|i| {
        let st = state();
        if i == 1 {
            st.ui.unwrap().set_meshes_necessary(true);
            tutorial(
                &st,
                "Several mods option",
                "You have selected the several mods option. This process may take a very long time, especially if you process BSA. \nThis process has only been tested on the Mod Organizer mods folder.",
                || {},
            );
        }
    });
    ui.on_advanced_clicked(|_| {
        tutorial(&state(), "Advanced settings", "Advanced settings can only be modified when using custom profiles.", || {});
    });

    ui.on_open_log(|| {
        let st = state();
        message(&st, rfd::MessageLevel::Info, "Open log file", "PROTOTYPE: this would open <exe dir>/logs/<profile>.html.", || {});
    });
    ui.on_open_docs(|| shared::shell_open("https://www.nexusmods.com/skyrimspecialedition/mods/23316"));
    ui.on_open_discord(|| shared::shell_open("https://discordapp.com/invite/B9abN8d"));
    ui.on_about(|| {
        message(
            &state(),
            rfd::MessageLevel::Info,
            "About",
            "Cathedral Assets Optimizer (Slint prototype)\nMade by G'k\nThis program is distributed in the hope that it will be useful but WITHOUT ANY WARRANTY. See the GNU General Public License.",
            || {},
        )
    });
    ui.on_about_slint(|| {
        let st = state();
        let ui = st.ui.unwrap();
        let d = AboutSlintDialog::new().unwrap();
        d.on_confirm(|| shared::close_modal(&state().about_dlg));
        d.window().on_close_requested(|| defer_close(|st| &st.about_dlg));
        let g = shared::show_modal(&ui, &d);
        *st.about_dlg.borrow_mut() = Some((d, g));
    });

    ui.on_proto_prev_style(|| relaunch_with(|c| c.style = c.cycled_style(-1)));
    ui.on_proto_next_style(|| relaunch_with(|c| c.style = c.cycled_style(1)));
    ui.on_proto_toggle_menu(|| relaunch_with(|c| c.menu = if c.menu == "native" { "drawn".into() } else { "native".into() }));
    ui.on_proto_toggle_log(|| {
        let st = state();
        let mut cfg = st.cfg.borrow_mut();
        cfg.log = if cfg.log == "rows" { "plain".into() } else { "rows".into() };
        st.ui.unwrap().set_proto_log(cfg.log.clone().into());
    });
}

/// Closing from the title-bar X: defer to the next turn so the dialog isn't torn down inside
/// its own close-request handler.
fn defer_close<D: ComponentHandle + 'static>(slot: fn(&State) -> &Slot<D>) -> slint::CloseRequestResponse {
    slint::Timer::single_shot(std::time::Duration::ZERO, move || shared::close_modal(slot(&state())));
    slint::CloseRequestResponse::KeepWindowShown
}

fn relaunch_with(edit: impl FnOnce(&mut Config)) {
    let st = state();
    let ui = st.ui.unwrap();
    let mut cfg = st.cfg.borrow().clone();
    edit(&mut cfg);
    let p = ui.window().position();
    cfg.pos = Some((p.x, p.y));
    cfg.light = !ui.get_dark();
    cfg.relaunch();
    let _ = ui.hide();
    let _ = slint::quit_event_loop();
}

/// setGameMode: per-profile tab enable, base-profile read-only, and the profile's settings.
fn apply_profile(st: &Rc<State>, idx: usize) {
    let ui = st.ui.unwrap();
    let Some(p) = st.profiles.borrow().get(idx).cloned() else { return };
    ui.set_base_profile(p.base);
    for i in 0..st.tabs.row_count() {
        let mut t = st.tabs.row_data(i).unwrap();
        t.enabled = match t.id.as_str() {
            "bsa" => p.bsa,
            "meshes" => p.meshes,
            "textures" => p.textures,
            "animations" => p.animations,
            _ => true,
        };
        st.tabs.set_row_data(i, t);
    }
    // QTabBar moves the selection off a tab that becomes disabled; pick the first enabled one.
    let current = ui.get_current_tab();
    if !st.tabs.iter().any(|t| t.id == current && t.enabled)
        && let Some(t) = st.tabs.iter().find(|t| t.enabled)
    {
        ui.set_current_tab(t.id);
    }
    ui.set_bsa_game_index(p.bsa_game_index);
    ui.set_bsa_max_size(p.max_size_gb);
    ui.set_meshes_stream_index(p.stream_index);
    ui.set_output_format_index(p.output_format_index);
    st.unwanted.set_vec(p.unwanted.iter().map(|s| SharedString::from(s.as_str())).collect::<Vec<_>>());

    // Deviation (map Notes): the Dry Run and Several-mods rules also apply on load, not only on click.
    if ui.get_dry_run() {
        ui.set_bsa_extract(false);
        ui.set_bsa_create(false);
        ui.set_bsa_delete_backups(false);
    }
    if ui.get_mode_index() == 1 {
        ui.set_meshes_necessary(true);
    }
}

fn push_log(st: &Rc<State>, text: String, level: i32) {
    {
        let mut plain = st.log_plain.borrow_mut();
        plain.push_str(&text);
        plain.push('\n');
    }
    st.log.push(LogRow { text: text.into(), level });
    if let Some(ui) = st.ui.upgrade() {
        ui.set_log_plain(st.log_plain.borrow().as_str().into());
    }
}

/// QMessageBox through rfd: native, and modal to the main window via its HWND parent.
fn message(st: &Rc<State>, level: rfd::MessageLevel, title: &str, text: &str, then: impl FnOnce() + 'static) {
    let ui = st.ui.unwrap();
    let dlg = rfd::AsyncMessageDialog::new()
        .set_level(level)
        .set_title(title)
        .set_description(text)
        .set_buttons(rfd::MessageButtons::Ok)
        .set_parent(&ui.window().window_handle());
    slint::spawn_local(async move {
        dlg.show().await;
        then();
    })
    .expect("spawn_local");
}

/// showTutorialWindow: only when Tools > Show tutorials is ticked.
fn tutorial(st: &Rc<State>, title: &str, text: &str, then: impl FnOnce() + 'static) {
    if st.ui.unwrap().get_show_tutorials() {
        message(st, rfd::MessageLevel::Info, title, text, then);
    } else {
        then();
    }
}

fn open_directory(st: &Rc<State>) {
    let ui = st.ui.unwrap();
    let mut dlg = rfd::AsyncFileDialog::new().set_title("Open Directory").set_parent(&ui.window().window_handle());
    let current = ui.get_user_path().to_string();
    if std::path::Path::new(&current).is_dir() {
        dlg = dlg.set_directory(&current);
    }
    let weak = ui.as_weak();
    slint::spawn_local(async move {
        if let Some(folder) = dlg.pick_folder().await
            && let Some(ui) = weak.upgrade()
        {
            ui.set_user_path(folder.path().display().to_string().into());
        }
    })
    .expect("spawn_local");
}

/// The window accepts OS folder drops through winit's DroppedFile; Slint's DropArea only sees
/// drags inside the app (slint#1967).
fn hook_folder_drop(ui: &MainWindow) {
    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |_, ev| {
        let Some(ui) = weak.upgrade() else { return EventResult::Propagate };
        match ev {
            WindowEvent::HoveredFile(_) => ui.set_drop_hover(true),
            WindowEvent::HoveredFileCancelled => ui.set_drop_hover(false),
            WindowEvent::DroppedFile(path) => {
                ui.set_drop_hover(false);
                // MainWindow::dropEvent accepts any existing path (QDir::exists is true for files).
                if path.exists() {
                    ui.set_user_path(path.display().to_string().into());
                }
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

fn new_profile(st: &Rc<State>) {
    tutorial(
        st,
        "New profile",
        "You are about to create a new profile. It will create a new directory in 'CAO/profiles'. Please check it out after creation, some files will be created inside it.\n\nPROTOTYPE: nothing is written; the profile lives in memory.",
        || open_name_dialog(&state()),
    );
}

fn open_name_dialog(st: &Rc<State>) {
    let ui = st.ui.unwrap();
    let d = TextInputDialog::new().unwrap();
    d.set_dialog_title("New profile".into());
    d.set_label("Name:".into());
    let dw = d.as_weak();
    d.on_confirm(move || {
        let name = dw.unwrap().get_value().to_string();
        shared::close_modal(&state().text_dlg);
        if !name.trim().is_empty() {
            open_base_dialog(&state(), name);
        }
    });
    d.on_dismiss(|| shared::close_modal(&state().text_dlg));
    d.window().on_close_requested(|| defer_close(|st| &st.text_dlg));
    let g = shared::show_modal(&ui, &d);
    *st.text_dlg.borrow_mut() = Some((d, g));
}

fn open_base_dialog(st: &Rc<State>, name: String) {
    let ui = st.ui.unwrap();
    let d = ChoiceDialog::new().unwrap();
    d.set_dialog_title("Base profile".into());
    d.set_label("Which profile do you want to use as a base?".into());
    d.set_choices(ModelRc::from(st.names.clone()));
    d.set_current_index(ui.get_profile_index());
    let dw = d.as_weak();
    d.on_confirm(move || {
        let base = dw.unwrap().get_current_index() as usize;
        shared::close_modal(&state().choice_dlg);
        let st = state();
        let Some(mut p) = st.profiles.borrow().get(base).cloned() else { return };
        p.name = name.clone();
        p.base = false;
        st.profiles.borrow_mut().push(p);
        st.names.push(name.as_str().into());
        let idx = st.names.row_count() - 1;
        st.ui.unwrap().set_profile_index(idx as i32);
        apply_profile(&st, idx);
    });
    d.on_dismiss(|| shared::close_modal(&state().choice_dlg));
    d.window().on_close_requested(|| defer_close(|st| &st.choice_dlg));
    let g = shared::show_modal(&ui, &d);
    *st.choice_dlg.borrow_mut() = Some((d, g));
}

/// TexturesFormatSelectDialog. Edits go to a scratch model; only OK copies them back, so
/// Cancel/X revert (the deviation-list fix — Qt applies the choices on any close).
fn edit_unwanted(st: &Rc<State>) {
    let ui = st.ui.unwrap();
    let current: Vec<String> = st.unwanted.iter().map(|s| s.to_string()).collect();
    let source = Rc::new(VecModel::from(
        shared::DXGI_FORMATS
            .iter()
            .map(|n| FormatItem { name: (*n).into(), checked: current.iter().any(|c| c == n) })
            .collect::<Vec<_>>(),
    ));
    let needle = Rc::new(RefCell::new(String::new()));
    let filtered = Rc::new(FilterModel::new(source.clone(), {
        let needle = needle.clone();
        // Qt::MatchContains is case-insensitive.
        move |it: &FormatItem| it.name.to_lowercase().contains(&needle.borrow().to_lowercase())
    }));

    let d = FormatsDialog::new().unwrap();
    d.set_items(ModelRc::from(filtered.clone()));
    d.on_search_edited(move |t| {
        *needle.borrow_mut() = t.to_string();
        filtered.reset();
    });
    d.on_confirm(move || {
        let st = state();
        let chosen: Vec<SharedString> = source.iter().filter(|i| i.checked).map(|i| i.name).collect();
        st.unwanted.set_vec(chosen);
        shared::close_modal(&st.formats);
    });
    d.on_dismiss(|| shared::close_modal(&state().formats));
    d.window().on_close_requested(|| defer_close(|st| &st.formats));
    let g = shared::show_modal(&ui, &d);
    *st.formats.borrow_mut() = Some((d, g));
}

fn cancel_run(st: &Rc<State>) {
    let ui = st.ui.unwrap();
    if let Some(h) = st.run.borrow().as_ref() {
        h.request_cancel();
    }
    ui.set_cancelling(true);
    ui.set_progress_text("Cancelling…".into());
    ui.set_status_text("Cancelling…".into());
}

fn run_clicked(st: &Rc<State>) {
    let ui = st.ui.unwrap();
    if ui.get_running() {
        cancel_run(st);
        return;
    }
    let path = ui.get_user_path().to_string();
    if !std::path::Path::new(path.trim()).is_dir() {
        message(st, rfd::MessageLevel::Error, "Start Error", "The run could not start: choose an existing mod folder first (Open Directory, or drop one on the window).", || {});
        return;
    }
    st.log.set_vec(Vec::new());
    st.log_plain.borrow_mut().clear();
    ui.set_running(true);
    ui.set_cancelling(false);
    ui.set_progress_indeterminate(true);
    ui.set_progress(0.0);
    ui.set_progress_text("Starting".into());
    ui.set_status_text("Starting".into());
    let weak = ui.as_weak();
    let handle = shared::start_run(path, ui.get_dry_run(), move |ev| {
        // Queued to the UI thread; fails harmlessly once the event loop is gone.
        let _ = weak.upgrade_in_event_loop(move |_| on_run_event(ev));
    });
    *st.run.borrow_mut() = Some(handle);
}

fn on_run_event(ev: RunEvent) {
    let st = state();
    let ui = st.ui.unwrap();
    match ev {
        RunEvent::Phase { label, indeterminate } => {
            ui.set_progress_indeterminate(indeterminate);
            if !ui.get_cancelling() {
                ui.set_progress_text(label.as_str().into());
                ui.set_status_text(label.into());
            }
        }
        RunEvent::Progress { completed, total, succeeded, failed, label } => {
            ui.set_progress_indeterminate(false);
            ui.set_progress(completed as f32 / total.max(1) as f32);
            if !ui.get_cancelling() {
                let text = format!("{label} - {completed} / {total} attempts ({succeeded} succeeded, {failed} failed)");
                ui.set_progress_text(text.as_str().into());
                ui.set_status_text(text.into());
            }
        }
        RunEvent::Log { text, level } => push_log(&st, text, level),
        RunEvent::Finished { label } => {
            ui.set_running(false);
            ui.set_cancelling(false);
            ui.set_progress_indeterminate(false);
            ui.set_progress_text(label.as_str().into());
            ui.set_status_text(label.into());
            // Joins the worker, which has already sent its last event.
            drop(st.run.borrow_mut().take());
            if st.close_requested.get() {
                let _ = ui.hide();
                let _ = slint::quit_event_loop();
            }
        }
    }
}
