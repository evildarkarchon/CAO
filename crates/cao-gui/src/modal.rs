//! Modal Slint dialogs over the main window (#462, #508).
//!
//! Slint has no modal windows (slint#6607), so a dialog is made modal by hand:
//!
//! - **Owner.** The dialog's winit window is created owned by the main window,
//!   so it stays above it, minimises with it and has no taskbar button. winit
//!   sets the owner at creation through the safe
//!   `WindowAttributesExtWindows::with_owner_window`, reached through the
//!   attributes hook `main` installs ([`owned_window_attributes`]). No `unsafe`
//!   `SetWindowLongPtrW` is needed.
//! - **Disabled owner.** A depth-counted [`ModalGuard`] disables the main window
//!   while any dialog is open.
//! - **Close order.** [`ModalSlot::close`] re-enables the owner *before* hiding the
//!   dialog: if the owner is still disabled when its owned window hides, Windows
//!   activates some other application instead of returning to CAO.
//! - **Deferred teardown.** The dialog is hidden and dropped on the next turn of
//!   the event loop, so it outlives its own click handler.
//! - **Title-bar close** goes through the same [`ModalSlot::close`].
//!
//! Never call winit's `focus_window()` here: on Windows it fakes an Alt keypress
//! to win the foreground lock, which puts the menu bar into keyboard mode so the
//! user's next click only dismisses it. Windows re-activates an enabled owner by
//! itself when its owned dialog hides.
//!
//! Everything winit-specific is a no-op under the headless test backend, which
//! has no winit window; the in-tree `modal-blocked` flag still applies there.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::platform::windows::{WindowAttributesExtWindows, WindowExtWindows};
use slint::winit_030::winit::window::{WindowAttributes, WindowButtons};
use slint::{CloseRequestResponse, ComponentHandle, PhysicalPosition, PlatformError};

use cao_gui_ui::MainWindow;

thread_local! {
    // Nesting depth of open dialogs. A counter, not a bool: the New Profile flow
    // closes one dialog and opens the next in the same turn, and a bool guard
    // dropped late would re-enable the main window under the second dialog.
    static DEPTH: Cell<u32> = const { Cell::new(0) };

    // The HWND the window being created right now should be owned by. Set only
    // while `ModalSlot::open` creates and shows a dialog, so the main window and
    // anything else are created unowned.
    static PENDING_OWNER: Cell<Option<isize>> = const { Cell::new(None) };
}

/// The winit window-attributes hook `main` installs with
/// `slint::BackendSelector::with_winit_window_attributes_hook`.
///
/// While [`ModalSlot::open`] creates a dialog, it makes the new window owned by
/// the main window and leaves it only a close button: a modal dialog has nothing
/// to minimise or maximise. Every other window is left as it is. Slint calls the
/// hook on the UI thread when it creates a window adapter.
pub fn owned_window_attributes(attributes: WindowAttributes) -> WindowAttributes {
    match PENDING_OWNER.get() {
        Some(owner) => attributes
            .with_owner_window(owner)
            .with_enabled_buttons(WindowButtons::CLOSE),
        None => attributes,
    }
}

/// The number of Slint dialogs open over the main window.
pub fn depth() -> u32 {
    DEPTH.get()
}

/// The Win32 window behind a Slint window; `None` before its winit window
/// exists, and always under the headless backend.
fn hwnd_of(window: &slint::Window) -> Option<isize> {
    match window.window_handle().window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Enables or disables the main window for input: the real window through
/// winit, and the content in-tree through `modal-blocked`.
fn set_owner_enabled(main: &MainWindow, enabled: bool) {
    main.set_modal_blocked(!enabled);
    // `None` without a winit window (headless): nothing more to disable.
    let _ = main
        .window()
        .with_winit_window(|window| window.set_enable(enabled));
}

/// Disables the main window while alive. Keep it beside its dialog and drop it
/// *before* hiding the dialog; see the module docs.
pub struct ModalGuard {
    main: slint::Weak<MainWindow>,
}

impl ModalGuard {
    /// Counts one more open dialog, disabling `main` if it is the first.
    fn new(main: &MainWindow) -> Self {
        let depth = DEPTH.get() + 1;
        DEPTH.set(depth);
        if depth == 1 {
            set_owner_enabled(main, false);
        }
        Self {
            main: main.as_weak(),
        }
    }
}

impl Drop for ModalGuard {
    /// Counts one dialog fewer, re-enabling the main window after the last.
    fn drop(&mut self) {
        let depth = DEPTH.get().saturating_sub(1);
        DEPTH.set(depth);
        if depth == 0
            && let Some(main) = self.main.upgrade()
        {
            set_owner_enabled(&main, true);
        }
    }
}

/// Clears [`PENDING_OWNER`] when dropped, so a failed dialog creation cannot
/// leave the next window owned.
struct PendingOwner;

impl PendingOwner {
    fn set(owner: Option<isize>) -> Self {
        PENDING_OWNER.set(owner);
        Self
    }
}

impl Drop for PendingOwner {
    fn drop(&mut self) {
        PENDING_OWNER.set(None);
    }
}

type Open<D> = Option<(D, ModalGuard)>;

/// The place one kind of modal dialog lives while it is open.
///
/// Opening fills the slot; closing empties it at once, so a follow-up dialog can
/// open in the same turn, while the teardown waits for the next turn.
pub struct ModalSlot<D: ComponentHandle + 'static> {
    open: Rc<RefCell<Open<D>>>,
}

