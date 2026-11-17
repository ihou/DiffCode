//! The Windows shell: one top-level Win32 window, its messages turned into `Event`s, and a frame drawn with
//! Direct2D for each. Per-monitor DPI aware; input methods through IMM32; files dropped on the window open.
//!
//! The window has no system caption: the client area reaches the top edge, the interface draws the caption
//! buttons in its top bar, and hit testing gives Windows the caption (to drag, snap and double-click), the top
//! resize edge and the maximize button (for the Windows 11 snap layouts) back from what the last frame drew.

mod render;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use diffcode_ui::geom::{Point, Rect, Size};
use diffcode_ui::input::{Event, Key, Modifiers, MouseButton};
use diffcode_ui::paint::CursorIcon;
use diffcode_ui::platform::{HttpGet, Platform, Program, Waker, WindowCommand};
use diffcode_ui::text_field::Clipboard;
use diffcode_ui::ui::{maximize_button_id, window_drag_id, Ui, UiState};
use render::{DxBackend, DxPainter};
use windows::core::{implement, w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MonitorFromWindow, ScreenToClient, ValidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::Networking::WinInet::{
    HttpQueryInfoW, InternetCloseHandle, InternetOpenUrlW, InternetOpenW, InternetReadFile, InternetSetOptionW,
    HTTP_QUERY_FLAG_NUMBER, HTTP_QUERY_STATUS_CODE, INTERNET_FLAG_NO_CACHE_WRITE, INTERNET_FLAG_NO_COOKIES,
    INTERNET_FLAG_NO_UI, INTERNET_FLAG_RELOAD, INTERNET_OPEN_TYPE_PRECONFIG, INTERNET_OPTION_CONNECT_TIMEOUT,
    INTERNET_OPTION_RECEIVE_TIMEOUT, INTERNET_OPTION_SEND_TIMEOUT,
};
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::{FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource};
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::{
    IDropSource, IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP,
    CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, WaitForSingleObject};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_WAKE: u32 = WM_APP + 1;
/// From the common controls headers; the same value everywhere.
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_REPAINT: usize = 1;

/// Holds the one-instance lock while DiffCode runs.
pub struct Instance(#[allow(dead_code)] HANDLE);

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Takes the one-instance lock, or brings the DiffCode already running forward and returns `None`.
/// `relaunch`: started by a DiffCode that is closing to restart; wait for it to go instead.
pub fn single_instance(relaunch: bool) -> Option<Instance> {
    unsafe {
        let handle = CreateMutexW(None, false, w!("Local\\DiffCode.Instance")).ok()?;
        // Taken at once, or within a few seconds of a restarting copy closing.
        let wait = if relaunch { 15_000 } else { 0 };
        let r = WaitForSingleObject(handle, wait);
        if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
            return Some(Instance(handle));
        }
        let _ = CloseHandle(handle);
        if let Ok(other) = FindWindowW(w!("DiffCodeWindow"), PCWSTR::null()) {
            if IsIconic(other).as_bool() {
                let _ = ShowWindow(other, SW_RESTORE);
            }
            let _ = SetForegroundWindow(other);
        }
        None
    }
}

/// Whether Windows shows its own interface in Chinese.
pub fn system_chinese() -> bool {
    const LANG_CHINESE: u16 = 0x04;
    unsafe { GetUserDefaultUILanguage() & 0x3FF == LANG_CHINESE }
}

/// Runs the program in a window until it closes.
pub fn run(program: Box<dyn Program>) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // OLE, not just COM: dragging files in and out needs it.
        let _ = OleInitialize(None);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("DiffCodeWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as _)).unwrap_or_default(),
            hIconSm: load_icon(instance.into(), SM_CXSMICON, GetDpiForSystem()).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let dpi = GetDpiForSystem().max(96);
        let scale = dpi as f32 / 96.0;
        let backend = match DxBackend::new() {
            Ok(b) => Rc::new(b),
            Err(e) => {
                let text = HSTRING::from(format!("DiffCode could not start Direct2D: {e}"));
                MessageBoxW(None, &text, w!("DiffCode"), MB_ICONERROR);
                return;
            }
        };
        let window = Box::new(Window {
            hwnd: HWND::default(),
            program,
            st: UiState::default(),
            painter: None,
            backend,
            platform: None,
            dark: system_dark(),
            dpi,
            busy: Cell::new(false),
            last_click: None,
            surrogate: None,
            ime_area: None,
            ime_enabled: true,
            focus: None,
            tracking: false,
            tracking_nc: false,
            closing: false,
        });
        let placement = window.program.window_placement();
        let raw = Box::into_raw(window);
        let style = WS_OVERLAPPEDWINDOW;
        let mut rect = RECT { left: 0, top: 0, right: (1280.0 * scale) as i32, bottom: (820.0 * scale) as i32 };
        let _ = AdjustWindowRectExForDpi(&mut rect, style, false, WINDOW_EX_STYLE(0), dpi);
        // No caption or top frame: the client area starts at the window's top edge.
        rect.top = 0;
        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class,
            w!("DiffCode"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(instance.into()),
            Some(raw as *const _),
        );
        let Ok(hwnd) = hwnd else {
            drop(Box::from_raw(raw));
            return;
        };
        let mut show = SW_SHOWDEFAULT;
        if let Some((x, y, w, h, maximized)) = placement.filter(|p| p.2 > 0 && p.3 > 0) {
            // SetWindowPlacement keeps a window whose monitor is gone on one that is still there.
            let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            if GetWindowPlacement(hwnd, &mut wp).is_ok() {
                wp.rcNormalPosition = RECT { left: x, top: y, right: x + w, bottom: y + h };
                wp.showCmd = SW_HIDE.0 as u32;
                let _ = SetWindowPlacement(hwnd, &wp);
            }
            show = if maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL };
        }
        // Files dragged in light up where they would land; WS_EX_ACCEPTFILES stays as the fallback.
        let target: IDropTarget = DropTarget { hwnd, accepts: Cell::new(false) }.into();
        let _ = RegisterDragDrop(hwnd, &target);
        set_icons(hwnd, GetDpiForWindow(hwnd).max(96));
        // The frame without a caption from the start (WM_NCCALCSIZE below).
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, show);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        drop(Box::from_raw(raw));
    }
}

