//! The main window's behaviour: startup, profile selection and creation, saving
//! and loading settings, and the menu (C++ `MainWindow`). Optimization Runs are
//! in [`run`].

mod run;

use std::cell::{OnceCell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};

use cao_core::routing::ProfileCapabilities;
use cao_gui_ui::{AboutSlintDialog, ChoiceDialog, MainWindow, TabInfo, TextInputDialog};
use cao_gui_ui::{FormatItem, FormatsDialog, TexturesTab};
use cao_optimizers::application_log::ApplicationLog;
use cao_optimizers::profile::profile_capabilities;
use cao_profiles::{CommonSettings, Options, Profile, ProfileError, ProfileSettings, Profiles};
use slint::{
    CloseRequestResponse, ComponentHandle, FilterModel, Model, ModelRc, SharedString, VecModel,
};

use crate::desktop::{Desktop, MessageLevel};
use crate::form::{self, Tab};
use crate::formats;
use crate::log_tab::AttachedLog;
use crate::modal::ModalSlot;
use crate::rules;

mod input;

const DOCUMENTATION_URL: &str = "https://www.nexusmods.com/skyrimspecialedition/mods/23316";
const DISCORD_URL: &str = "https://discordapp.com/invite/B9abN8d";

/// Why the window could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// `profiles/` holds no profile, so there is nothing to select. Usually the
    /// exe was moved away from its `profiles/` directory.
    #[error("no profiles were found in `{}`", path.display())]
    NoProfiles { path: std::path::PathBuf },
    /// `profiles/common.ini` exists but cannot be read.
    #[error(transparent)]
    Profile(#[from] ProfileError),
    /// Slint could not create the window.
    #[error("the window could not be created: {0}")]
    Window(#[from] slint::PlatformError),
}

/// What the window last loaded, which saving writes back to.
struct Loaded {
    /// The selected profile.
    profile: Profile,
    /// `common.ini`, as last loaded or saved.
    common: CommonSettings,
    /// The selected profile's options, over which the controls are read back.
    options: Options,
    /// The selected profile's settings; `None` when its `profile.ini` could not
    /// be read, in which case it is never written either.
    settings: Option<ProfileSettings>,
}

/// The main window and its state. Create it with [`App::new`]; every callback
/// holds it weakly, so dropping the last `Rc` drops the window too.
pub struct App {
    /// This app's own `Rc`, weakly, for the callbacks it hands out.
    me: Weak<Self>,
    window: MainWindow,
    profiles: Profiles,
    desktop: Rc<dyn Desktop>,
    loaded: RefCell<Loaded>,
    /// While a run holds the controls (C++ `_bLockVariables`), the options are
    /// not saved. Set from a run's start to its end (#510).
    locked: std::cell::Cell<bool>,
    /// Optimization Runs started from the window (#510).
    runs: run::Runs,
    profile_names: Rc<VecModel<SharedString>>,
    tabs: Rc<VecModel<TabInfo>>,
    name_dialog: ModalSlot<TextInputDialog>,
    base_dialog: ModalSlot<ChoiceDialog>,
    about_slint_dialog: ModalSlot<AboutSlintDialog>,
    formats_dialog: ModalSlot<FormatsDialog>,
    /// The Application Log, once `main` has attached it (#511).
    log: OnceCell<AttachedLog>,
    /// Whether the current folder drop's first path has been handled (#512).
    drop_handled: std::cell::Cell<bool>,
}

impl App {
    /// Creates the main window over the CAO install in `app_dir`, the directory
    /// holding the exe (deviation 1: only `main` derives it, from the exe; the
    /// working directory is never consulted). The window is not shown yet.
    ///
    /// As C++ did at startup, this loads `common.ini`, selects its profile (or
    /// SSE when that profile is gone), shows the profile's settings and, on the
    /// very first start, a welcome message.
    ///
    /// # Errors
    /// [`StartupError`] when `profiles/` has no profile, `common.ini` cannot be
    /// read, or the window cannot be created.
    pub fn new(app_dir: &Path, desktop: Rc<dyn Desktop>) -> Result<Rc<Self>, StartupError> {
        let profiles = Profiles::new(app_dir);
        let names = profiles.list();
        if names.is_empty() {
            return Err(StartupError::NoProfiles {
                path: profiles.root().to_owned(),
            });
        }
        let common = profiles.load_common()?;
        let current = profiles.resolve(&common.profile);

        let window = MainWindow::new()?;
        let tab = |tab: Tab| TabInfo {
            id: tab.id().into(),
            title: tab.title().into(),
            enabled: true,
        };
        let app = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            loaded: RefCell::new(Loaded {
                profile: profiles.open(&current),
                common: common.clone(),
                options: Options::default(),
                settings: None,
            }),
            profiles,
            desktop,
            locked: Default::default(),
            runs: run::Runs::new(app_dir),
            profile_names: Rc::new(VecModel::from(
                names.iter().map(SharedString::from).collect::<Vec<_>>(),
            )),
            tabs: Rc::new(VecModel::from(
                Tab::ALL.into_iter().map(tab).collect::<Vec<_>>(),
            )),
            name_dialog: ModalSlot::default(),
            base_dialog: ModalSlot::default(),
            about_slint_dialog: ModalSlot::default(),
            formats_dialog: ModalSlot::default(),
            log: OnceCell::new(),
            drop_handled: Default::default(),
            window,
        });

        let ui = &app.window;
        ui.set_profiles(ModelRc::from(app.profile_names.clone()));
        ui.set_tabs(ModelRc::from(app.tabs.clone()));
        // C++ `loadUi`: the common settings first.
        ui.set_dark(common.dark_mode);
        ui.set_show_advanced(common.show_advanced_settings);
        ui.set_show_tutorials(common.show_tutorial);
        app.wire();
        app.load_profile(&current);
        app.first_start();
        Ok(app)
    }

    /// Shows the window and runs the event loop until it closes.
    ///
    /// # Errors
    /// The `PlatformError` from showing the window or running the loop.
    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }

    /// The main window component.
    pub fn window(&self) -> &MainWindow {
        &self.window
    }

    /// The open "New profile" name dialog, if any.
    pub fn name_dialog(&self) -> Option<TextInputDialog> {
        self.name_dialog.dialog()
    }

    /// The open "Base profile" dialog, if any.
    pub fn base_dialog(&self) -> Option<ChoiceDialog> {
        self.base_dialog.dialog()
    }

    /// The open About Slint dialog, if any.
    pub fn about_slint_dialog(&self) -> Option<AboutSlintDialog> {
        self.about_slint_dialog.dialog()
    }

    /// The open unwanted-formats dialog, if any.
    pub fn formats_dialog(&self) -> Option<FormatsDialog> {
        self.formats_dialog.dialog()
    }

    /// Connects the window's callbacks. Each holds the app weakly: the window
    /// owns its callbacks, and the app owns the window.
    fn wire(&self) {
        let ui = &self.window;
        let app = &self.me;
        let with = |action: fn(&Self)| self.with(action);
        ui.on_profile_selected(self.with_arg(Self::profile_selected));
        ui.on_new_profile(with(Self::new_profile));
        ui.on_open_directory(with(Self::open_directory));
        ui.on_advanced_clicked(self.with_arg(|app, _: bool| {
            app.tutorial(
                "Advanced settings",
                "Advanced settings can only be modified when using custom profiles.",
                Box::new(|| {}),
            );
        }));
        // C++ saves the window when debug logging is toggled.
        ui.on_debug_log_toggled(self.with_arg(|app, debug| {
            app.save();
            app.debug_log_toggled(debug);
        }));
        ui.on_open_log(with(Self::open_log));
        ui.on_open_docs(with(|app| app.desktop.open(DOCUMENTATION_URL)));
        ui.on_open_discord(with(|app| app.desktop.open(DISCORD_URL)));
        ui.on_about(with(Self::about));
        ui.on_about_slint(with(Self::about_slint));
        ui.on_run_clicked(with(Self::run_clicked));
        ui.on_edit_unwanted(with(Self::edit_unwanted));
        self.wire_rules();
        ui.window().on_close_requested({
            let app = app.clone();
            move || {
                if let Some(app) = app.upgrade() {
                    // During a run the window stays open: the run is cancelled,
                    // and the window closes once it reports its outcome (#510).
                    if !app.may_close() {
                        return CloseRequestResponse::KeepWindowShown;
                    }
                    app.save();
                }
                // Fails only without an event loop (headless), where hiding the
                // window is all closing means.
                let _ = slint::quit_event_loop();
                CloseRequestResponse::HideWindow
            }
        });
        self.wire_tabs_and_drop();
    }

    // ---- Profiles

    /// The profile combo's `activated` (C++ `setGameMode(currentText)`).
    fn profile_selected(&self, index: i32) {
        let name = usize::try_from(index)
            .ok()
            .and_then(|index| self.profile_names.row_data(index));
        if let Some(name) = name {
            self.select_profile(&name);
        }
    }

    /// C++ `setGameMode`: saves the window into the current profile, then loads
    /// `name` into it, and stamps a new Application Log for it.
    fn select_profile(&self, name: &str) {
        self.save();
        self.load_profile(name);
        self.stamp_log(name);
    }

    /// Loads profile `name` into the window: remembers it in `common.ini`, shows
    /// its options and settings, enables the tabs it allows, and makes the
    /// advanced groups read-only for a base profile.
    fn load_profile(&self, name: &str) {
        let ui = &self.window;
        let profile = self.profiles.open(name);
        let mut loaded = self.loaded.borrow_mut();

        // C++ `Profiles::setCurrentProfile` writes the choice at once.
        loaded.common.profile = name.to_owned();
        if let Err(error) = self.profiles.save_common(&loaded.common) {
            self.error(
                "Profile",
                &format!("The selected profile could not be remembered: {error}"),
            );
        }

        let options = match profile.load_options(&loaded.options) {
            Ok(options) => options,
            Err(error) => {
                self.error(
                    "Profile",
                    &format!("The profile's options could not be read: {error}"),
                );
                loaded.options.clone()
            }
        };
        let settings = match profile.load_settings() {
            Ok(settings) => Some(settings),
            Err(error) => {
                self.error(
                    "Profile",
                    &format!("The profile `{name}` could not be read: {error}"),
                );
                None
            }
        };

        if let Some(index) = self
            .profile_names
            .iter()
            .position(|candidate| candidate == name)
        {
            ui.set_profile_index(index as i32);
        }
        if let Some(settings) = &settings {
            form::show_settings(ui, settings);
        }
        form::show_options(ui, &options);
        // The UI rules also hold for what was loaded (deviations 3 and 21).
        rules::apply(ui);
        // An unreadable profile enables no asset tab.
        self.enable_tabs(settings.as_ref().map(profile_capabilities).as_ref());
        ui.set_base_profile(profile.is_base());

        loaded.profile = profile;
        loaded.options = options;
        loaded.settings = settings;
    }

    /// Enables the asset tabs the profile's Profile Capabilities allow (none
    /// when `capabilities` is `None`, for an unreadable `profile.ini`), and
    /// moves the selection off a tab that becomes disabled, as QTabBar does.
    fn enable_tabs(&self, capabilities: Option<&ProfileCapabilities>) {
        for row in 0..self.tabs.row_count() {
            let Some(mut tab) = self.tabs.row_data(row) else {
                continue;
            };
            tab.enabled = Tab::from_id(&tab.id).is_none_or(|id| id.enabled_by(capabilities));
            self.tabs.set_row_data(row, tab);
        }
        let current = self.window.get_current_tab();
        if !self.tabs.iter().any(|tab| tab.id == current && tab.enabled)
            && let Some(tab) = self.tabs.iter().find(|tab| tab.enabled)
        {
            self.window.set_current_tab(tab.id);
        }
    }

    /// C++ `saveUi`: saves the common settings, then, unless a run holds the
    /// controls, the options into `settings.ini` and the settings into
    /// `profile.ini` of the current profile. A failure is shown, not raised.
    ///
    /// Returns `false` when the window's settings could not be written to
    /// `profile.ini`, which then differs from what the window shows. The other
    /// failures are only shown: a run reads its settings from `profile.ini` but
    /// takes its options from memory (#545).
    pub fn save(&self) -> bool {
        let ui = &self.window;
        let mut loaded = self.loaded.borrow_mut();
        loaded.common.show_advanced_settings = ui.get_show_advanced();
        loaded.common.dark_mode = ui.get_dark();
        loaded.common.show_tutorial = ui.get_show_tutorials();
        if let Err(error) = self.profiles.save_common(&loaded.common) {
            self.error(
                "Save",
                &format!("The common settings could not be saved: {error}"),
            );
        }
        // A run holds the controls, so the window still shows the settings
        // saved when it started.
        if self.locked.get() {
            return true;
        }

        // The capabilities come from the loaded `profile.ini`, not the tab bar:
        // an unreadable one disables every tab, yet says nothing of what the
        // profile supports, so its asset options are kept as loaded (#580).
        let capabilities = loaded.settings.as_ref().map(profile_capabilities);
        let options = form::read_options(ui, &loaded.options, capabilities.as_ref());
        if let Err(error) = loaded.profile.save_options(&options) {
            self.error(
                "Save",
                &format!("The profile's options could not be saved: {error}"),
            );
        }
        loaded.options = options;
        let mut settings_saved = true;
        if let Some(current) = &loaded.settings {
            let settings = form::read_settings(ui, current);
            if let Err(error) = loaded.profile.save_settings(&settings) {
                self.error(
                    "Save",
                    &format!("The profile's settings could not be saved: {error}"),
                );
                settings_saved = false;
            }
            loaded.settings = Some(settings);
        }
        settings_saved
    }

    /// C++ `createProfile`: a tutorial box, then the name, then the base profile;
    /// creates the profile and selects it.
    fn new_profile(&self) {
        let app = self.weak();
        self.tutorial(
            "New profile",
            "You are about to create a new profile. It will create a new directory in 'CAO/profiles'. Please check it out after creation, some files will be created inside it.",
            Box::new(move || {
                if let Some(app) = app.upgrade() {
                    app.ask_profile_name();
                }
            }),
        );
    }

    /// Opens the "New profile" name dialog.
    fn ask_profile_name(&self) {
        let app = self.weak();
        let opened = self.name_dialog.open(&self.window, || {
            let dialog = TextInputDialog::new()?;
            dialog.set_dialog_title("New profile".into());
            dialog.set_label("Name:".into());
            let confirm = (app.clone(), dialog.as_weak());
            dialog.on_confirm(move || {
                let (Some(app), Some(dialog)) = (confirm.0.upgrade(), confirm.1.upgrade()) else {
                    return;
                };
                let name = dialog.get_value().to_string();
                app.name_dialog.close();
                // C++ returns on an empty name, as on Cancel.
                if !name.is_empty() {
                    app.ask_base_profile(name);
                }
            });
            let dismiss = app.clone();
            dialog.on_dismiss(move || {
                if let Some(app) = dismiss.upgrade() {
                    app.name_dialog.close();
                }
            });
            Ok(dialog)
        });
        self.report_dialog_error(opened);
    }

    /// Opens the "Base profile" dialog for the new profile `name`.
    fn ask_base_profile(&self, name: String) {
        let app = self.weak();
        let opened = self.base_dialog.open(&self.window, || {
            let dialog = ChoiceDialog::new()?;
            dialog.set_dialog_title("Base profile".into());
            dialog.set_label("Which profile do you want to use as a base?".into());
            dialog.set_choices(ModelRc::from(self.profile_names.clone()));
            dialog.set_current_index(self.window.get_profile_index());
            let confirm = (app.clone(), dialog.as_weak());
            dialog.on_confirm(move || {
                let (Some(app), Some(dialog)) = (confirm.0.upgrade(), confirm.1.upgrade()) else {
                    return;
                };
                let base = usize::try_from(dialog.get_current_index())
                    .ok()
                    .and_then(|index| app.profile_names.row_data(index))
                    .unwrap_or_default();
                app.base_dialog.close();
                app.create_profile(&name, &base);
            });
            let dismiss = app.clone();
            dialog.on_dismiss(move || {
                if let Some(app) = dismiss.upgrade() {
                    app.base_dialog.close();
                }
            });
            Ok(dialog)
        });
        self.report_dialog_error(opened);
    }

    /// Creates profile `name` from `base`, refreshes the profile list and
    /// selects the new profile.
    fn create_profile(&self, name: &str, base: &str) {
        if let Err(error) = self.profiles.create(name, base) {
            self.error(
                "New profile",
                &format!("The profile could not be created: {error}"),
            );
            return;
        }
        let names: Vec<SharedString> = self
            .profiles
            .list()
            .iter()
            .map(SharedString::from)
            .collect();
        self.profile_names.set_vec(names);
        self.select_profile(name);
    }

    // ---- Top row and menu

    /// "Open Directory": asks for the mod folder.
    fn open_directory(&self) {
        let current = self.window.get_user_path().to_string();
        let window = self.window.as_weak();
        self.desktop.pick_folder(
            self.window.window(),
            Some(Path::new(&current)),
            Box::new(move |folder| {
                if let (Some(folder), Some(window)) = (folder, window.upgrade()) {
                    window.set_user_path(folder.display().to_string().into());
                }
            }),
        );
    }

    /// Help → About.
    fn about(&self) {
        let text = format!(
            "Cathedral Assets Optimizer {}\nMade by G'k\nThis program is distributed in the hope that it will be useful but WITHOUT ANY WARRANTLY. See the Mozilla Public License",
            env!("CARGO_PKG_VERSION")
        );
        self.desktop.message(
            Some(self.window.window()),
            MessageLevel::Info,
            "About",
            &text,
            Box::new(|| {}),
        );
    }

    /// Help → About Slint, where C++ had About Qt.
    fn about_slint(&self) {
        let app = self.weak();
        let opened = self.about_slint_dialog.open(&self.window, || {
            let dialog = AboutSlintDialog::new()?;
            dialog.on_confirm(move || {
                if let Some(app) = app.upgrade() {
                    app.about_slint_dialog.close();
                }
            });
            Ok(dialog)
        });
        self.report_dialog_error(opened);
    }

    /// C++ `firstStart`: a standalone welcome box the first time CAO runs.
    fn first_start(&self) {
        let mut loaded = self.loaded.borrow_mut();
        if loaded.common.not_first_start {
            return;
        }
        loaded.common.not_first_start = true;
        if let Err(error) = self.profiles.save_common(&loaded.common) {
            self.error(
                "Save",
                &format!("The common settings could not be saved: {error}"),
            );
        }
        drop(loaded);
        self.desktop.message(
            None,
            MessageLevel::Info,
            &format!("Welcome to Cathedral Assets Optimizer {}", env!("CARGO_PKG_VERSION")),
            "It appears you are running CAO for the first time. All options have tooltips explaining what they do. If you need help, you can also join us on Discord. A dark theme is also available.",
            Box::new(|| {}),
        );
    }

    // ---- Helpers

    /// A weak handle to this app, for callbacks.
    fn weak(&self) -> Weak<Self> {
        self.me.clone()
    }

    /// A no-argument window callback that runs `action` on the app, holding it
    /// weakly (the window owns its callbacks, and the app owns the window). It
    /// does nothing once the app is gone.
    fn with(&self, action: impl Fn(&Self) + 'static) -> impl Fn() + 'static {
        let app = self.weak();
        move || {
            if let Some(app) = app.upgrade() {
                action(&app);
            }
        }
    }

    /// [`App::with`] for a callback that passes one argument on to `action`.
    fn with_arg<A>(&self, action: impl Fn(&Self, A) + 'static) -> impl Fn(A) + 'static {
        let app = self.weak();
        move |argument| {
            if let Some(app) = app.upgrade() {
                action(&app, argument);
            }
        }
    }

    /// C++ `showTutorialWindow`: an information box when Tools → Show tutorials
    /// is on. `then` runs after it closes, or at once when it is off.
    fn tutorial(&self, title: &str, text: &str, then: Box<dyn FnOnce()>) {
        if self.window.get_show_tutorials() {
            self.desktop.message(
                Some(self.window.window()),
                MessageLevel::Info,
                title,
                text,
                then,
            );
        } else {
            then();
        }
    }

    /// A critical box over the window.
    fn error(&self, title: &str, text: &str) {
        self.desktop.message(
            Some(self.window.window()),
            MessageLevel::Error,
            title,
            text,
            Box::new(|| {}),
        );
    }

    /// Shows a failure to create or show a dialog.
    fn report_dialog_error(&self, opened: Result<(), slint::PlatformError>) {
        if let Err(error) = opened {
            self.error("Error", &format!("The dialog could not be opened: {error}"));
        }
    }
}

// ---- UI rules (#509)

impl App {
    /// Connects the controls that trigger a UI rule to [`rules::apply`], the
    /// same call `load_profile` makes after loading settings.
    fn wire_rules(&self) {
        let ui = &self.window;
        let apply_rules = |app: &Self| rules::apply(&app.window);
        ui.on_dry_run_clicked(self.with_arg(move |app, _| apply_rules(app)));
        // A RadioGroup disabled with its unchecked group reverts what a rule
        // writes to its radios, so the rules re-apply once the group is back.
        ui.on_meshes_process_toggled(self.with(apply_rules));
        ui.on_game_selected(self.with_arg(move |app, _| apply_rules(app)));
        ui.on_mode_activated(self.with_arg(|app, _| app.mode_activated()));
    }

    /// The Textures tab's Edit button (C++ `TexturesFormatSelectDialog`): opens
    /// the unwanted-formats dialog over a scratch copy of every format, checked
    /// where the tab lists it.
    ///
    /// The dialog shows the scratch rows through a filter model; a checkbox's
    /// two-way binding writes through the filter into its scratch row, so the
    /// search hides rows without losing their state. OK replaces the tab's list
    /// with the checked rows, in C++'s order; Cancel and the title bar drop the
    /// scratch copy, where C++ committed on any close (deviation 2).
    fn edit_unwanted(&self) {
        let listed: Vec<SharedString> = self
            .window
            .global::<TexturesTab>()
            .get_unwanted()
            .iter()
            .collect();
        let scratch = Rc::new(VecModel::from(
            formats::UNWANTED_FORMAT_CHOICES
                .iter()
                .map(|&(name, _)| FormatItem {
                    name: name.into(),
                    checked: listed.iter().any(|listed| listed == name),
                })
                .collect::<Vec<_>>(),
        ));
        // Lowercased, as C++'s `Qt::MatchContains` search ignores case.
        let search = Rc::new(RefCell::new(String::new()));
        let filtered = Rc::new(FilterModel::new(scratch.clone(), {
            let search = search.clone();
            move |item: &FormatItem| item.name.to_lowercase().contains(search.borrow().as_str())
        }));

        let app = self.weak();
        let opened = self.formats_dialog.open(&self.window, || {
            let dialog = FormatsDialog::new()?;
            dialog.set_items(ModelRc::from(filtered.clone()));
            dialog.on_search_edited(move |text| {
                *search.borrow_mut() = text.to_lowercase();
                filtered.reset();
            });
            let confirm = app.clone();
            dialog.on_confirm(move || {
                let Some(app) = confirm.upgrade() else { return };
                let chosen: Vec<SharedString> = scratch
                    .iter()
                    .filter(|item| item.checked)
                    .map(|item| item.name)
                    .collect();
                app.window
                    .global::<TexturesTab>()
                    .set_unwanted(ModelRc::new(VecModel::from(chosen)));
                app.formats_dialog.close();
            });
            let dismiss = app.clone();
            dialog.on_dismiss(move || {
                if let Some(app) = dismiss.upgrade() {
                    app.formats_dialog.close();
                }
            });
            Ok(dialog)
        });
        self.report_dialog_error(opened);
    }

    /// The mode combo's `activated`: applies the Several Mods rule and, for
    /// Several Mods, shows C++'s tutorial box.
    fn mode_activated(&self) {
        rules::apply(&self.window);
        if rules::several_mods_chosen(&self.window) {
            self.tutorial(
                "Several mods option",
                "You have selected the several mods option. This process may take a very long time, especially if you process BSA. \nThis process has only been tested on the Mod Organizer mods folder.",
                Box::new(|| {}),
            );
        }
    }
}

// ---- Application Log (#511)
impl App {
    /// Shows the process's Application Log in the Log tab, from the records
    /// logged since [`bootstrap_log`](crate::bootstrap_log) on; from here on,
    /// selecting a profile stamps a new log file, toggling debug logging
    /// switches the level in the current one, and Tools → Open log file opens
    /// it. `main` calls this once, right after [`App::new`]; later calls are
    /// ignored.
    ///
    /// The rows arrive on the event loop's next turns, so they show once the
    /// window runs.
    pub fn attach_log(&self, log: &'static ApplicationLog) {
        if self.log.get().is_some() {
            return;
        }
        // `Profiles::new(app_dir)` keeps `<app_dir>/profiles`; the log goes
        // beside it, under `<app_dir>/logs`.
        let app_dir = self.profiles.root().parent().unwrap_or(Path::new(""));
        let attached = AttachedLog::attach(&self.window, log, app_dir);
        // Fails only if the cell is already set, which the early return above
        // rules out: the window lives on one thread, so nothing set it since.
        let _ = self.log.set(attached);
    }

    /// The Application Log [`App::attach_log`] attached, if any; a run start
    /// redirects it (#510).
    pub fn application_log(&self) -> Option<&'static ApplicationLog> {
        self.log.get().map(AttachedLog::sink)
    }

    /// Moves the log to a fresh `logs/<profile>/yy.MM.dd.hh.mm.html` for the
    /// newly selected `profile`, at its debug choice. The startup profile is
    /// not stamped here: `main` bootstrapped the log for it, and a second stamp
    /// a minute later would drop the startup records from the tab.
    fn stamp_log(&self, profile: &str) {
        let Some(log) = self.log.get() else {
            return;
        };
        let debug = self.loaded.borrow().options.debug_log;
        let path = log.stamp_path(profile);
        // The profile session's runs log to this file even when it cannot be
        // opened now: each run start retries it, and refuses to start without it.
        self.remember_run_log(path.clone());
        if let Err(error) = log.stamp(&path, debug) {
            self.error(
                "Profile",
                &format!("The profile's log file could not be opened: {error}"),
            );
        }
    }

    /// Tools → Enable debug log: the level and layout switch at once, in the
    /// same file, so logging carries on either way (deviation 23). While a run
    /// holds the controls the running log is left alone, as C++ left it; the
    /// next run start applies the choice.
    fn debug_log_toggled(&self, debug: bool) {
        if self.locked.get() {
            return;
        }
        if let Some(log) = self.log.get()
            && let Err(error) = log.set_debug(debug)
        {
            self.error(
                "Error",
                &format!("The log file could not be reopened: {error}"),
            );
        }
    }

    /// Tools → Open log file: the file the log is writing to now. Every record
    /// is written unbuffered, so it holds everything up to the click.
    fn open_log(&self) {
        if let Some(log) = self.log.get() {
            self.desktop.open(&log.path().display().to_string());
        }
    }
}
