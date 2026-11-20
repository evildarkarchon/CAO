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

    /// Asks a two-way question in a message box modal to `owner`, with one
    /// button labelled `yes` and one labelled `no`. `then` receives `true`
    /// for `yes`, and `false` for `no` or when the box is closed some other
    /// way. `no` comes first and is the default that Enter picks, so a
    /// question whose `yes` loses something should be asked this way round.
    // The two labels belong with the question; a struct for one call site
    // would only move the same seven values around.
    #[allow(clippy::too_many_arguments)]
    fn ask(
        &self,
        owner: &slint::Window,
        level: MessageLevel,
        title: &str,
        text: &str,
        yes: &str,
        no: &str,
        then: Box<dyn FnOnce(bool)>,
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
/// and Explorer for opening. `rfd`'s `common-controls-v6` feature draws the
/// message boxes as task dialogs, which can label their buttons; it relies on
/// the manifest's ComCtl32 v6 entry, without which the exe would fail to start
/// with `STATUS_ENTRYPOINT_NOT_FOUND` (#462, GUI-14).
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
            .set_level(rfd_level(level))
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

    fn ask(
        &self,
        owner: &slint::Window,
        level: MessageLevel,
        title: &str,
        text: &str,
        yes: &str,
        no: &str,
        then: Box<dyn FnOnce(bool)>,
    ) {
        // `no` in rfd's OK slot: the task dialog makes its first button the
        // default, and the default should be the answer that loses nothing.
        // Escape and the title bar return `Cancel`, which is not `yes` either.
        let dialog = rfd::AsyncMessageDialog::new()
            .set_level(rfd_level(level))
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                no.to_owned(),
                yes.to_owned(),
            ))
            .set_parent(&owner.window_handle());
        let yes = yes.to_owned();
        // As in `message`: without an event loop nothing could run `then`.
        let _ = slint::spawn_local(async move {
            let answer = dialog.show().await;
            then(answer == rfd::MessageDialogResult::Custom(yes));
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

/// rfd's name for a message box's level.
fn rfd_level(level: MessageLevel) -> rfd::MessageLevel {
    match level {
        MessageLevel::Info => rfd::MessageLevel::Info,
        MessageLevel::Warning => rfd::MessageLevel::Warning,
        MessageLevel::Error => rfd::MessageLevel::Error,
    }
}