struct Window {
    hwnd: HWND,
    program: Box<dyn Program>,
    st: UiState,
    painter: Option<DxPainter>,
    backend: Rc<DxBackend>,
    platform: Option<WinPlatform>,
    dark: bool,
    dpi: u32,
    /// Inside a frame: messages that arrive from a nested loop (a modal dialog) must not touch the state.
    busy: Cell<bool>,
    last_click: Option<(Instant, POINT, MouseButton, u8)>,
    surrogate: Option<u16>,
    ime_area: Option<Rect>,
    ime_enabled: bool,
    /// The widget with the keyboard after the last frame: a composition does not follow the focus elsewhere.
    focus: Option<diffcode_ui::ui::Id>,
    tracking: bool,
    /// Watching for the mouse to leave the maximize button, which Windows treats as part of the frame.
    tracking_nc: bool,
    closing: bool,
}

impl Window {
    fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    fn client_pixels(&self) -> (u32, u32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
    }

    fn dip(&self, lparam: LPARAM) -> Point {
        let x = (lparam.0 & 0xFFFF) as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
        Point::new(x / self.scale(), y / self.scale())
    }

    /// Runs one frame for `event` (or none), drawing it unless more input is already waiting.
    fn frame(&mut self, event: Option<Event>) {
        if let Some(e) = &event {
            self.st.observe(e);
        }
        let pixels = self.client_pixels();
        let scale = self.scale();
        let size = Size::new(pixels.0 as f32 / scale, pixels.1 as f32 / scale);
        let theme = self.program.theme(self.dark);
        // While input is queued, intermediate frames handle their event without drawing.
        let behind = event.is_some() && unsafe { GetQueueStatus(QS_INPUT) } >> 16 != 0;
        let (Some(painter), Some(platform)) = (self.painter.as_mut(), self.platform.as_mut()) else { return };
        let drawing = !behind && pixels.0 > 0 && pixels.1 > 0 && painter.begin(pixels, self.dpi as f32, theme.window);
        self.busy.set(true);
        {
            let mut ui = Ui::begin(&mut self.st, painter, &*self.backend, &theme, event, size);
            self.program.frame(&mut ui, platform);
            ui.finish();
        }
        self.busy.set(false);
        if drawing {
            painter.end();
        } else {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
        self.after_frame();
    }

    fn after_frame(&mut self) {
        if let Some(paths) = self.platform.as_mut().and_then(|p| p.pending_drag.take()) {
            self.drag_out(&paths);
        }
        if let Some(command) = self.platform.as_mut().and_then(|p| p.pending_command.take()) {
            unsafe {
                match command {
                    WindowCommand::Minimize => {
                        let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                    }
                    WindowCommand::ToggleMaximize => {
                        let _ = ShowWindow(self.hwnd, if IsZoomed(self.hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                    }
                    WindowCommand::Close => {
                        let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
            }
        }
        unsafe {
            if self.st.mouse_inside {
                SetCursor(Some(cursor(self.st.cursor)));
            }
            if let Some(d) = self.st.repaint_after {
                SetTimer(Some(self.hwnd), TIMER_REPAINT, d.as_millis().clamp(1, 60_000) as u32, None);
            }
            if self.st.repaint_now {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            // Input methods: only while a text field has the caret, with their windows by it.
            if self.st.ime_reset || self.st.focus != self.focus {
                self.focus = self.st.focus;
                let himc = ImmGetContext(self.hwnd);
                if !himc.is_invalid() {
                    let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
                    let _ = ImmReleaseContext(self.hwnd, himc);
                }
            }
            let area = self.st.ime_area;
            if area.is_some() != self.ime_enabled {
                self.ime_enabled = area.is_some();
                let _ = ImmAssociateContextEx(self.hwnd, HIMC::default(), if self.ime_enabled { IACE_DEFAULT } else { 0 });
            }
            if area != self.ime_area {
                self.ime_area = area;
                if let Some(a) = area {
                    self.place_ime(a);
                }
            }
            if self.platform.as_ref().is_some_and(|p| p.quit) && !self.closing {
                self.closing = true;
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    /// Drags files out of the window as the shell would from a folder, so every target knows what it gets. The
    /// shell's loop runs until the drop; the window keeps still meanwhile, and takes no drop from itself.
    fn drag_out(&mut self, paths: &[PathBuf]) {
        unsafe {
            let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
            for path in paths {
                let mut pidl = std::ptr::null_mut();
                if SHParseDisplayName(&HSTRING::from(path.as_path()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
                    pidls.push(pidl);
                }
            }
            if !pidls.is_empty() {
                let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const ITEMIDLIST).collect();
                if let Ok(data) = SHCreateDataObject::<_, IDataObject>(None, Some(&list), None::<&IDataObject>) {
                    self.busy.set(true);
                    let _ = SHDoDragDrop(Some(self.hwnd), &data, None::<&IDropSource>, DROPEFFECT_COPY);
                    self.busy.set(false);
                }
            }
            for pidl in pidls {
                ILFree(Some(pidl as *const ITEMIDLIST));
            }
            // The button went up inside the shell's loop, where the window never saw it.
            let mut at = POINT::default();
            let _ = GetCursorPos(&mut at);
            let _ = ScreenToClient(self.hwnd, &mut at);
            let s = self.scale();
            self.frame(Some(Event::MouseUp { pos: Point::new(-10_000.0, -10_000.0), button: MouseButton::Left, mods: Modifiers::NONE }));
            self.frame(Some(Event::MouseMove(Point::new(at.x as f32 / s, at.y as f32 / s))));
        }
    }

    fn frame_thickness(&self) -> i32 {
        frame_thickness(self.dpi)
    }

    /// The client point under a screen point, in DIPs.
    fn client_dip(&self, x: i32, y: i32) -> Point {
        let mut p = POINT { x, y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        Point::new(p.x as f32 / self.scale(), p.y as f32 / self.scale())
    }

    /// What part of the window a screen point is, for the caption-less frame: the top resize edge, the empty top
    /// bar as the caption, the maximize button as itself, everything else the interface's.
    fn hit_test(&self, x: i32, y: i32) -> u32 {
        let mut p = POINT { x, y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
            if !IsZoomed(self.hwnd).as_bool() && p.y >= 0 && p.y < self.frame_thickness() {
                return HTTOP;
            }
        }
        let at = Point::new(p.x as f32 / self.scale(), p.y as f32 / self.scale());
        match self.st.hit_at(at) {
            Some(id) if id == window_drag_id() => HTCAPTION,
            Some(id) if id == maximize_button_id() => HTMAXBUTTON,
            _ => HTCLIENT,
        }
    }

    fn track_leave(&mut self, nonclient: bool) {
        let tracking = if nonclient { self.tracking_nc } else { self.tracking };
        if tracking {
            return;
        }
        let mut tme = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if nonclient { TME_LEAVE | TME_NONCLIENT } else { TME_LEAVE },
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        unsafe {
            let _ = TrackMouseEvent(&mut tme);
        }
        if nonclient {
            self.tracking_nc = true;
        } else {
            self.tracking = true;
        }
    }

    /// The mouse left the client area or the maximize button: unless it went from one to the other, the interface
    /// hears it has gone.
    fn mouse_left(&mut self) {
        let mut at = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut at);
            if WindowFromPoint(at) == self.hwnd {
                let hit = self.hit_test(at.x, at.y);
                if hit == HTCLIENT || hit == HTMAXBUTTON {
                    return;
                }
            }
        }
        self.frame(Some(Event::MouseLeave));
    }

    fn place_ime(&self, a: Rect) {
        let s = self.scale();
        let px = |v: f32| (v * s).round() as i32;
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_invalid() {
                return;
            }
            let at = POINT { x: px(a.x), y: px(a.bottom()) };
            let comp = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: at, rcArea: RECT::default() };
            let _ = ImmSetCompositionWindow(himc, &comp);
            let cand = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: at,
                rcArea: RECT { left: px(a.x), top: px(a.y), right: px(a.right()), bottom: px(a.bottom()) },
            };
            let _ = ImmSetCandidateWindow(himc, &cand);
            let _ = ImmReleaseContext(self.hwnd, himc);
        }
    }

    fn mouse_down(&mut self, button: MouseButton, lparam: LPARAM) {
        let p = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
        let now = Instant::now();
        let (limit, slop) = unsafe { (GetDoubleClickTime(), GetSystemMetrics(SM_CXDOUBLECLK).max(4) / 2) };
        let clicks = match self.last_click {
            Some((t, q, b, n))
                if b == button
                    && now.duration_since(t).as_millis() <= limit as u128
                    && (p.x - q.x).abs() <= slop
                    && (p.y - q.y).abs() <= slop =>
            {
                if n >= 3 {
                    1
                } else {
                    n + 1
                }
            }
            _ => 1,
        };
        self.last_click = Some((now, p, button, clicks));
        unsafe {
            SetCapture(self.hwnd);
        }
        let pos = self.dip(lparam);
        self.frame(Some(Event::MouseDown { pos, button, clicks, mods: modifiers() }));
    }

    fn mouse_up(&mut self, button: MouseButton, lparam: LPARAM) {
        let pos = self.dip(lparam);
        self.frame(Some(Event::MouseUp { pos, button, mods: modifiers() }));
        if !self.st.buttons.iter().any(|b| *b) {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
    }

    fn wheel(&mut self, wparam: WPARAM, lparam: LPARAM, horizontal: bool) {
        let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
        let mut lines = 3u32;
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        let step = 19.0 * lines.clamp(1, 20) as f32;
        let mut p = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        let pos = Point::new(p.x as f32 / self.scale(), p.y as f32 / self.scale());
        let mods = modifiers();
        let d = if horizontal {
            Point::new(delta * step, 0.0)
        } else if mods.shift {
            Point::new(-delta * step, 0.0)
        } else {
            Point::new(0.0, -delta * step)
        };
        self.frame(Some(Event::Wheel { pos, delta: d, mods }));
    }

    fn composition(&mut self, lparam: LPARAM) {
        let flags = lparam.0 as u32;
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_invalid() {
                return;
            }
            let read = |kind: IME_COMPOSITION_STRING| -> String {
                let bytes = ImmGetCompositionStringW(himc, kind, None, 0);
                if bytes <= 0 {
                    return String::new();
                }
                let mut buf = vec![0u16; bytes as usize / 2];
                ImmGetCompositionStringW(himc, kind, Some(buf.as_mut_ptr() as *mut _), bytes as u32);
                String::from_utf16_lossy(&buf)
            };
            let result = (flags & GCS_RESULTSTR.0 != 0).then(|| read(GCS_RESULTSTR));
            let comp = (flags & GCS_COMPSTR.0 != 0).then(|| {
                let text = read(GCS_COMPSTR);
                let caret = (flags & GCS_CURSORPOS.0 != 0).then(|| {
                    let c16 = ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0).max(0) as usize;
                    // UTF-16 caret → byte offset.
                    let mut units = 0;
                    let mut byte = text.len();
                    for (i, ch) in text.char_indices() {
                        if units >= c16 {
                            byte = i;
                            break;
                        }
                        units += ch.len_utf16();
                    }
                    byte
                });
                (text, caret)
            });
            let _ = ImmReleaseContext(self.hwnd, himc);
            if let Some(r) = result {
                if !r.is_empty() {
                    self.frame(Some(Event::Preedit { text: String::new(), caret: None }));
                    self.frame(Some(Event::Text(r)));
                }
            }
            match comp {
                Some((text, caret)) => self.frame(Some(Event::Preedit { text, caret })),
                None if flags & GCS_RESULTSTR.0 != 0 => self.frame(Some(Event::Preedit { text: String::new(), caret: None })),
                None => {}
            }
        }
    }
}

fn modifiers() -> Modifiers {
    unsafe {
        Modifiers {
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
            alt: GetKeyState(VK_MENU.0 as i32) < 0,
        }
    }
}

fn key_of(vk: u32) -> Option<Key> {
    let vk16 = VIRTUAL_KEY(vk as u16);
    Some(match vk16 {
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_TAB => Key::Tab,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_INSERT => Key::Insert,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_SPACE => Key::Space,
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => Key::Shift,
        VK_ADD => Key::Char('+'),
        VK_SUBTRACT => Key::Char('-'),
        _ if (VK_F1.0..=VK_F24.0).contains(&vk16.0) => Key::F((vk16.0 - VK_F1.0 + 1) as u8),
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk16.0) => Key::Char((b'0' + (vk16.0 - VK_NUMPAD0.0) as u8) as char),
        _ if (b'A' as u32..=b'Z' as u32).contains(&vk) => Key::Char((vk as u8 + 32) as char),
        _ if (b'0' as u32..=b'9' as u32).contains(&vk) => Key::Char(vk as u8 as char),
        _ => {
            // Punctuation, by what the key types on the current layout.
            let ch = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) } & 0x7FFF;
            match char::from_u32(ch) {
                Some(c) if c.is_ascii_graphic() => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

fn cursor(icon: CursorIcon) -> HCURSOR {
    let id = match icon {
        CursorIcon::Arrow => IDC_ARROW,
        CursorIcon::Hand => IDC_HAND,
        CursorIcon::Text => IDC_IBEAM,
        CursorIcon::ResizeHorizontal => IDC_SIZEWE,
        CursorIcon::ResizeVertical => IDC_SIZENS,
        CursorIcon::Busy => IDC_WAIT,
    };
    unsafe { LoadCursorW(None, id).unwrap_or_default() }
}

/// Whether apps should be dark, from the personalisation setting.
fn system_dark() -> bool {
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && value == 0
}

fn set_dark_title_bar(hwnd: HWND, dark: bool) {
    let value = BOOL::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &value as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
    }
}

/// Thickness of the resize frame Windows would draw, in pixels: the caption-less top keeps one this tall.
fn frame_thickness(dpi: u32) -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

/// WM_NCCALCSIZE: Windows' frame on the left, right and bottom; on top, none, so the client area takes the
/// caption's place. Maximized, the window hangs over the screen's edges by the frame, so the top steps in by it.
unsafe fn client_area(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // The proposed window rect comes in and the client rect goes out in the same place, either way.
        let rect: *mut RECT = if wparam.0 != 0 { (lparam.0 as *mut NCCALCSIZE_PARAMS).cast() } else { lparam.0 as *mut RECT };
        let top = (*rect).top;
        let r = DefWindowProcW(hwnd, WM_NCCALCSIZE, wparam, lparam);
        if r.0 != 0 {
            return r;
        }
        let rect = &mut *rect;
        rect.top = top;
        if IsZoomed(hwnd).as_bool() {
            rect.top += frame_thickness(GetDpiForWindow(hwnd).max(96));
            // An auto-hiding taskbar comes back when the mouse reaches its edge of the screen, which a maximized
            // window that covers that edge would keep from happening: leave it a sliver.
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
                for edge in [ABE_LEFT, ABE_TOP, ABE_RIGHT, ABE_BOTTOM] {
                    let mut bar = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, uEdge: edge, rc: mi.rcMonitor, ..Default::default() };
                    if SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &mut bar) != 0 {
                        match edge {
                            ABE_LEFT => rect.left += 2,
                            ABE_TOP => rect.top += 2,
                            ABE_RIGHT => rect.right -= 2,
                            _ => rect.bottom -= 2,
                        }
                    }
                }
            }
        }
        LRESULT(0)
    }
}

/// The app's icon (resource 1) at the size Windows uses for `metric` at `dpi`.
fn load_icon(instance: HINSTANCE, metric: SYSTEM_METRICS_INDEX, dpi: u32) -> Option<HICON> {
    unsafe {
        let size = GetSystemMetricsForDpi(metric, dpi);
        let h = LoadImageW(Some(instance), PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR).ok()?;
        Some(HICON(h.0))
    }
}

/// The window's icons for the taskbar and Alt+Tab, drawn for the monitor it is on.
fn set_icons(hwnd: HWND, dpi: u32) {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        for (which, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
            if let Some(icon) = load_icon(instance, metric, dpi) {
                let old = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(which as usize)), Some(LPARAM(icon.0 as isize)));
                if old.0 != 0 {
                    let _ = DestroyIcon(HICON(old.0 as *mut _));
                }
            }
        }
    }
}

