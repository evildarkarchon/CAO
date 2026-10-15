//! Direct manipulation of the main window (#512): reordering the tabs by
//! dragging them, and dropping a folder from Explorer onto the window.

use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, Model};

use super::App;

impl App {
    /// Connects the tab bar's drag reorder and the window's folder drop.
    /// Called once from `App::wire`.
    pub(super) fn wire_tabs_and_drop(&self) {
        let ui = &self.window;
        ui.on_move_tab({
            let app = self.weak();
            move |from, to| {
                if let Some(app) = app.upgrade() {
                    app.move_tab(from, to);
                }
            }
        });

        // Slint's `DropArea` only sees drags inside the app (slint#1967), so
        // Explorer drops come from winit. A window has a single winit event
        // filter and this installs it: anything else that needs raw winit
        // events must be routed through `App::window_event`. A no-op under the
        // headless backend, which has no winit window.
        ui.window().on_winit_window_event({
            let app = self.weak();
            move |_, event| match app.upgrade() {
                Some(app) => app.window_event(event),
                None => EventResult::Propagate,
            }
        });
    }

    /// Moves the tab at row `from` so it ends up at row `to`, as the tab bar
    /// asks when a dragged tab is released. Rows out of range are ignored. The
    /// order is not saved; C++ did not keep it either.
    fn move_tab(&self, from: i32, to: i32) {
        let count = self.tabs.row_count();
        let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
            return;
        };
        if from >= count || to >= count || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
    }

    /// Handles a raw winit event for the main window: the folder drop (C++
    /// `dragEnterEvent`/`dropEvent`). The real window's winit filter calls
    /// this; tests call it directly with the events Explorer would cause.
    ///
    /// - `HoveredFile` shows the drop hint over the window.
    /// - `HoveredFileCancelled` hides it.
    /// - `DroppedFile` hides it and, for the first path of a drop only, sets
    ///   the mod folder to that path when it exists. Like C++, which checked
    ///   `QDir::exists`, a file is accepted as well as a folder.
    ///
    /// While a modal dialog is open the drop is ignored, as Qt ignores drops on
    /// a window a modal dialog blocks.
    ///
    /// Always returns [`EventResult::Propagate`], so Slint still sees every
    /// event.
    pub fn window_event(&self, event: &WindowEvent) -> EventResult {
        let blocked = self.window.get_modal_blocked();
        match event {
            WindowEvent::HoveredFile(_) if !blocked => {
                // winit sends one `HoveredFile` per dragged path before the
                // drop; each starts the drop afresh, which is harmless.
                self.drop_handled.set(false);
                self.window.set_drop_hover(true);
            }
            WindowEvent::HoveredFileCancelled => self.window.set_drop_hover(false),
            WindowEvent::DroppedFile(path) => {
                // Hiding the hint is always safe, even while blocked.
                self.window.set_drop_hover(false);
                // winit sends one `DroppedFile` per path; C++ used only the
                // first URL of a drop, so the rest are ignored.
                if !blocked && !self.drop_handled.replace(true) && path.exists() {
                    self.window.set_user_path(path.display().to_string().into());
                }
            }
            _ => {}
        }
        EventResult::Propagate
    }
}
