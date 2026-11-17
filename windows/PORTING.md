# DiffCode for Windows — porting notes

How the Windows version is built, how it maps onto the Mac app, and where each feature stands.

## Architecture

- `crates/core` (`diffcode-core`): platform-neutral logic, ported from `Sources/Dif/Core`. Tested on any machine.
- `crates/ui` (`diffcode-ui`): platform-neutral interface — a small drawing toolkit (geometry, `Painter` trait,
  text layout handles, events, widgets) and every view and the app state (ported from `Sources/Dif/App` and `UI`).
- `crates/app` (`diffcode`, binary `DiffCode.exe`): raw Win32 window + Direct2D/DirectWrite painter on Windows; on macOS
  a headless CoreGraphics/CoreText painter that renders scripted frames to PNG, so views can be checked without a
  Windows machine. The macOS backend never ships.
- Localisation: `lang::l("English", "中文")` and `lf!` (format args), like the Mac `L(_:_:)`.
- Toolchain on the Mac: rustup at `/opt/homebrew/opt/rustup/bin` (stable, target `x86_64-pc-windows-gnu`),
  mingw-w64 linker (`.cargo/config.toml`). `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`.

## Core (Sources/Dif/Core → crates/core/src)

| Swift | Rust | Status |
|---|---|---|
| Git.swift | git.rs | done (runner, status, numstat, log, name-status, blob, ls-files, grep) |
| TextCodec.swift | text_codec.rs | done |
| Remote.swift | remote.rs | done |
| DiffEngine.swift | diff.rs | done (TextBuffer, layout, scroll mapping, unified rows, word diff, Myers, loader) |
| Syntax.swift | syntax.rs | done (all languages + PowerShell/Batch; byte spans) |
| WorkingTree.swift | working_tree.rs | done (rollback, per-hunk revert, Recycle Bin via `trash`, undo) |
| Commit.swift | commit.rs | done (whole, partial via private index, merge, undo) |
| CommitGraph.swift | commit_graph.rs | done → history.rs (graph_layout, refs) |
| History.swift | history.rs | done (blame, file/line history, compare, old_lines_for_new) |
| HistoryEdits.swift | history_edits.rs | done (cherry-pick, revert, reset+undo, merge, rebase, branches, force push) |
| Stash.swift | stash.rs | done → history_edits.rs |
| Annotations.swift | annotations.rs | done |
| Fuzzy.swift | fuzzy.rs | done |
| TextEditing.swift | text_editing.rs | done (byte columns, grapheme moves) |
| EditorSession.swift | editor_session.rs | done (session takes &mut DiffDocument; IME through WM_IME_* in the shell) |
| FileWatcher.swift | watcher.rs | done (notify, 250 ms coalescing, .git filtering) |
| Sandbox.swift | — | not needed on Windows |

Core: 67 tests (`cargo test -p diffcode-core`), clippy clean, cross-checks for `x86_64-pc-windows-gnu`.

## Interface (App + UI → crates/ui, crates/app)

Checked against `docs/GUIDE.zh-CN.md`, feature by feature, in the Mac harness (`--app out.png`, light/dark, English/中文).

