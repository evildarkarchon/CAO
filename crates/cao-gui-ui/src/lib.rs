//! CAO's main window and dialogs, compiled from `ui/*.slint` (#508).
//!
//! This crate holds Slint's generated code only; the window's behaviour is in
//! `cao-gui`. It exists because the generated code needs `unsafe`, which
//! `cao-gui` forbids (see this crate's `Cargo.toml`).
//!
//! What it exports: `MainWindow` with the top rows, menu and run state; the
//! per-tab globals `BsaTab`, `MeshesTab`, `TexturesTab` and `AnimationsTab`;
//! `TabInfo`; the Log tab's `LogTab` global with its `LogRow` and
//! `LogSeverity` (#511); `UiRules`, what the UI rules leave enabled (#509); and
//! the modal dialogs `TextInputDialog`, `ChoiceDialog`, `AboutSlintDialog` and
//! `FormatsDialog` with its `FormatItem` rows (#509).

slint::include_modules!();