/// An HTTPS GET with WinINet, which goes through the proxy set in Windows' Internet settings, as a browser would.
fn web_get(url: &str) -> Result<String, String> {
    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = InternetCloseHandle(self.0);
                }
            }
        }
    }
    let failed = || {
        use diffcode_ui::core::lang::l;
        // WinINet's own codes (wininet.h); the system has no text for them outside wininet.dll.
        match unsafe { GetLastError() }.0 {
            12002 => l("the connection timed out", "连接超时"),
            12007 => l("the server could not be found", "找不到服务器"),
            12029 | 12030 => l("could not connect to the server", "无法连接到服务器"),
            12175 | 12157 | 12045 | 12038 | 12037 => l("the secure connection failed", "安全连接失败"),
            code => l(&format!("network error {code}"), &format!("网络错误 {code}")),
        }
    };
    unsafe {
        let agent = HSTRING::from(format!("DiffCode/{}", env!("CARGO_PKG_VERSION")));
        let session = Handle(InternetOpenW(&agent, INTERNET_OPEN_TYPE_PRECONFIG.0, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(failed());
        }
        let timeout: u32 = 20_000;
        for option in [INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_SEND_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT] {
            let _ = InternetSetOptionW(Some(session.0), option, Some(&timeout as *const u32 as *const _), 4);
        }
        let headers: Vec<u16> = "Accept: application/vnd.github+json\r\n".encode_utf16().collect();
        let flags = INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_COOKIES | INTERNET_FLAG_NO_UI;
        let request = Handle(InternetOpenUrlW(session.0, &HSTRING::from(url), Some(&headers), flags, None));
        if request.0.is_null() {
            return Err(failed());
        }
        let mut status = 0u32;
        let mut len = 4u32;
        let _ = HttpQueryInfoW(request.0, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, Some(&mut status as *mut u32 as *mut _), &mut len, None);
        let mut body = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            if InternetReadFile(request.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read).is_err() {
                return Err(failed());
            }
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > 4 << 20 {
                return Err("reply too large".into());
            }
        }
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        String::from_utf8(body).map_err(|_| "reply is not text".into())
    }
}