| Feature | Status |
|---|---|
| Projects sidebar, file tree, path bar, locate | done |
| Local Changes (tree / flat), stats, auto refresh | done |
| Side-by-side and unified diff, word diff, overview ruler, image compare | done |
| In-place editing, autosave, Ctrl+S | done |
| Rollback / delete / per-hunk revert, undo | done |
| Commit, partial commit, amend, undo | done |
| Stash | done |
| Branches, fetch / pull / push, Smart Checkout, force push | done |
| Merge conflicts banner and resolution | done |
| Git graph popup | done |
| Cherry-pick / revert / reset | done |
| Blame, file / selection history, compare with branch / commits | done |
| Quick open, find in path, search everywhere, keymap, settings (including the fonts for code and for the terminal) | done |
| Integrated terminal (alacritty_terminal + ConPTY) | done: panes side by side, dividers, scrollback 2000, selection, copy/paste, mouse reporting, IME, bell → attention, busy check on close/quit |
| Language switch with restart | done (`--relaunch`, single instance) |
| Window placement saved | done |
| Drag files out of the window | done (file tree, Local Changes, working-tree comparison; `SHDoDragDrop`, copy only) |
| Drop highlight while dragging in | done (`IDropTarget`; over a terminal only that terminal lights up) |
| Title bar | done: no system caption; the top bar is one surface with the panels, caption buttons drawn by the interface, Windows' hit testing kept (drag, snap, double-click, Win 11 snap layouts on maximize) |
| Icon, version and manifest resources | done (`crates/app/build.rs`, `res/`) |
| Welcome screen shows the app icon | done (read from the exe's icon resource; the Mac too, from `NSApp.applicationIconImage`) |
| Installer, portable zip, CI | done (see Releases) |
| Update check | done: GitHub's latest release at launch and daily (Settings → Check for updates), offered in the sidebar, a toast and a dialog; Download opens the release page. Only a release that carries the Windows installer counts. WinINet, so Windows' proxy settings apply |
| Third-party notices | done (`installer/notices.py`, shipped as `THIRD-PARTY-NOTICES.txt` beside the program) |

Terminal on Windows: PowerShell 7 (`pwsh.exe`), else Windows PowerShell, else `%COMSPEC%`. Keys follow Windows
Terminal: Ctrl+Shift+T new, Ctrl+Shift+W close, Ctrl+C copies a selection (else interrupts), Ctrl+V / Ctrl+Shift+V
paste; Ctrl+K and other Ctrl+letters go to the shell (Ctrl+L clears there); Clear Terminal is in the menu and Find
Action. Alt+←/→ between terminals, Alt+F12 show/hide.

Title bar: `WM_NCCALCSIZE` keeps Windows' frame on the sides and bottom and lets the client area reach the top
edge (stepping in by the frame when maximized). `WM_NCHITTEST` answers from the last frame's hits: the top
frame-thickness band is the resize edge, empty top-bar space (`Ui::window_drag_area`, drawn first so every widget
over it keeps its clicks) is `HTCAPTION`, the maximize button is `HTMAXBUTTON` for the snap layouts, and its mouse
messages are fed to the interface as client events. Minimize / maximize / close run after the frame. With no
system caption there is no icon at its left, as in Windows Terminal; the taskbar and Alt+Tab use the resource icon.

Icon: `swift Scripts/make-icon.swift --windows windows/crates/app/res/DiffCode.ico` draws the Mac icon with the
tile filling the canvas (Windows icons have no grid margin or shadow), PNG images 16–256 px. `build.rs` compiles
icon, version (from the workspace version) and manifest (PerMonitorV2, long paths, common controls 6) with
`embed-resource` (rc.exe under MSVC, `x86_64-w64-mingw32-windres` when cross-building).

Editor keys follow the IDE's Windows keymap, as the keymap popup says: Delete Line Ctrl+Y, Redo Ctrl+Shift+Z.
Pull has no shortcut, as on the Mac. The program in the foreground comes from a Toolhelp snapshot of the
shell's descendants.

Every feature is checked in the Mac harness first, then on Windows with a build from CI.

Checks: `cargo test --workspace`, `cargo clippy --workspace`, `cargo clippy -p diffcode --target x86_64-pc-windows-gnu`.

## Releases

`.github/workflows/windows.yml` runs on `windows-2025` for every push to main and pull request touching `windows/`:
tests, `cargo build --release` (MSVC, C runtime linked in, so no Visual C++ Redistributable is needed), then
`installer/DiffCode.iss` with the Inno Setup that GitHub's image includes. The workflow artifact holds:

- `DiffCode-Setup-x64.exe` — installs for the current user without an administrator prompt
  (`%LOCALAPPDATA%\Programs\DiffCode`, Start menu, optional desktop shortcut, uninstaller; `/ALLUSERS` for everyone).
  English and Simplified Chinese, light or dark with Windows.
- `DiffCode-windows-x64.zip` — the same program to unzip and run.

Both carry `LICENSE.txt` and `THIRD-PARTY-NOTICES.txt` (every crate compiled in, with its license text).
- `SHA256SUMS-windows.txt`.

Publishing a GitHub release attaches them to it; so does running the workflow by hand with a tag. A release is one
tag for both platforms: bump `release/version.env`, publish the Mac build as `release/macos/README.md` describes,
then `gh release create v<version> dist/macos/… --notes-file release/github/<version>.md`; the Windows assets
follow a few minutes later, and only then does the update check offer the release to Windows users. The version is
`MARKETING_VERSION` in `release/version.env`, which `windows/Cargo.toml` must match (the workflow checks), and the
executable's fourth version number is `BUILD_NUMBER`. The file names carry no version, so
`releases/latest/download/DiffCode-Setup-x64.exe` always fetches the newest, like `DiffCode.pkg` for the Mac.

The executables are not code-signed, as with most open-source Windows software: a certificate costs money every
year. Windows SmartScreen therefore warns on the first run of a downloaded installer ("Windows protected your PC"
→ More info → Run anyway) until the download has built up reputation. Users who would rather not can compare the
SHA-256 sums or build from source. Free signing for open-source projects exists (SignPath Foundation, applied for
by the maintainer and run from CI) should it become worth it; winget and Scoop manifests can follow a release.
