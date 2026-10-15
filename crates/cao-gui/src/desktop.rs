//! The native dialogs and shell calls the window makes: message boxes, the
//! folder picker, and opening a URL or file.
//!
//! They sit behind [`Desktop`] because native dialogs cannot run under the
//! headless test backend: they would block or pop up. [`NativeDesktop`] is the
//! real one; the tests record the requests instead.

use std::path::{Path, PathBuf};

/// How a message box presents itself (QMessageBox's information, warning and
/// critical boxes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLevel {
    Info,
    Warning,
    Error,
}

/// The native dialogs and shell calls the window makes.
///
/// Every method returns at once; a dialog's answer arrives through its `then`
/// continuation, which runs on the UI thread after the dialog closes. That is
/// how the window chains a tutorial box into the dialog that follows it.
pub trait Desktop {
    /// Shows a message box with an OK button. With an `owner`, it is modal to
    /// that window; without one it stands alone, like C++'s first-start box.
    fn message(
        &self,
        owner: Option<&slint::Window>,
        level: MessageLevel,
        title: &str,
        text: &str,
        then: Box<dyn FnOnce()>,
    );

    /// Asks for a folder, starting in `start` when given, modal to `owner`.
    /// `then` receives `None` when the user cancels.
    fn pick_folder(
        &self,
        owner: &slint::Window,
        start: Option<&Path>,
        then: Box<dyn FnOnce(Option<PathBuf>)>,
    );

    /// Opens a URL or file with its default handler (C++
    /// `QDesktopServices::openUrl`).
    fn open(&self, target: &str);
}

/// The real dialogs: `rfd` 0.17 with `set_parent` for owned modality (#462),
/// and Explorer for opening. `rfd`'s `common-controls-v6` feature stays off; it
/// crashes with `STATUS_ENTRYPOINT_NOT_FOUND` without a ComCtl32 v6 manifest
/// entry (#471).
pub struct NativeDesktop;

impl Desktop for NativeDesktop {
    fn message(
        &self,
        owner: Option<&slint::Window>,
        level: MessageLevel,
        title: &str,
        text: &str,
        then: Box<dyn FnOnce()>,
    ) {
        let mut dialog = rfd::AsyncMessageDialog::new()
            .set_level(match level {
                MessageLevel::Info => rfd::MessageLevel::Info,
                MessageLevel::Warning => rfd::MessageLevel::Warning,
                MessageLevel::Error => rfd::MessageLevel::Error,
            })
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::Ok);
        if let Some(owner) = owner {
            dialog = dialog.set_parent(&owner.window_handle());
        }
        // `spawn_local` fails only without an event loop, and then nothing
        // could run the continuation anyway.
        let _ = slint::spawn_local(async move {
            dialog.show().await;
            then();
        });
    }

    fn pick_folder(
        &self,
        owner: &slint::Window,
        start: Option<&Path>,
        then: Box<dyn FnOnce(Option<PathBuf>)>,
    ) {
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Open Directory")
            .set_parent(&owner.window_handle());
        if let Some(start) = start.filter(|start| start.is_dir()) {
            dialog = dialog.set_directory(start);
        }
        // As in `message`: without an event loop nothing could run `then`.
        let _ = slint::spawn_local(async move {
            let folder = dialog.pick_folder().await;
            then(folder.map(|folder| folder.path().to_path_buf()));
        });
    }

    fn open(&self, target: &str) {
        // Explorer hands the target to its default handler. Its exit code says
        // nothing useful (it is 1 even on success), and a failure to start it
        // leaves nothing the user could act on, so the result is ignored.
        let _ = std::process::Command::new("explorer").arg(target).spawn();
    }
}