/// Bytes of one of the executable's own resources.
fn resource(name: PCWSTR, kind: PCWSTR) -> Option<&'static [u8]> {
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let info = FindResourceW(Some(module), name, kind);
        if info.is_invalid() {
            return None;
        }
        let data = LoadResource(Some(module), info).ok()?;
        let p = LockResource(data) as *const u8;
        let len = SizeofResource(Some(module), info) as usize;
        (!p.is_null() && len > 0).then(|| std::slice::from_raw_parts(p, len))
    }
}

/// The PNG image in the app's icon best for `pixels`, read from the executable's resources (where an icon file's
/// directory becomes a group of 14-byte entries naming each image's own resource).
fn icon_png(pixels: u32) -> Option<&'static [u8]> {
    let group = resource(PCWSTR(1 as _), RT_GROUP_ICON)?;
    let count = u16::from_le_bytes([*group.get(4)?, *group.get(5)?]) as usize;
    let entry = |i: usize| group.get(6 + 14 * i..6 + 14 * (i + 1));
    let sizes: Vec<u32> = (0..count).map(|i| entry(i).map_or(0, |e| if e[0] == 0 { 256 } else { e[0] as u32 })).collect();
    let e = entry(diffcode_ui::paint::icon_size_for(&sizes, pixels)?)?;
    let id = u16::from_le_bytes([e[12], e[13]]);
    let png = resource(PCWSTR(id as usize as _), RT_ICON)?;
    png.starts_with(b"\x89PNG").then_some(png)
}