impl<D: ComponentHandle + 'static> Default for ModalSlot<D> {
    fn default() -> Self {
        Self {
            open: Rc::default(),
        }
    }
}

impl<D: ComponentHandle + 'static> ModalSlot<D> {
    /// Creates a dialog with `create`, shows it owned by and modal to `main`,
    /// and keeps it here until [`Self::close`]. The title-bar close button
    /// closes it the same way. A dialog already open here is closed first.
    ///
    /// `create` should construct the component and set its properties and
    /// callbacks; the dialog's buttons should end by calling [`Self::close`].
    ///
    /// # Errors
    /// The `PlatformError` from creating or showing the dialog. The main window
    /// is left enabled then.
    pub fn open(
        &self,
        main: &MainWindow,
        create: impl FnOnce() -> Result<D, PlatformError>,
    ) -> Result<(), PlatformError> {
        self.close();
        let guard = ModalGuard::new(main);
        let dialog = {
            // Slint creates the winit window attributes when the component's
            // window adapter is made, at the latest on `show`.
            let _owner = PendingOwner::set(hwnd_of(main.window()));
            let dialog = create()?;
            dialog.show()?;
            dialog
        };

        let slot = Rc::downgrade(&self.open);
        dialog.window().on_close_requested(move || {
            close_slot(&slot);
            // `close_slot` hides the window itself, after re-enabling the owner.
            CloseRequestResponse::KeepWindowShown
        });
        center_when_created(main, &dialog);

        *self.open.borrow_mut() = Some((dialog, guard));
        Ok(())
    }

    /// Closes the dialog, if one is open: empties the slot now, and on the next
    /// turn re-enables the main window, hides the dialog and drops it, in that
    /// order.
    pub fn close(&self) {
        close_slot(&Rc::downgrade(&self.open));
    }

    /// A handle to the open dialog, if any.
    pub fn dialog(&self) -> Option<D> {
        self.open
            .borrow()
            .as_ref()
            .map(|(dialog, _)| dialog.clone_strong())
    }
}

/// [`ModalSlot::close`] on a weak slot, for callbacks the slot's own dialog holds.
fn close_slot<D: ComponentHandle + 'static>(slot: &Weak<RefCell<Open<D>>>) {
    let Some(slot) = slot.upgrade() else { return };
    let Some((dialog, guard)) = slot.borrow_mut().take() else {
        return;
    };
    slint::Timer::single_shot(Duration::ZERO, move || {
        drop(guard);
        // Hiding an already hidden window is harmless; there is nothing to do
        // about a failure to hide one that is being dropped anyway.
        let _ = dialog.hide();
        drop(dialog);
    });
}

/// Centres `dialog` over the top third of `main` once its winit window exists;
/// winit creates it asynchronously, after `show`.
fn center_when_created<D: ComponentHandle + 'static>(main: &MainWindow, dialog: &D) {
    let (dialog, main) = (dialog.as_weak(), main.as_weak());
    // `spawn_local` fails only without an event loop (headless), where there is
    // no position to fix.
    let _ = slint::spawn_local(async move {
        let Some(dialog) = dialog.upgrade() else {
            return;
        };
        if dialog.window().winit_window().await.is_err() {
            return;
        }
        let Some(main) = main.upgrade() else { return };
        let (position, size) = (main.window().position(), main.window().size());
        let own = dialog.window().size();
        dialog.window().set_position(PhysicalPosition::new(
            position.x + (size.width as i32 - own.width as i32) / 2,
            position.y + (size.height as i32 - own.height as i32) / 3,
        ));
    });
}
