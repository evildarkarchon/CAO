//! The main window's behaviour: startup, profile selection and creation, saving
//! and loading settings, and the menu (C++ `MainWindow`).

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};

use cao_gui_ui::{AboutSlintDialog, ChoiceDialog, MainWindow, TabInfo, TextInputDialog};
use cao_profiles::{CommonSettings, Options, Profile, ProfileError, ProfileSettings, Profiles};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::desktop::{Desktop, MessageLevel};
use crate::form::{self, EnabledTabs};
use crate::modal::ModalSlot;

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
    /// not saved. #510 sets it.
    locked: std::cell::Cell<bool>,
    profile_names: Rc<VecModel<SharedString>>,
    tabs: Rc<VecModel<TabInfo>>,
    name_dialog: ModalSlot<TextInputDialog>,
    base_dialog: ModalSlot<ChoiceDialog>,
    about_slint_dialog: ModalSlot<AboutSlintDialog>,
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
        let tab = |id: &str, title: &str| TabInfo {
            id: id.into(),
            title: title.into(),
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
            profile_names: Rc::new(VecModel::from(
                names.iter().map(SharedString::from).collect::<Vec<_>>(),
            )),
            tabs: Rc::new(VecModel::from(vec![
                tab("bsa", "BSA"),
                tab("meshes", "Meshes"),
                tab("textures", "Textures"),
                tab("animations", "Animations"),
                tab("log", "Log"),
            ])),
            name_dialog: ModalSlot::default(),
            base_dialog: ModalSlot::default(),
            about_slint_dialog: ModalSlot::default(),
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

    /// Connects the window's callbacks. Each holds the app weakly: the window
    /// owns its callbacks, and the app owns the window.
    fn wire(&self) {
        let ui = &self.window;
        let app = &self.me;
        let with = |action: fn(&Self)| {
            let app = app.clone();
            move || {
                if let Some(app) = app.upgrade() {
                    action(&app);
                }
            }
        };

        ui.on_profile_selected({
            let app = app.clone();
            move |index| {
                if let Some(app) = app.upgrade() {
                    app.profile_selected(index);
                }
            }
        });
        ui.on_new_profile(with(Self::new_profile));
        ui.on_open_directory(with(Self::open_directory));
        ui.on_advanced_clicked({
            let app = app.clone();
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.tutorial(
                        "Advanced settings",
                        "Advanced settings can only be modified when using custom profiles.",
                        Box::new(|| {}),
                    );
                }
            }
        });
        // C++ saves the window when debug logging is toggled.
        ui.on_debug_log_toggled({
            let app = app.clone();
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.save();
                }
            }
        });
        ui.on_open_docs(with(|app| app.desktop.open(DOCUMENTATION_URL)));
        ui.on_open_discord(with(|app| app.desktop.open(DISCORD_URL)));
        ui.on_about(with(Self::about));
        ui.on_about_slint(with(Self::about_slint));
        ui.window().on_close_requested({
            let app = app.clone();
            move || {
                if let Some(app) = app.upgrade() {
                    app.save();
                }
                // Fails only without an event loop (headless), where hiding the
                // window is all closing means.
                let _ = slint::quit_event_loop();
                CloseRequestResponse::HideWindow
            }
        });
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
    /// `name` into it.
    fn select_profile(&self, name: &str) {
        self.save();
        self.load_profile(name);
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
        // An unreadable profile enables no asset tab.
        self.enable_tabs(match &settings {
            Some(settings) => EnabledTabs {
                bsa: settings.bsa_enabled,
                meshes: settings.meshes_enabled,
                textures: settings.textures_enabled,
                animations: settings.animations_enabled,
            },
            None => EnabledTabs {
                bsa: false,
                meshes: false,
                textures: false,
                animations: false,
            },
        });
        ui.set_base_profile(profile.is_base());

        loaded.profile = profile;
        loaded.options = options;
        loaded.settings = settings;
    }

    /// Enables the asset tabs the profile allows, and moves the selection off a
    /// tab that becomes disabled, as QTabBar does.
    fn enable_tabs(&self, enabled: EnabledTabs) {
        for row in 0..self.tabs.row_count() {
            let Some(mut tab) = self.tabs.row_data(row) else {
                continue;
            };
            tab.enabled = match tab.id.as_str() {
                "bsa" => enabled.bsa,
                "meshes" => enabled.meshes,
                "textures" => enabled.textures,
                "animations" => enabled.animations,
                _ => true,
            };
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
    pub fn save(&self) {
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
        if self.locked.get() {
            return;
        }

        let options = form::read_options(ui, &loaded.options);
        if let Err(error) = loaded.profile.save_options(&options) {
            self.error(
                "Save",
                &format!("The profile's options could not be saved: {error}"),
            );
        }
        loaded.options = options;
        if let Some(current) = &loaded.settings {
            let settings = form::read_settings(ui, current);
            if let Err(error) = loaded.profile.save_settings(&settings) {
                self.error(
                    "Save",
                    &format!("The profile's settings could not be saved: {error}"),
                );
            }
            loaded.settings = Some(settings);
        }
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
