//! Helpers shared by the headless window tests (spec seam 4): scratch app
//! directories, a recording stand-in for the native dialogs, and finding
//! controls by accessible label.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use cao_gui::{App, Desktop, MessageLevel};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::ComponentHandle;

/// Serializes tests that may start a run: one run may be active per process,
/// and the test harness runs a file's tests on parallel threads.
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A fresh, empty directory under the target directory for one test.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cao-gui")
        .join(name);
    // A missing directory is the expected case; anything else surfaces below.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An app directory holding a copy of the repository's shipped `profiles/`.
pub fn app_dir(name: &str) -> PathBuf {
    let app = scratch_dir(name);
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    copy_tree(&shipped, &app.join("profiles"));
    app
}

/// Replaces the value of one existing `key=value` line of `profiles/<file>`,
/// such as `common.ini` or `SSE/profile.ini`.
pub fn edit_ini(app: &Path, file: &str, key: &str, value: &str) {
    let path = app.join("profiles").join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    let prefix = format!("{key}=");
    assert!(
        text.lines().any(|line| line.starts_with(&prefix)),
        "{file} has no {key}"
    );
    let edited: Vec<String> = text
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) {
                format!("{prefix}{value}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    std::fs::write(&path, edited.join("\r\n")).unwrap();
}

/// Copies the directory tree `from` into `to`, creating `to` and its
/// subdirectories; files already in `to` are overwritten.
///
/// # Panics
/// On any I/O failure.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// One native dialog the window asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Message {
        level: MessageLevel,
        title: String,
        text: String,
    },
    FolderPicker,
    Opened(String),
}

/// Stands in for the native dialogs: records each request and answers at
/// once, as if the user pressed OK. `folder` is what the folder picker returns.
#[derive(Default)]
pub struct FakeDesktop {
    pub shown: RefCell<Vec<Shown>>,
    pub folder: RefCell<Option<PathBuf>>,
}

impl FakeDesktop {
    /// The titles of the messages shown so far, in order.
    pub fn message_titles(&self) -> Vec<String> {
        self.shown
            .borrow()
            .iter()
            .filter_map(|shown| match shown {
                Shown::Message { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect()
    }
}

impl Desktop for FakeDesktop {
    fn message(
        &self,
        _owner: Option<&slint::Window>,
        level: MessageLevel,
        title: &str,
        text: &str,
        then: Box<dyn FnOnce()>,
    ) {
        self.shown.borrow_mut().push(Shown::Message {
            level,
            title: title.to_owned(),
            text: text.to_owned(),
        });
        then();
    }

    fn pick_folder(
        &self,
        _owner: &slint::Window,
        _start: Option<&Path>,
        then: Box<dyn FnOnce(Option<PathBuf>)>,
    ) {
        self.shown.borrow_mut().push(Shown::FolderPicker);
        then(self.folder.borrow().clone());
    }

    fn open(&self, target: &str) {
        self.shown
            .borrow_mut()
            .push(Shown::Opened(target.to_owned()));
    }
}

/// Starts the headless backend on this test thread. Each `#[test]` runs on its
/// own thread, so every test calls this once before creating a window.
pub fn headless() {
    i_slint_backend_testing::init_no_event_loop();
}

/// Opens the window over `app` with a fresh fake desktop, and shows it.
pub fn open(app: &Path) -> (Rc<App>, Rc<FakeDesktop>) {
    let desktop = Rc::new(FakeDesktop::default());
    let window = App::new(app, desktop.clone()).unwrap();
    window.window().show().unwrap();
    (window, desktop)
}

/// Runs the event loop until `done` holds, delivering whatever the Run Worker
/// has posted to it, or fails after a minute.
///
/// Needs the event-loop backend (`i_slint_backend_testing::
/// init_integration_test_with_mock_time`), which a process may start only
/// once, so a test using this is alone in its file.
pub fn run_event_loop_until(mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "the event loop never reached the expected state"
        );
        // Queued behind what is already posted, so one pass delivers that.
        slint::invoke_from_event_loop(|| {
            // Only fails without an event loop, which this one is running.
            let _ = slint::quit_event_loop();
        })
        .unwrap();
        slint::run_event_loop().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The run-detail rows the Log tab shows below its log rows (#510).
pub fn detail_rows(ui: &cao_gui::ui::MainWindow) -> Vec<String> {
    use cao_gui::ui::{LogSeverity, LogTab};
    use slint::Model;
    ui.global::<LogTab>()
        .get_rows()
        .iter()
        .filter(|row| row.severity == LogSeverity::Detail)
        .map(|row| row.text.into())
        .collect()
}

/// Lets one turn of the event loop pass: zero-delay timers, such as a modal
/// dialog's deferred teardown, run now.
pub fn next_turn() {
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
}

/// Whether an element is a control rather than static text: a label beside a
/// control (or a control's own caption) reads as `Text`, the control as itself.
fn is_control(element: &ElementHandle) -> bool {
    !matches!(
        element.accessible_role(),
        None | Some(AccessibleRole::None) | Some(AccessibleRole::Text)
    )
}

/// The one control labelled `label` in `component`'s window.
///
/// # Panics
/// When no control, or more than one, has that label.
pub fn control(component: &impl ComponentHandle, label: &str) -> ElementHandle {
    let found: Vec<ElementHandle> = ElementHandle::find_by_accessible_label(component, label)
        .filter(is_control)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected one control labelled {label:?}, found {}",
        found.len()
    );
    found.into_iter().next().unwrap()
}

/// Whether some control is labelled `label`.
pub fn has_control(component: &impl ComponentHandle, label: &str) -> bool {
    ElementHandle::find_by_accessible_label(component, label).any(|element| is_control(&element))
}

/// The one menu-bar or menu entry showing `text`.
///
/// Slint 1.18's drawn menu bar gives its entries no accessible role of their
/// own: each is a `Text` labelled with its title, inside a clickable area.
/// Static text elsewhere with the same words would also match, so menu titles
/// must be unique among the window's texts.
///
/// # Panics
/// When no entry, or more than one, shows `text`.
pub fn menu_entry(component: &impl ComponentHandle, text: &str) -> ElementHandle {
    let found: Vec<ElementHandle> = ElementHandle::find_by_accessible_label(component, text)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Text))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected one menu entry {text:?}, found {}",
        found.len()
    );
    found.into_iter().next().unwrap()
}

