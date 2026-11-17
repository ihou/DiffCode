//! What the interface asks of the operating system, and what the shell runs. The Windows shell implements these
//! with Win32; the Mac harness with stand-ins.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::text_field::Clipboard;
use crate::theme::Theme;
use crate::ui::Ui;

/// What the window's own caption buttons ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    Minimize,
    ToggleMaximize,
    Close,
}

/// Fetches a URL's body as text, from any thread (an HTTPS GET through the system's proxy settings).
pub type HttpGet = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// Wakes the interface from another thread: the shell runs a frame soon after, on its own thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub trait Platform {
    fn clipboard(&mut self) -> &mut dyn Clipboard;
    /// The system's folder picker; `None` when cancelled.
    fn pick_folder(&mut self, title: &str) -> Option<PathBuf>;
    fn open_url(&mut self, url: &str);
    /// Shows the file selected in the file manager.
    fn reveal(&mut self, path: &Path);
    fn waker(&self) -> Waker;
    fn set_title(&mut self, title: &str);
    /// Ends the program after this frame.
    fn quit(&mut self);
    /// What the process holds in memory right now, in megabytes (its working set).
    fn memory_mb(&self) -> f64 {
        0.0
    }
    /// The program a terminal's shell (process `shell`) is running in the foreground, by name; None when the shell
    /// itself is waiting for a command.
    fn foreground_process(&self, shell: u32) -> Option<String> {
        let _ = shell;
        None
    }
    /// Draws the user's eye to the window when it is behind others (the taskbar button flashes).
    fn request_attention(&mut self) {}
    /// The window has no system title bar: the interface draws the caption buttons in its top bar and marks where
    /// the window is dragged from.
    fn caption_buttons(&self) -> bool {
        false
    }
    fn is_maximized(&self) -> bool {
        false
    }
    fn window_command(&mut self, command: WindowCommand) {
        let _ = command;
    }
    /// Fetching from the web, for the update check; None where DiffCode does not look for updates itself.
    fn http_get(&self) -> Option<HttpGet> {
        None
    }
    /// The app's icon as PNG, the size nearest `pixels` (the smallest at least that big, when there is one).
    fn app_icon(&self, pixels: u32) -> Option<Arc<Vec<u8>>> {
        let _ = pixels;
        None
    }
    /// The default monospace face, and the installed families whose characters all share one width (the code view
    /// and the terminal put characters in columns), sorted, for the font settings.
    fn monospace_families(&self) -> (String, Vec<String>) {
        (String::new(), Vec::new())
    }
    /// Starts dragging files out of the window (a copy, never a move), while the button that started it is held.
    fn drag_files(&mut self, paths: Vec<PathBuf>) {
        let _ = paths;
    }
}

/// The whole interface, as the shell drives it.
pub trait Program {
    /// The look for the next frame. `system_dark`: the system's app mode.
    fn theme(&mut self, system_dark: bool) -> Theme;
    /// Draws one frame and handles the event in it.
    fn frame(&mut self, ui: &mut Ui, platform: &mut dyn Platform);
    /// The window is about to close; `false` keeps it open (and the program shows why).
    fn close_requested(&mut self, platform: &mut dyn Platform) -> bool {
        let _ = platform;
        true
    }
    /// Where the window was last time: x, y, width, height in pixels, and whether it was maximised.
    fn window_placement(&self) -> Option<(i32, i32, i32, i32, bool)> {
        None
    }
    /// The window is closing at this place; it opens there next time.
    fn save_window_placement(&mut self, placement: (i32, i32, i32, i32, bool)) {
        let _ = placement;
    }
}