fn dropped_files(hdrop: HDROP) -> Vec<PathBuf> {
    let out = files_of(hdrop);
    unsafe { DragFinish(hdrop) };
    out
}

fn files_of(hdrop: HDROP) -> Vec<PathBuf> {
    let mut out = Vec::new();
    unsafe {
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            out.push(PathBuf::from(String::from_utf16_lossy(&buf[..n])));
        }
    }
    out
}

// MARK: - Files dragged in

fn window_of(hwnd: HWND) -> Option<&'static mut Window> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Window;
        (!ptr.is_null() && !(*ptr).busy.get()).then(|| &mut *ptr)
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

/// The window as a drop target: tells the interface where files are hovering, so it can light up where they would
/// land, then hands them over.
#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
    /// What is being dragged includes files.
    accepts: Cell<bool>,
}

impl DropTarget {
    /// The window, unless it is inside a frame (a drag that started in it, which it does not take back). It lives
    /// as long as the message loop that calls here.
    fn window(&self) -> Option<&'static mut Window> {
        window_of(self.hwnd)
    }

    fn client_point(&self, pt: &POINTL, scale: f32) -> Point {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        Point::new(p.x as f32 / scale, p.y as f32 / scale)
    }

    fn over(&self, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let window = if self.accepts.get() { self.window() } else { None };
        unsafe {
            *effect = if window.is_some() { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
        if let Some(w) = window {
            let at = self.client_point(pt, w.scale());
            w.frame(Some(Event::FilesHover(Some(at))));
        }
        Ok(())
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: windows::core::Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let accepts = data.ok().is_ok_and(|d| unsafe { d.QueryGetData(&hdrop_format()) } == S_OK);
        self.accepts.set(accepts);
        self.over(pt, effect)
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        self.over(pt, effect)
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if let Some(w) = self.window() {
            w.frame(Some(Event::FilesHover(None)));
        }
        Ok(())
    }

    fn Drop(&self, data: windows::core::Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let window = self.window();
        let files = match (data.ok(), &window) {
            (Ok(d), Some(_)) => unsafe {
                match d.GetData(&hdrop_format()) {
                    Ok(mut medium) => {
                        let files = files_of(HDROP(medium.u.hGlobal.0));
                        ReleaseStgMedium(&mut medium);
                        files
                    }
                    Err(_) => Vec::new(),
                }
            },
            _ => Vec::new(),
        };
        unsafe {
            *effect = if files.is_empty() { DROPEFFECT_NONE } else { DROPEFFECT_COPY };
        }
        if let Some(w) = window {
            let at = self.client_point(pt, w.scale());
            w.frame(Some(Event::FilesHover(None)));
            if !files.is_empty() {
                unsafe {
                    let _ = SetForegroundWindow(self.hwnd);
                }
                w.frame(Some(Event::MouseMove(at)));
                w.frame(Some(Event::Files(files)));
            }
        }
        Ok(())
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            let w = cs.lpCreateParams as *mut Window;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, w as isize);
            (*w).hwnd = hwnd;
            (*w).dpi = GetDpiForWindow(hwnd).max(96);
            let waker_flag = Arc::new(AtomicBool::new(false));
            (*w).platform = Some(WinPlatform { hwnd, quit: false, clipboard: WinClipboard { hwnd }, wake: waker_flag, pending_drag: None, pending_command: None });
            (*w).painter = Some(DxPainter::new((*w).backend.clone(), hwnd));
            set_dark_title_bar(hwnd, (*w).dark);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Window;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        // A nested message loop inside a frame: keep the window alive without touching the state.
        if (*ptr).busy.get() {
            return match msg {
                WM_PAINT => {
                    let _ = ValidateRect(Some(hwnd), None);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_NCCALCSIZE => client_area(hwnd, wparam, lparam),
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            };
        }
        let w = &mut *ptr;
        match msg {
            WM_PAINT => {
                w.frame(None);
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                w.frame(None);
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                w.dpi = ((wparam.0 & 0xFFFF) as u32).max(96);
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                set_icons(hwnd, w.dpi);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                let dark = system_dark();
                if dark != w.dark {
                    w.dark = dark;
                    set_dark_title_bar(hwnd, dark);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_TIMER if wparam.0 == TIMER_REPAINT => {
                let _ = KillTimer(Some(hwnd), TIMER_REPAINT);
                w.frame(None);
                LRESULT(0)
            }
            WM_WAKE => {
                if let Some(p) = &w.platform {
                    p.wake.store(false, Ordering::SeqCst);
                }
                w.frame(None);
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    SetCursor(Some(cursor(w.st.cursor)));
                    LRESULT(1)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_MOUSEMOVE => {
                w.track_leave(false);
                let pos = w.dip(lparam);
                w.st.observe(&Event::MouseMove(pos));
                // Drags follow the mouse at once; hovering waits for the next paint.
                if w.st.buttons.iter().any(|b| *b) {
                    w.frame(None);
                } else {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                w.tracking = false;
                w.mouse_left();
                LRESULT(0)
            }
            WM_NCMOUSELEAVE => {
                w.tracking_nc = false;
                w.mouse_left();
                LRESULT(0)
            }
            WM_NCCALCSIZE => client_area(hwnd, wparam, lparam),
            WM_NCHITTEST => {
                // The frame Windows still has (sides, bottom, corners) is its own.
                let r = DefWindowProcW(hwnd, msg, wparam, lparam);
                if r.0 as u32 != HTCLIENT {
                    return r;
                }
                let (x, y) = ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
                LRESULT(w.hit_test(x, y) as isize)
            }
            // The maximize button belongs to the frame as far as Windows knows, so it hears the mouse there through
            // these; handing them on would have Windows draw its own button over the interface's.
            WM_NCMOUSEMOVE => {
                let (x, y) = ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
                if wparam.0 as u32 == HTMAXBUTTON {
                    w.track_leave(true);
                    let pos = w.client_dip(x, y);
                    w.frame(Some(Event::MouseMove(pos)));
                } else if w.st.mouse_inside {
                    w.frame(Some(Event::MouseLeave));
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wparam.0 as u32 == HTMAXBUTTON => {
                let (x, y) = ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
                let pos = w.client_dip(x, y);
                w.frame(Some(Event::MouseDown { pos, button: MouseButton::Left, clicks: 1, mods: modifiers() }));
                LRESULT(0)
            }
            WM_NCLBUTTONUP if w.st.buttons[0] => {
                let (x, y) = ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
                let pos = w.client_dip(x, y);
                w.frame(Some(Event::MouseUp { pos, button: MouseButton::Left, mods: modifiers() }));
                if wparam.0 as u32 == HTMAXBUTTON {
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_LBUTTONDOWN => {
                w.mouse_down(MouseButton::Left, lparam);
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                w.mouse_down(MouseButton::Right, lparam);
                LRESULT(0)
            }
            WM_MBUTTONDOWN => {
                w.mouse_down(MouseButton::Middle, lparam);
                LRESULT(0)
            }
            WM_XBUTTONDOWN => {
                let b = if (wparam.0 >> 16) & 0xFFFF == 1 { MouseButton::Back } else { MouseButton::Forward };
                w.mouse_down(b, lparam);
                LRESULT(1)
            }
            WM_LBUTTONUP => {
                w.mouse_up(MouseButton::Left, lparam);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                w.mouse_up(MouseButton::Right, lparam);
                LRESULT(0)
            }
            WM_MBUTTONUP => {
                w.mouse_up(MouseButton::Middle, lparam);
                LRESULT(0)
            }
            WM_XBUTTONUP => {
                let b = if (wparam.0 >> 16) & 0xFFFF == 1 { MouseButton::Back } else { MouseButton::Forward };
                w.mouse_up(b, lparam);
                LRESULT(1)
            }
            WM_CAPTURECHANGED => {
                // Lost the mouse mid-drag (another window took it): finish the drag where it is.
                if lparam.0 != hwnd.0 as isize {
                    let pos = w.st.mouse;
                    for (i, b) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle].into_iter().enumerate() {
                        if w.st.buttons[i] {
                            w.frame(Some(Event::MouseUp { pos, button: b, mods: Modifiers::NONE }));
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                w.wheel(wparam, lparam, false);
                LRESULT(0)
            }
            WM_MOUSEHWHEEL => {
                w.wheel(wparam, lparam, true);
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = wparam.0 as u32;
                let repeat = (lparam.0 >> 30) & 1 == 1;
                if let Some(key) = key_of(vk) {
                    w.frame(Some(Event::Key { key, mods: modifiers(), repeat }));
                }
                if msg == WM_SYSKEYDOWN && vk == VK_F4.0 as u32 {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                LRESULT(0)
            }
            WM_SYSKEYUP => {
                // Alt and F10 alone would open the (absent) menu bar and swallow the next key.
                let vk = wparam.0 as u32;
                if vk == VK_MENU.0 as u32 || vk == VK_F10.0 as u32 {
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_CHAR => {
                let unit = wparam.0 as u16;
                let text = if (0xD800..0xDC00).contains(&unit) {
                    w.surrogate = Some(unit);
                    None
                } else if (0xDC00..0xE000).contains(&unit) {
                    w.surrogate.take().map(|hi| String::from_utf16_lossy(&[hi, unit]))
                } else {
                    char::from_u32(unit as u32).filter(|c| !c.is_control()).map(String::from)
                };
                if let Some(t) = text {
                    w.frame(Some(Event::Text(t)));
                }
                LRESULT(0)
            }
            WM_SYSCHAR => {
                // Alt+Space opens the window menu; other Alt+letters would only beep.
                if wparam.0 == ' ' as usize {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                } else {
                    LRESULT(0)
                }
            }
            WM_IME_SETCONTEXT => {
                // The composition is drawn in the field itself.
                let l = LPARAM(lparam.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
                DefWindowProcW(hwnd, msg, wparam, l)
            }
            WM_IME_STARTCOMPOSITION => {
                if let Some(a) = w.ime_area {
                    w.place_ime(a);
                }
                LRESULT(0)
            }
            WM_IME_COMPOSITION => {
                w.composition(lparam);
                LRESULT(0)
            }
            WM_IME_ENDCOMPOSITION => {
                w.frame(Some(Event::Preedit { text: String::new(), caret: None }));
                LRESULT(0)
            }
            WM_DROPFILES => {
                let hdrop = HDROP(wparam.0 as *mut _);
                // Where it was dropped, so a drop on a terminal types the paths there.
                let mut at = POINT::default();
                let _ = DragQueryPoint(hdrop, &mut at);
                let files = dropped_files(hdrop);
                if !files.is_empty() {
                    let _ = SetForegroundWindow(hwnd);
                    let s = w.scale();
                    w.frame(Some(Event::MouseMove(Point::new(at.x as f32 / s, at.y as f32 / s))));
                    w.frame(Some(Event::Files(files)));
                }
                LRESULT(0)
            }
            WM_SETFOCUS => {
                w.frame(Some(Event::FocusChanged(true)));
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                w.frame(Some(Event::FocusChanged(false)));
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                let s = w.scale();
                info.ptMinTrackSize = POINT { x: (720.0 * s) as i32, y: (480.0 * s) as i32 };
                LRESULT(0)
            }
            WM_CLOSE => {
                let allowed = match w.platform.as_mut() {
                    Some(p) => {
                        w.busy.set(true);
                        let ok = w.program.close_requested(p);
                        w.busy.set(false);
                        ok
                    }
                    None => true,
                };
                if allowed {
                    w.closing = true;
                    let _ = DestroyWindow(hwnd);
                } else {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                if GetWindowPlacement(hwnd, &mut wp).is_ok() {
                    let r = wp.rcNormalPosition;
                    let maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32 || (wp.flags.0 & WPF_RESTORETOMAXIMIZED.0) != 0 && wp.showCmd == SW_SHOWMINIMIZED.0 as u32;
                    w.program.save_window_placement((r.left, r.top, r.right - r.left, r.bottom - r.top, maximized));
                }
                if let Some(p) = w.painter.as_mut() {
                    p.release();
                }
                let _ = RevokeDragDrop(hwnd);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

// MARK: - Platform services

struct WinClipboard {
    hwnd: HWND,
}

impl Clipboard for WinClipboard {
    fn get(&mut self) -> Option<String> {
        unsafe {
            OpenClipboard(Some(self.hwnd)).ok()?;
            let text = (|| {
                let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
                let g = HGLOBAL(h.0);
                let p = GlobalLock(g) as *const u16;
                if p.is_null() {
                    return None;
                }
                let size = GlobalSize(g) / 2;
                let mut len = 0;
                while len < size && *p.add(len) != 0 {
                    len += 1;
                }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                let _ = GlobalUnlock(g);
                Some(s)
            })();
            let _ = CloseClipboard();
            text.map(|t| t.replace("\r\n", "\n"))
        }
    }

    fn set(&mut self, text: &str) {
        let units: Vec<u16> = text.replace("\r\n", "\n").replace('\n', "\r\n").encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            if OpenClipboard(Some(self.hwnd)).is_err() {
                return;
            }
            let _ = EmptyClipboard();
            if let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) {
                let p = GlobalLock(g) as *mut u16;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
                    let _ = GlobalUnlock(g);
                    if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(g.0))).is_err() {
                        let _ = GlobalFree(Some(g));
                    }
                } else {
                    let _ = GlobalFree(Some(g));
                }
            }
            let _ = CloseClipboard();
        }
    }
}

struct WinPlatform {
    hwnd: HWND,
    quit: bool,
    clipboard: WinClipboard,
    /// A wake message is on its way: further wakes wait for it.
    wake: Arc<AtomicBool>,
    /// Files to drag out, once the frame that asked has finished.
    pending_drag: Option<Vec<PathBuf>>,
    /// A caption button's command, carried out once the frame that asked has finished.
    pending_command: Option<WindowCommand>,
}

impl Platform for WinPlatform {
    fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }

    fn monospace_families(&self) -> (String, Vec<String>) {
        render::monospace_families()
    }

    fn pick_folder(&mut self, title: &str) -> Option<PathBuf> {
        unsafe {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let options = dialog.GetOptions().ok()?;
            dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
            let _ = dialog.SetTitle(&HSTRING::from(title));
            dialog.Show(Some(self.hwnd)).ok()?;
            let item = dialog.GetResult().ok()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const _));
            path.map(PathBuf::from)
        }
    }

    fn open_url(&mut self, url: &str) {
        unsafe {
            ShellExecuteW(Some(self.hwnd), w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }

    fn reveal(&mut self, path: &Path) {
        let args = HSTRING::from(format!("/select,\"{}\"", path.display()));
        unsafe {
            ShellExecuteW(Some(self.hwnd), w!("open"), w!("explorer.exe"), &args, PCWSTR::null(), SW_SHOWNORMAL);
        }
    }

    fn waker(&self) -> Waker {
        let hwnd = self.hwnd.0 as isize;
        let flag = self.wake.clone();
        Arc::new(move || {
            if !flag.swap(true, Ordering::SeqCst) {
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0));
                }
            }
        })
    }

    fn set_title(&mut self, title: &str) {
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title));
        }
    }

    fn quit(&mut self) {
        self.quit = true;
    }

    fn foreground_process(&self, shell: u32) -> Option<String> {
        use windows::Win32::System::Diagnostics::ToolHelp::*;
        let mut all: Vec<(u32, u32, String)> = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
            let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    let exe = &entry.szExeFile;
                    let len = exe.iter().position(|&c| c == 0).unwrap_or(exe.len());
                    all.push((entry.th32ProcessID, entry.th32ParentProcessID, String::from_utf16_lossy(&exe[..len])));
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        diffcode_ui::terminal::foreground_among(&all, shell)
    }

    fn drag_files(&mut self, paths: Vec<PathBuf>) {
        self.pending_drag = Some(paths);
    }

    fn caption_buttons(&self) -> bool {
        true
    }

    fn is_maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd).as_bool() }
    }

    fn window_command(&mut self, command: WindowCommand) {
        self.pending_command = Some(command);
    }

    fn http_get(&self) -> Option<HttpGet> {
        Some(Arc::new(web_get))
    }

    fn app_icon(&self, pixels: u32) -> Option<Arc<Vec<u8>>> {
        icon_png(pixels).map(|png| Arc::new(png.to_vec()))
    }

    fn request_attention(&mut self) {
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: self.hwnd,
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        unsafe {
            let _ = FlashWindowEx(&info);
        }
    }

    fn memory_mb(&self) -> f64 {
        unsafe {
            // The private working set, as Task Manager's Memory column shows it, where Windows reports it (newer
            // builds); the whole working set elsewhere.
            let mut c = PROCESS_MEMORY_COUNTERS_EX2 { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32, ..Default::default() };
            if !K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb).as_bool() {
                return 0.0;
            }
            let bytes = if c.PrivateWorkingSetSize > 0 { c.PrivateWorkingSetSize } else { c.WorkingSetSize };
            bytes as f64 / (1024.0 * 1024.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use diffcode_ui::app::update::{parse_release, LATEST};
    use diffcode_ui::paint::{Backend, Font};

    #[test]
    fn lists_the_installed_monospace_fonts() {
        let (default, list) = super::render::monospace_families();
        assert!(["Cascadia Mono", "Consolas", "Courier New"].contains(&default.as_str()), "default {default}");
        assert!(list.iter().any(|f| f == "Consolas") && list.iter().any(|f| f == "Courier New"), "{list:?}");
        assert!(!list.iter().any(|f| f == "Segoe UI" || f == "Arial" || f == "Wingdings"), "{list:?}");
        eprintln!("default {default}; {} monospace families: {list:?}", list.len());
    }

    #[test]
    fn draws_in_the_faces_picked_in_settings() {
        let backend = super::render::DxBackend::new().expect("DirectWrite");
        let (code, terminal) = (Font::mono(12.0), Font::terminal(12.0));
        let default = backend.metrics(code);
        assert_eq!(backend.metrics(terminal), default);
        assert!(backend.set_families(Some("Courier New"), None));
        assert!(!backend.set_families(Some("Courier New"), None), "the same pick again changes nothing");
        let courier = backend.metrics(code);
        assert_ne!(courier, default);
        assert_eq!(backend.metrics(terminal), default, "the terminal keeps its own face");
        assert!(backend.set_families(Some("Courier New"), Some("Courier New")));
        assert_eq!(backend.metrics(terminal), courier);
        // A family that is not installed draws in the default face.
        assert!(backend.set_families(Some("No Such Font 4F2A"), None));
        assert_eq!(backend.metrics(code), default);
        assert_eq!(backend.metrics(terminal), default);
    }

    /// The real request the update check makes, through WinINet and whatever proxy Windows is set to use. It needs
    /// the network, so it runs only when asked: `cargo test -p diffcode -- --ignored`.
    #[test]
    #[ignore]
    fn asks_github_for_the_latest_release() {
        let reply = super::web_get(LATEST).expect("the request fails");
        assert!(reply.contains("\"tag_name\""), "unexpected reply: {}", &reply[..reply.len().min(300)]);
        parse_release(&reply).expect("the reply does not parse");
        eprintln!("latest release: {:?}", parse_release(&reply));
    }
}