/// Clicks the menu-bar or menu entry showing `text`, as a user would.
pub fn click_menu(component: &impl ComponentHandle, text: &str) {
    menu_entry(component, text).mock_single_click(slint::platform::PointerEventButton::Left);
    next_turn();
}

/// Selects a tab by its title, as a click on it would.
pub fn select_tab(component: &impl ComponentHandle, title: &str) {
    click(component, title);
}

/// Activates the control labelled `label` (clicks a button, toggles a box),
/// then lets the turn end, so `changed` handlers (a RadioGroup unchecking its
/// other radios, say) have run, as they would have before the user's next input.
pub fn click(component: &impl ComponentHandle, label: &str) {
    control(component, label).invoke_accessible_default_action();
    next_turn();
}

/// Enters `value` into the control labelled `label`, as typing it would.
pub fn enter(component: &impl ComponentHandle, label: &str, value: &str) {
    control(component, label).set_accessible_value(value);
    next_turn();
}

/// Whether the control labelled `label` is checked.
pub fn checked(component: &impl ComponentHandle, label: &str) -> bool {
    control(component, label)
        .accessible_checked()
        .unwrap_or(false)
}

/// Whether the control labelled `label` is enabled.
pub fn enabled(component: &impl ComponentHandle, label: &str) -> bool {
    control(component, label)
        .accessible_enabled()
        .unwrap_or(true)
}

/// Picks `item` in the combo box labelled `combo`, as a user would: opens its
/// popup and clicks the item, which raises the combo's `selected`.
///
/// # Panics
/// When the popup shows no item, or more than one, labelled `item`.
pub fn choose(component: &impl ComponentHandle, combo: &str, item: &str) {
    control(component, combo).invoke_accessible_expand_action();
    next_turn();
    let found: Vec<ElementHandle> = ElementHandle::find_by_accessible_label(component, item)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::ListItem))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected one {combo:?} item {item:?}, found {}",
        found.len()
    );
    found[0].mock_single_click(slint::platform::PointerEventButton::Left);
    next_turn();
}
