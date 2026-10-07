# Reproducing the main window's hard parts in Slint

Research for [#462](https://github.com/evildarkarchon/CAO/issues/462) on the
[#458](https://github.com/evildarkarchon/CAO/issues/458) map ("Port CAO to Rust and Slint").

**Question.** Can current Slint (1.18.x) plus helper crates reproduce the parts of CAO's main window
that are hard to build, and how? The target is structural parity with `src/MainWindow.ui` and
`src/TexturesFormatSelectDialog.ui`.

**Versions checked (2026-10-06).** `slint` 1.18.1 (released 2026-09-21; the newest on crates.io),
`rfd` 0.17.2, and `winit` 0.30.13 (the version Slint 1.18.1's winit backend pulls in). Facts come from
the Slint source at tag [`v1.18.1`](https://github.com/slint-ui/slint/tree/v1.18.1), the crate
sources that Cargo downloaded, the docs on docs.slint.dev and docs.rs, and Slint's GitHub issues.
I also built a throwaway probe app outside the repo with Slint 1.18.1 (Fluent style, winit backend),
compiled it, ran it on Windows 11, and took screenshots. In this file, **"Probe"** means the
throwaway app showed the behavior, **"Source"** means I read it in the source code at the version
above, and **"Issue"** means it comes from Slint's issue tracker.

## Summary

| Feature (from the `.ui` files) | Mechanism or workaround in Slint 1.18.1 | Feasibility |
|---|---|---|
| Checkable group boxes ("Process meshes", "Process textures", "Downsizing") | Write a small custom `CheckableGroupBox`: a `CheckBox` as the title and a bordered body. Each child binds its own `enabled` to `checked`, because Slint does not pass `enabled` down to children. | Feasible, custom code (about 30 lines) |
| `QTabWidget` with `movable=true` and per-tab enable/disable | The standard `TabWidget` cannot do either. Write a custom tab bar: `for` over a `[TabInfo]` model, a `TouchArea` drag that calls a Rust `move-tab`, and an `enabled` flag on each tab. Show content with `if current == id`. | Feasible, custom code (about 60 lines) |
| Dropping a folder from Explorer onto the window | `DropArea` only handles drags that start inside the app. Explorer drops do not reach it on the winit backend. Use `WinitWindowAccessor::on_winit_window_event` (feature `unstable-winit-030`) and handle `WindowEvent::DroppedFile(PathBuf)`. | Feasible, but through an API Slint marks unstable |
| Menu bar with checkable items | Built-in `MenuBar`/`Menu`/`MenuItem` with `checkable: true; checked <=> ...`. | Feasible, built in |
| Rich-text tooltips (26) and `WhatsThisCursor` (20) | Built-in `Tooltip { text: @markdown(...) }` with `**bold**`, and `@tr` values inserted with `\{...}`. Cursor: `TouchArea { mouse-cursor: help; }`. The std widgets do not expose `mouse-cursor`, so this needs an overlay that also forwards clicks. | Tooltips: feasible, built in. Help cursor: feasible with a workaround |
| Indeterminate progress bar with text, and a status bar | `ProgressIndicator { indeterminate: true; }` with a `Text` drawn on top. There is no status-bar widget, so use a bottom `Text` row. | Feasible |
| Read-only log view: coloured tail of the HTML log | Parse the plog HTML in Rust into `{text, color}` rows and show them in a `ListView` of plain `Text`. `StyledText::from_markdown` rejects the current log markup. | Feasible. Text selection is lost unless rows use `TextInput` |
| Searchable, checkable list of 75 formats that hides non-matches | `VecModel<FormatItem>` wrapped in `slint::FilterModel`; `ListView` of `CheckBox`; `unfiltered_row` writes changes back to the source. | Feasible, built in |
| Modal dialogs: folder picker, text input, list choice, info/critical, About | Folder picker and info/critical boxes: `rfd`, with the Slint window as parent (native and modal). Text input, list choice, unwanted formats, About: Slint `Dialog` windows. Slint has **no modal windows**, so disable the main window while a dialog is open (`WindowExtWindows::set_enable(false)`), or draw the dialog inside the main window. | Feasible. Modality is a workaround |
| Switching dark and light mode at runtime | `export { Palette }`, then `app.global::<Palette>().set_color_scheme(...)`. | Feasible for everything Slint draws. **The native menu bar does not follow it** (see below) |
| Delivering background-thread events to the UI | `Weak::upgrade_in_event_loop` / `slint::invoke_from_event_loop`. | Feasible, built in. Probe confirms |
| Is the MenuBar native or drawn by Slint on Windows? | **Native Win32 menu** through the `muda` crate. It sits in the non-client area under the title bar and follows the **Windows system theme**. Setting the environment variable `SLINT_NO_MUDA` makes Slint draw it instead. | Answered. Probe screenshots |

Nothing is infeasible. Four items rely on workarounds:

- folder drop: an unstable winit hook;
- dialog modality: Slint has no modal windows;
- native menu-bar theming: the menu follows the system theme, not the app's toggle;
- the help cursor: it needs an overlay.

## Details

### 1. Checkable group boxes

- The `.ui` file has three checkable boxes: `meshesGroupBox` ("Process meshes"), `texturesGroupBox`
  ("Process textures"), and `texturesResizingGroupBox` ("Downsizing"). `bsaBaseGroupBox` and
  `animationsGroupBox` declare `checkable=false`.
- Slint's `GroupBox` has only `title`, `enabled`, and `content-padding`. It draws the title as a
  `Text` and does not pass `enabled` to its children (Source:
  [`widgets/fluent/groupbox.slint`](https://github.com/slint-ui/slint/blob/v1.18.1/internal/compiler/widgets/fluent/groupbox.slint)).
  A request to add a checkable GroupBox is still open
  ([slint#8833](https://github.com/slint-ui/slint/issues/8833)). The pull request for it, #8851, was
  closed unmerged because it only covered one style (Issue).
- **Workaround.** A `CheckableGroupBox` component with `in-out property <bool> checked`, a `CheckBox`
  title, and a bordered body containing `@children`. Each child sets
  `enabled: group.checked && <its own rule>`. The `RadioGroup` widget has its own `enabled`
  property that also disables its buttons (`group-enabled` in
  [`radiogroup.slint`](https://github.com/slint-ui/slint/blob/v1.18.1/internal/compiler/widgets/fluent/radiogroup.slint)).
  The probe compiled and rendered this (Probe).
- **Interaction with other rules.** Several rules feed the same `enabled` flags: Dry Run, Several
  mods, "base profile is read-only", "a run is active", and the group's checkbox. Qt combines these
  through widget-tree inheritance. Slint needs one explicit expression per control. This is a design
  point for the prototype (#466), not a blocker.

### 2. Tab widget: user-reorderable tabs and per-tab enable

- The `.ui` sets `tabWidget.movable = true`. `MainWindow::setGameMode` calls `setTabEnabled` for
  the BSA, Meshes, Textures, and Animations tabs. During a run the whole tab widget is disabled.
- The standard `TabWidget` in Slint 1.18.1 does not support this. `Tab` declares only `title`
  ([`builtin_elements.rs`, `Tab`](https://github.com/slint-ui/slint/blob/v1.18.1/internal/compiler/builtin_elements.rs)).
  The lowering pass connects only `title`, `current`, `current-focused`, `tab-index`, and
  `num-tabs` to the internal `TabImpl`. `TabImpl` does have an `enabled` property, but nothing a
  user writes can reach it. The same pass also rejects tabs built with `for` or `if`
  ("dynamic tabs ('if' or 'for') are currently not supported"), so the tabs cannot be reordered
  either (Source:
  [`passes/lower_tabwidget.rs`](https://github.com/slint-ui/slint/blob/v1.18.1/internal/compiler/passes/lower_tabwidget.rs)).
  A broader tab-customization request is open
  ([slint#11674](https://github.com/slint-ui/slint/issues/11674)) (Issue).
- **Workaround.** A custom tab bar: `for tab[i] in tabs` draws a header `Rectangle` per tab. A
  `TouchArea` shifts the header's `x` while it is dragged past a small threshold. On release it calls
  `move-tab(from, to)`, and Rust does `VecModel::remove` and `insert`. A disabled tab is drawn greyed
  out and ignores clicks. Content panes are `if current-id == N: ...`. The probe compiled and
  rendered this, including a disabled "Animations" tab (Probe). I did not test the drag with a real
  mouse.
- Qt does not save the tab order, so the custom bar does not need to either.
- Slint 1.17 also added `DragArea`/`DropArea`, which can be used inside the app for reordering
  ([guide](https://docs.slint.dev/latest/docs/slint/guide/development/drag-and-drop/)). But their
  `data-transfer` payload is opaque and must be built in Rust. A plain `TouchArea` is simpler for
  this case.

### 3. Dropping a folder onto the window

- C++ behavior: `setAcceptDrops(true)`. `dropEvent` takes the first URL and, if it is an existing
  directory, puts it in `userPathTextEdit`.
- Slint 1.17 added `DragArea`/`DropArea`/`data-transfer`. The docs say a `DropArea` "also accepts
  drops from other applications" on platforms that support it
  ([guide](https://docs.slint.dev/latest/docs/slint/guide/development/drag-and-drop/)). But in
  1.18.1 **the winit backend does not pass OS drags on to Slint**. The backend that injects drags
  (`WindowInner::process_drag_event` with `BackendDragEvent`) is only called by the testing backend.
  The winit backend never handles winit's `DroppedFile`/`HoveredFile` (Source: searched
  `internal/backends/winit` and `internal/core/window.rs`). Slint's maintainers say drops between
  windows are done on master but need a winit 0.31 release
  ([slint#1967](https://github.com/slint-ui/slint/issues/1967), closed 2026-07-19) (Issue).
- **Workaround (works today).** winit 0.30 on Windows registers an OLE drop target by default
  (`drag_and_drop: true`, see `winit/src/platform_impl/windows/mod.rs`) and sends
  `WindowEvent::DroppedFile(PathBuf)` once per dropped file. Slint lets the app see raw winit events
  through `slint::winit_030::WinitWindowAccessor::on_winit_window_event`, behind the cargo feature
  `unstable-winit-030`
  ([docs.rs](https://docs.rs/slint/1.18.1/slint/winit_030/trait.WinitWindowAccessor.html)). The
  filter runs for every window event before Slint handles it (Source:
  `winitwindowadapter.rs`, `window_event_filter`). The probe registers the filter right after
  `ProbeWindow::new()` and compiles (Probe). I did not try a real Explorer drop.
- Caveats:
  - The feature flag is documented as "may be removed or changed in future minor releases", so pin
    `slint = "~1.18"`.
  - `DroppedFile` has no position, but CAO only needs a drop anywhere on the window.
  - Multiple files arrive as several events. Keep the first one, as Qt did.
  - winit's docs warn that its drop target conflicts with code that sets up multi-threaded COM
    (`COINIT_MULTITHREADED`) on the UI thread. Keep MTA COM, such as DirectXTex's GPU path, off the
    UI thread. The planned worker threads already do this.
  - Once Slint ships drops from other applications through `DropArea` (winit 0.31), the hook can be
    replaced. Nothing else changes.

### 4. Menu bar with checkable items

- `MenuItem` has `checkable`, `in-out checked`, `enabled`, `shortcut`, and `icon`. When the user
  activates a checkable item, its `checked` value flips. Only one `MenuBar` is allowed per `Window`,
  and it cannot sit inside `if` or `for`
  ([`builtin_elements.rs`, `MenuItem`/`MenuBar`](https://github.com/slint-ui/slint/blob/v1.18.1/internal/compiler/builtin_elements.rs);
  [Window docs, MenuBar](https://docs.slint.dev/latest/docs/slint/reference/window/window/#menubar)).
  The native backend maps checkable items to `muda::CheckMenuItem` (Source: `muda.rs`).
- Probe: "Enable dark theme", "Enable debug log", and "Show tutorials" written as
  `checkable: true; checked <=> root.<prop>;` compile and show in the native menu (Probe).
- `MenuItem` has no tooltip. The `.ui` puts a tooltip on `actionEnable_debug_log` ("Used when
  reporting bugs"). `QMenu` hides tooltips by default (`toolTipsVisible` is false), so this tooltip
  never appeared in the C++ app. Dropping it does not change anything the user sees.

### 5. Rich-text tooltips and the What's This cursor

- **Tooltips.** Slint 1.17 added a built-in `Tooltip` element. Its `text` is `styled-text`, or you
  can give it custom child content (Source: `builtin_elements.rs`, `Tooltip`;
  [docs](https://docs.slint.dev/latest/docs/slint/reference/window/tooltip/)). Styled text supports
  CommonMark emphasis (`*em*`, `**strong**`), strikethrough, code, links, lists, `<u>`, and
  `<font color="...">`. It does not support headings, other HTML tags, and so on
  ([`StyledText` docs](https://docs.slint.dev/latest/docs/slint/reference/elements/styledtext/);
  Source: `internal/common/styled_text.rs`).
- Of the 26 `toolTip` properties, 25 are Qt rich text and 1 is plain (the debug-log action). Only
  4 contain bold spans (`font-weight:600`: create BSA, extract BSA, separate textures, separate
  incompressible). One more has `font-weight:400` spans that look like plain text. All of them
  translate to `**...**` markdown.
- **`@tr` together with `@markdown`.** `@markdown("...")` does not translate its own text. Values
  inserted with `\{expr}` are converted to plain styled text (Source: `passes/resolving.rs`,
  `from_at_markdown`). The pattern that compiles is shown below (Probe). Because the port ships
  English only, this keeps every string inside `@tr` for later extraction.

  ```slint
  Tooltip {
      text: @markdown("\{@tr("Creates a new BSA, packing the existing loose files.")} **\{@tr("Warning")}**. \{@tr("If you enable this option, the process will be considerably slowed down.")}");
  }
  ```

- **Tooltip risks.**
  - Tooltips are drawn inside the window, so they get cut off near the window edge
    ([slint#12260](https://github.com/slint-ui/slint/issues/12260), open; the fix is the open pull
    request [#11740](https://github.com/slint-ui/slint/pull/11740), "Native winit popup").
  - They appear next to the pointer, not anchored to the element
    ([slint#13091](https://github.com/slint-ui/slint/issues/13091), open).
  - CAO's window is 676 px wide, and several controls with tooltips sit at the right edge. Keep
    tooltip text wrapped to a modest width.
- **`WhatsThisCursor`** appears 20 times in the `.ui`. Slint's `MouseCursor.help` maps to winit
  `CursorIcon::Help`, which is the Windows `IDC_HELP` arrow-plus-question-mark, the same cursor Qt
  shows (Source: `winitwindowadapter.rs`). Only `TouchArea` has `mouse-cursor`. When several nested
  `TouchArea`s are hovered, the innermost one sets the cursor (Source: `items/input_items.rs`,
  `input_event_filter_before_children`), and the Fluent `CheckBox` contains its own `TouchArea`.
  So wrapping a `CheckBox` in a help-cursor `TouchArea` does **not** work: the inner area resets the
  cursor. The workaround is an overlay `TouchArea { mouse-cursor: help; clicked => { toggle the
  widget } }` placed on top of the widget. The other option is a CAO-owned copy of the Fluent
  `CheckBox`/radio button visuals with `mouse-cursor` set (allowed: the style sources are under
  GPL-3.0). I reasoned this from the source and did not test it with a mouse. Check it in #466.

### 6. Indeterminate progress bar with text, and a status bar

- C++ shows "busy" with `setRange(0, 0)`, sets the label with `setFormat(text)`, and copies the same
  text to `statusBar()->showMessage()`. Some Qt styles hide the text during the busy animation.
- Slint's `ProgressIndicator` has `progress` (a float from 0 to 1) and `indeterminate`. It has no
  text, and the Fluent version is 3 px high by default (Source:
  [`fluent/progressindicator.slint`](https://github.com/slint-ui/slint/blob/v1.18.1/internal/compiler/widgets/fluent/progressindicator.slint)).
  The probe stacked a 20 px `ProgressIndicator { indeterminate: true; }` and a centered `Text` in
  one `Rectangle`. The animation runs and the text stays readable in both dark and light mode
  (Probe, screenshots). Because `progress` is a float, the C++ workaround of keeping counts above
  `INT_MAX` text-only is not needed.
- There is no status-bar widget. A bottom `Text` row in the main layout is enough, and the probe
  shows it.

### 7. Read-only log view (coloured tail of the HTML log)

- Current behavior:
  - Every 5 s during a run, and after each run, `updateLog()` re-reads the whole configured log
    file.
  - It `appendHtml`s each line into a `QPlainTextEdit` with `maximumBlockCount = 25`.
  - It then appends the structured run details as plain text.
  - Each log line has the form `<br><font color=DarkRed>timestamp SEVERITY{func@line} message</font>`
    (`src/Logger.h`). The colours are DarkRed, Red, Orange, Green, Blue, and Purple, one per
    severity, and the file starts with a `<style>` header.
  - The 25-block cap covers the run details too, so the view shows the last 25 blocks of log lines
    and details together.
- **Don't feed these lines to `StyledText`.** The probe passed real samples to
  `StyledText::from_markdown` (Probe):
  - `<br>` fails: "HTML tag `<br>` is not supported".
  - `color=DarkRed` without quotes fails: invalid quote.
  - `color="Orange"` fails: the colour names are lower-case CSS names and the match is
    case-sensitive (Source: `internal/common/color_parsing.rs`).
  - The `<style>` header fails: "Markdown HTML blocks are not supported".
  - `C:\mods\my_mod_name\__x__.dds` parses without error, but Markdown would quietly reformat
    underscores and asterisks in user paths.
- **Recommended.** Parse each log line in Rust: strip `<br>`, map the `<font color>` value (or the
  severity) to a `slint::Color`, and expose a `[LogLine { text, color }]` model. Show it in a
  `ListView` of `Text { wrap: word-wrap; }`. The probe renders this (Probe).
- The rows cannot be selected or copied, unlike the read-only `QPlainTextEdit`. If that matters,
  make each row a read-only `TextInput`. The Logging-implementation item on the map could also
  record each line's severity directly, so the GUI does not need to scrape HTML.

### 8. Searchable, checkable list of 75 formats

- `TexturesFormatSelectDialog` holds 75 entries, one `DEFFMT(...)` per entry in `src/texturesformats.h`. Typing in
  the `QLineEdit` hides rows that do not contain the text. Qt's `MatchContains` ignores case by
  default.
- Slint: put the source `VecModel<FormatItem { name, checked }>` inside
  `slint::FilterModel::new(model, |item| ...)` and bind it to a `ListView` of `CheckBox`es. On every
  `edited`, call `FilterModel::reset()`. When a checkbox toggles, `FilterModel::unfiltered_row(row)`
  gives the source row to update
  ([docs.rs FilterModel](https://docs.rs/slint/1.18.1/slint/struct.FilterModel.html)). The probe
  compiles this with 75 items (Probe).
- The deviation list says "Cancel" must revert edits. To do that, copy the checked set when the
  dialog opens and restore it on cancel.

### 9. Modal dialogs

What C++ uses:

- `QFileDialog::getExistingDirectory` (the folder picker);
- `QInputDialog::getText` (new profile name);
- `QInputDialog::getItem` (base profile);
- `QMessageBox::information` (the tutorial pop-ups and first-start welcome);
- `QMessageBox::critical` (start and log errors);
- `QMessageBox::about` and `aboutQt`;
- the window-modal `TexturesFormatSelectDialog::open()`.

Facts:

- **Slint has no modal windows.** The request is still open
  ([slint#6607](https://github.com/slint-ui/slint/issues/6607)), and a maintainer comment from
  2025-06 says winit lacks the support it needs (Issue). There is no `show_modal` in 1.18.1 (Source:
  searched `api.rs`, `window.rs`, the winit backend). A Slint `Dialog` is a top-level `Window`
  whose `StandardButton`s are laid out for you. Each `kind: ok` or `kind: cancel` button gets a
  matching `ok_clicked`/`cancel_clicked` callback that Rust can handle (Source:
  `builtin_elements.rs`, `Dialog`; [docs](https://docs.slint.dev/latest/docs/slint/reference/window/dialog/)).
  `AboutSlint` is a std widget (compiled in the probe).
- **rfd 0.17.2.**
  - `FileDialog`/`AsyncFileDialog::pick_folder()`, `set_directory`, `set_title`, `set_parent`.
  - `MessageDialog`/`AsyncMessageDialog` with `MessageLevel::{Info, Warning, Error}`.
  - `set_parent` takes anything with `HasWindowHandle + HasDisplayHandle`.
  - Slint's `Window::window_handle()` provides both with the cargo feature `raw-window-handle-06`.
    The probe compiles `rfd::AsyncFileDialog::new().set_parent(&app.window().window_handle())`
    (Probe).
  - On Windows, the async variants run the dialog on a spawned thread (Source:
    `rfd/src/backend/win_cid`). Await them inside `slint::spawn_local`.
- **rfd `common-controls-v6` needs an application manifest.** With that feature on and no manifest,
  the probe failed to start with `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139), because
  `TaskDialogIndirect` is only exported by ComCtl32 v6 (Probe; rfd crate docs). CAO's current
  `Cathedral_Assets_Optimizer.manifest` has no ComCtl32 v6 dependency. Leave the feature off: CAO
  only needs OK buttons, which plain `MessageBoxW` handles. Otherwise add the dependency to the
  manifest.
- **Recommendation.**
  - **Folder picker:** `rfd::AsyncFileDialog::pick_folder` with the main window as parent. This is
    the native `IFileDialog`, the same dialog Qt used on Windows.
  - **Info and critical boxes (tutorials, welcome, start errors):** `rfd::AsyncMessageDialog` with
    the main window as parent. These are native and truly modal. They follow the system theme, not
    the app's toggle; with QDarkStyle the Qt boxes were dark.
  - **Text input, list choice, unwanted formats, About:** Slint `Dialog` components, so they stay
    separate windows like in Qt. Make each one modal yourself:
    - call `main.window().with_winit_window(|w| w.set_enable(false))` (`winit::platform::windows::WindowExtWindows`) before `show()`;
    - re-enable before `hide()`;
    - if needed, set an owner window so the dialog stays above the main window and has no taskbar
      entry. Do this either with `BackendSelector::with_winit_window_attributes_hook` +
      `WindowAttributesExtWindows::with_owner_window` (the hook applies to every window, so a
      thread-local "next owner" is needed), or with a Win32 `SetWindowLongPtrW(GWLP_HWNDPARENT)`
      call on the raw handle.

    The probe compiles the `set_enable` path. I did not exercise it with a mouse.
  - Simpler alternative: draw these dialogs inside the main window, as a full-window `Rectangle`
    that swallows input plus a centered panel. That is modal by construction, but they would no
    longer be separate windows.
- **Control flow.** `createProfile` is one blocking sequence: tutorial box, then `getText`, then
  `getItem`. In Rust it becomes one `slint::spawn_local(async { ... })` that awaits each dialog. A
  Slint `Dialog` needs a small future adapter, for example a oneshot channel completed from
  `ok_clicked`/`cancel_clicked`.
- `firstStart()` runs before the event loop. A synchronous `rfd::MessageDialog::show()` before
  `app.run()` works there.

### 10. Switching dark and light mode at runtime

- Every std-widgets style exposes a `Palette` global with `in-out property <ColorScheme>
  color-scheme`. Fluent derives all of its colours from it (Source: `fluent/styling.slint`,
  `fluent/style-base.slint`;
  [Palette docs](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/globals/palette/)).
  Re-export it (`export { Palette } from "std-widgets.slint";`) and call
  `app.global::<Palette>().set_color_scheme(ColorScheme::Dark | Light)`. The probe switches every
  Slint-drawn widget this way (Probe, screenshots `dark` vs `light`).
- **The native menu bar and title bar do not follow** that property:
  - The `muda` menu gets its theme from `WinitWindowAdapter::set_color_scheme`. Only the window's
    system theme calls that function: at creation and on winit `ThemeChanged`. Changing `Palette`
    never reaches it (Source: `winitwindowadapter.rs`, `muda.rs::set_menubar_theme`).
  - winit's Windows `set_theme` changes the title bar but sends no `ThemeChanged`, so it cannot
    re-theme the menu either (Source: `winit/src/platform_impl/windows/window.rs`).
  - Probe screenshots on a dark-mode Windows 11 machine: in light mode the client area is light but
    the menu bar stays dark.
- Options:
  1. Accept it: the native menu and title bar follow Windows, and the content follows CAO's
     setting. This is the lowest-effort choice.
  2. Set the environment variable `SLINT_NO_MUDA` before Slint starts. Slint then draws the menu bar
     itself and it follows `Palette` (Probe screenshot `nomuda-light`). This switch is only an
     environment check with a source comment ("the only way to exercise these code paths"). It is
     not a documented API, so pin the Slint version if you rely on it. On edition 2024 `set_var` is
     `unsafe`; call it first thing in `main`.

  Also call winit `set_theme` on the window after it has been created, so the title bar matches.
- The `.ui` says "Enable dark theme". With the map's "dark Slint style" note, this becomes a
  dark/light switch of one Fluent style. Default it to dark.

### 11. Delivering background-thread events to the UI

- `slint::invoke_from_event_loop(f: FnOnce + Send)` and
  `Weak<T>::upgrade_in_event_loop(f: FnOnce(T) + Send)` queue a closure to run on the
  event-loop thread. `upgrade_in_event_loop` does nothing if the component has been dropped. `Weak`
  is `Send + Sync` ([docs.rs](https://docs.rs/slint/1.18.1/slint/fn.invoke_from_event_loop.html);
  Source: `internal/core/api.rs`).
- On the winit backend the closures go through winit's `EventLoopProxy`. Once the loop has ended
  they return `EventLoopError::EventLoopTerminated`, so a worker can see that the UI is gone and
  must not block on it (Source: `internal/backends/winit/lib.rs`).
- The probe ran a worker thread that updated the progress text every 200 ms with
  `upgrade_in_event_loop`, and the screenshots show it live (Probe).
- This maps onto the current queued delivery in `GuiRunDispatch.cpp`
  (`QMetaObject::invokeMethod(..., Qt::QueuedConnection)`). How to map the Run Handle onto this
  ("drop-cancels-and-waits", one run at a time) is still open and belongs to #468.

### 12. Is the MenuBar native or drawn in the window on Windows?

- **Native.** The winit backend's `muda` feature is on by default (`i-slint-backend-selector`
  turns it on for non-Android targets) and is compiled for Windows and macOS (Source: winit backend
  `build.rs`, `Cargo.toml`). `MudaAdapter::setup` builds a Win32 `HMENU` and attaches it to the
  window's HWND (`init_for_hwnd_with_theme`). Windows draws it in the non-client area directly below
  the title bar.
- The `Window`'s `width` and `height` describe the client area only, excluding the menu bar
  ([Window docs](https://docs.slint.dev/latest/docs/slint/reference/window/window/#menubar)). In the
  probe screenshot, the menu bar is a standard Win11 menu strip ("Tools Help") rendered by `muda`'s
  dark theme.
- Checkable items show the native checkmark. Keyboard shortcuts use `MenuItem.shortcut`. Setting
  `SLINT_NO_MUDA` switches to Slint's in-window menu bar, which looks like a Fluent row with popup
  menus (Probe screenshot).
- Visual parity with Qt: QDarkStyle drew a dark in-window `QMenuBar`. The native menu looks close
  when Windows is in dark mode, but it does not track CAO's toggle.

## Other hard parts the ticket didn't list

- **Radio buttons inside a grid layout.** Slint's `RadioButton` must be a direct child of
  `RadioGroup`, and `RadioGroup` accepts only `RadioButton` children (Source: `builtin_elements.rs`;
  the probe's first compile failed with "RadioButton can only be within a RadioGroup element"). The
  "Downsizing" box has two radio buttons interleaved with four spinboxes in a `QGridLayout`
  (`texturesResizingByRatioRadioButton`/`...BySizeRadioButton`). Either place a vertical
  `RadioGroup` next to a grid of spinboxes and match the row heights, or write a small custom
  exclusive radio component. "Process meshes" (three radio buttons in a row) fits `RadioGroup`
  directly.
- **`QDoubleSpinBox`.** Slint's `SpinBox` is `int`-only (`value`, `minimum`, `maximum`, and
  `step-size` are all `int`; Source: `widgets/common/spinbox-base.slint`). `bsaMaximumSize` is a
  `QDoubleSpinBox` (GB, Qt default of 2 decimals). Use a custom double spin box (a `LineEdit` that
  checks its input, plus up/down buttons), or an int `SpinBox` in hundredths that displays the value
  divided by 100.
- **Close handling.** `closeEvent` that ignores a close during a run maps to
  `Window::on_close_requested` returning `CloseRequestResponse::KeepWindowShown`. This is standard
  Slint API, listed here for completeness.

## Risks

1. **Unstable winit hook.** Folder drops depend on `unstable-winit-030`, and `SLINT_NO_MUDA` (if
   chosen) is an undocumented switch. Pin `slint = "~1.18"` and re-check both when upgrading. Slint
   master plans to support drops from other apps through `DropArea` once winit 0.31 ships.
2. **Hand-made modality.** Slint dialogs are made modal by disabling the main window. Missing a
   re-enable on any exit path, such as an error or the window's close button, leaves the main window
   disabled. Wrap it in a guard type that re-enables on `Drop`. Also handle `close-requested` on the
   dialog.
3. **Tooltips are in-window.** They get clipped at the window edge until #11740 lands.
4. **Theming is split.** The native menu bar, title bar, and rfd dialogs follow the Windows theme,
   while the client area follows CAO's toggle.
5. **The help cursor needs an overlay.** The overlay must forward clicks, and hover visuals on the
   underlying widget may be lost. Check this in the #466 prototype.
6. **Custom widgets.** The custom checkable group box, tab bar, double spin box, and radio layout add
   about 150-250 lines of `.slint` that CAO owns. Structural parity is reachable, but those widgets
   are not the stock Fluent ones.

## For other tickets

- **#466 (prototype).** Build these first, because they are custom or only verified in the source:
  - the tab bar drag-reorder and disabled tabs;
  - the `DroppedFile` hook with a real Explorer drop;
  - the `set_enable` modal `Dialog` path, including the close button;
  - the help-cursor overlay on a `CheckBox`;
  - the decision between a native and a Slint-drawn menu bar.

  The probe already confirmed the `@markdown`+`@tr` tooltip pattern, checkable `MenuItem`s,
  `FilterModel`, the progress overlay, `upgrade_in_event_loop`, the `Palette` toggle, and the
  native menu's look.
- **#468 (workspace architecture).**
  - The GUI crate needs the `slint` features `unstable-winit-030` and `raw-window-handle-06`, and
    the backend must be winit (`BackendSelector::backend_name("winit")`). These features are pinned
    to Slint minor releases.
  - Keep MTA COM off the UI thread, because winit's OLE drop target needs an STA thread.
  - Don't enable rfd's `common-controls-v6` unless the app manifest declares ComCtl32 v6.
  - Run events can use `Weak::upgrade_in_event_loop`. Its `EventLoopTerminated` error is the signal
    that the UI is gone.
  - The log view wants structured severity per line, so the Rust logging stack could expose it
    instead of the GUI parsing HTML.

## Probe notes

The probe was a throwaway crate in `%TEMP%`: `slint` 1.18.1 with `unstable-winit-030` and
`raw-window-handle-06`, `slint-build` 1.18.1 with the Fluent style, and `rfd` 0.17.2. It was built
with rustc 1.99.0 on Windows 11 (build 26300) with the system set to dark mode. It contained:

- the checkable group box;
- the custom reorderable tab bar;
- a native `MenuBar` with three checkable items;
- an `@markdown`/`@tr` `Tooltip` inside a `RadioGroup`;
- `mouse-cursor: help`;
- the indeterminate progress bar with text and the status row;
- the coloured log `ListView`;
- the 75-item `FilterModel` list;
- a `Dialog` and `AboutSlint`;
- rfd folder and message dialogs parented to the window;
- the `DroppedFile` hook;
- the `Palette` toggle;
- a worker thread using `upgrade_in_event_loop`.

Everything compiled without warnings. I captured screenshots in dark mode, in light mode, and in
light mode with `SLINT_NO_MUDA`. The probe was not committed.
