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

use cao_gui_unsafe::WorkArea;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::platform::windows::{WindowAttributesExtWindows, WindowExtWindows};
use slint::winit_030::winit::window::{WindowAttributes, WindowButtons};
use slint::{CloseRequestResponse, ComponentHandle, PhysicalPosition, PhysicalSize, PlatformError};

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
    /// Publishes `owner` (a window handle, or `None`) as the owner of the next
    /// window created on this thread, until the returned guard drops.
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

/// Centres `dialog` over the top third of `main` once its winit window exists,
/// then moves it inside the work area of `main`'s monitor, so its buttons are
/// never under the taskbar or off the screen when `main` sits low or near an
/// edge (GUI-6). winit creates the window asynchronously, after `show`.
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
        let centred = PhysicalPosition::new(
            position.x + (size.width as i32 - own.width as i32) / 2,
            position.y + (size.height as i32 - own.height as i32) / 3,
        );
        // `set_position` places the frame, so the clamp needs the frame's size
        // too; the client size stands in should winit not give it.
        let frame = dialog
            .window()
            .with_winit_window(|window| {
                let outer = window.outer_size();
                PhysicalSize::new(outer.width, outer.height)
            })
            .unwrap_or(own);
        let placed = match hwnd_of(main.window()).and_then(cao_gui_unsafe::monitor_work_area) {
            Some(area) => clamp_to_work_area(centred, frame, area),
            None => centred,
        };
        dialog.window().set_position(placed);
    });
}

/// Moves a window whose frame is `frame` and would sit at `position` the
/// least distance that puts it inside `area`. A window larger than the area
/// keeps its top left corner inside it, so its title bar stays reachable.
fn clamp_to_work_area(
    position: PhysicalPosition,
    frame: PhysicalSize,
    area: WorkArea,
) -> PhysicalPosition {
    // Lower bound last, so a frame wider or taller than the area is pinned to
    // the area's left or top edge.
    let clamp = |at: i32, extent: u32, low: i32, high: i32| {
        let extent = i32::try_from(extent).unwrap_or(i32::MAX);
        at.min(high.saturating_sub(extent)).max(low)
    };
    PhysicalPosition::new(
        clamp(position.x, frame.width, area.left, area.right),
        clamp(position.y, frame.height, area.top, area.bottom),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1366×768 screen with a 40px taskbar along the bottom.
    const LAPTOP: WorkArea = WorkArea {
        left: 0,
        top: 0,
        right: 1366,
        bottom: 728,
    };

    fn clamp(x: i32, y: i32, width: u32, height: u32, area: WorkArea) -> (i32, i32) {
        let placed = clamp_to_work_area(
            PhysicalPosition::new(x, y),
            PhysicalSize::new(width, height),
            area,
        );
        (placed.x, placed.y)
    }

    #[test]
    fn a_dialog_inside_the_work_area_stays_where_it_was_centred() {
        assert_eq!(clamp(400, 100, 340, 520, LAPTOP), (400, 100));
    }

    #[test]
    fn a_dialog_reaching_under_the_taskbar_moves_up_just_enough() {
        assert_eq!(clamp(400, 300, 340, 520, LAPTOP), (400, 208));
    }

    #[test]
    fn a_dialog_past_a_side_moves_back_onto_the_screen() {
        assert_eq!(clamp(1200, 100, 340, 520, LAPTOP), (1026, 100));
        assert_eq!(clamp(-50, -20, 340, 520, LAPTOP), (0, 0));
    }

    #[test]
    fn a_dialog_taller_than_the_work_area_keeps_its_title_bar_on_screen() {
        assert_eq!(clamp(400, 300, 340, 800, LAPTOP), (400, 0));
    }

    /// A second monitor to the left of the primary one has negative
    /// coordinates, and its taskbar may be at the top.
    #[test]
    fn a_monitor_left_of_the_primary_one_clamps_in_its_own_coordinates() {
        let left = WorkArea {
            left: -1920,
            top: 48,
            right: 0,
            bottom: 1080,
        };
        assert_eq!(clamp(-200, 10, 340, 520, left), (-340, 48));
    }
}
