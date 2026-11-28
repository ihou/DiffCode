//! The Linux shell: one GTK window, Cairo drawing, and the services the interface asks of the system.
//!
//! GTK owns the window, the input methods, the clipboard, the folder picker and the event loop; the interface draws
//! every frame itself. Input waits in a queue and one event is handled per frame, as the Win32 shell handles one
//! message, so a burst of keys is never folded into a single frame. Signals only touch that queue — never the shell —
//! so an input method answering synchronously cannot borrow the frame state a second time.

mod paint;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use diffcode_ui::geom::{Point, Size};
use diffcode_ui::input::{Event, Key, Modifiers, MouseButton};
use diffcode_ui::paint::CursorIcon;
use diffcode_ui::platform::{HttpGet, Platform, Program, Waker, WindowCommand};
use diffcode_ui::text_field::Clipboard;
use diffcode_ui::ui::{Id, Ui, UiState};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use paint::{LinuxBackend, LinuxPainter};

/// Where settings and the window placement live: `$XDG_CONFIG_HOME/DiffCode`, else `~/.config/DiffCode`.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("DiffCode")
}

/// Whether the desktop's language is Chinese.
pub fn system_chinese() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .is_some_and(|lang| lang.to_ascii_lowercase().starts_with("zh"))
}

/// Whether the desktop asks for a dark interface.
fn system_dark() -> bool {
    if let Some(settings) = gtk::Settings::default() {
        if settings.is_gtk_application_prefer_dark_theme() {
            return true;
        }
        if let Some(name) = settings.gtk_theme_name() {
            if name.to_ascii_lowercase().contains("dark") {
                return true;
            }
        }
    }
    std::env::var("GTK_THEME").is_ok_and(|theme| theme.to_ascii_lowercase().contains("dark"))
}

pub fn run(program: Box<dyn Program>) {
    if gtk::init().is_err() {
        eprintln!("DiffCode needs a display (X11 or Wayland).");
        std::process::exit(1);
    }
    let backend = Rc::new(LinuxBackend::new());
    let area = gtk::DrawingArea::new();
    area.set_can_focus(true);
    area.add_events(
        gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::SCROLL_MASK
            | gdk::EventMask::SMOOTH_SCROLL_MASK
            | gdk::EventMask::KEY_PRESS_MASK
            | gdk::EventMask::ENTER_NOTIFY_MASK
            | gdk::EventMask::LEAVE_NOTIFY_MASK
            | gdk::EventMask::FOCUS_CHANGE_MASK,
    );
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("DiffCode");
    window.add(&area);
    if let Some(icon) = decode_icon(paint::APP_ICON_PNG) {
        window.set_icon(Some(&icon));
    }
    let geometry = gdk::Geometry::new(720, 480, 0, 0, 0, 0, 0, 0, 0.0, 0.0, gdk::Gravity::NorthWest);
    window.set_geometry_hints(Some(&area), Some(&geometry), gdk::WindowHints::MIN_SIZE);

    let waker = start_waker(&area);
    let mono = paint::monospace_families();
    let platform = LinuxPlatform {
        window: window.clone(),
        quit: false,
        command: None,
        waker,
        mono,
        clipboard: LinuxClipboard,
    };
    let queue = EventQueue::default();
    let dark = Rc::new(Cell::new(system_dark()));
    let ime_enabled = Rc::new(Cell::new(false));
    let im_context = gtk::IMMulticontext::new();
    let shell = Rc::new(RefCell::new(Shell {
        window: window.clone(),
        area: area.clone(),
        program,
        st: UiState::default(),
        backend,
        platform,
        events: VecDeque::new(),
        incoming: queue.clone(),
        dark: dark.clone(),
        closing: false,
        generation: Rc::new(Cell::new(0)),
        focus: None,
        ime_enabled: ime_enabled.clone(),
        im_context: im_context.clone(),
        cursors: HashMap::new(),
    }));

    if let Some((x, y, w, h, maximized)) = shell.borrow().program.window_placement() {
        window.move_(x, y);
        window.resize(w.max(720), h.max(480));
        if maximized {
            window.maximize();
        }
    } else {
        window.set_default_size(1280, 820);
    }

    connect_input(&area, &window, &im_context, &queue, &ime_enabled, &shell);
    connect_draw(&area, &shell);
    setup_drop(&area, &queue);
    setup_ime(&im_context, &queue, &area);
    connect_look(&area, &dark);

    window.show_all();
    area.grab_focus();
    if let Some(gdk_window) = area.window() {
        im_context.set_client_window(Some(&gdk_window));
    }
    area.queue_draw();
    gtk::main();
    // The scratch folder holds deleted-file undo copies for this run; nothing needs them after the loop ends.
    diffcode_core::diff::cleanup();
}

/// Events from GTK signals wait here until a frame takes one.
#[derive(Clone, Default)]
struct EventQueue(Rc<RefCell<VecDeque<Event>>>);

impl EventQueue {
    fn push(&self, event: Event, area: &gtk::DrawingArea) {
        {
            let mut queue = self.0.borrow_mut();
            // A drag or a moving mouse only needs its latest position; keeping every one would lag a frame behind.
            if matches!(event, Event::MouseMove(_)) {
                if let Some(at) = queue.iter().position(|e| matches!(e, Event::MouseMove(_))) {
                    queue.remove(at);
                }
            }
            queue.push_back(event);
        }
        area.queue_draw();
    }

    fn drain_into(&self, into: &mut VecDeque<Event>) {
        let mut queue = self.0.borrow_mut();
        for event in queue.drain(..) {
            if matches!(event, Event::MouseMove(_)) {
                if let Some(Event::MouseMove(_)) = into.back() {
                    into.pop_back();
                }
            }
            into.push_back(event);
        }
    }
}

/// Wakes the main thread from a worker (a terminal, a Git job, a watcher) and queues one frame.
fn start_waker(area: &gtk::DrawingArea) -> Waker {
    let (tx, rx) = async_channel::bounded::<()>(1);
    let pending = Arc::new(AtomicBool::new(false));
    let flag = pending.clone();
    let waker = Arc::new(move || {
        if !flag.swap(true, Ordering::SeqCst) {
            let _ = tx.try_send(());
        }
    });
    let area = area.clone();
    glib::spawn_future_local(async move {
        while rx.recv().await.is_ok() {
            pending.store(false, Ordering::SeqCst);
            area.queue_draw();
        }
    });
    waker
}

fn connect_draw(area: &gtk::DrawingArea, shell: &Rc<RefCell<Shell>>) {
    let weak = Rc::downgrade(shell);
    area.connect_draw(move |area, cr| {
        let Some(shell) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        // A modal dialog (the folder picker) runs a nested loop; its draw requests leave the frame alone.
        let Ok(mut shell) = shell.try_borrow_mut() else {
            return glib::Propagation::Proceed;
        };
        shell.draw(area, cr);
        glib::Propagation::Proceed
    });
}

fn connect_input(
    area: &gtk::DrawingArea,
    window: &gtk::Window,
    im_context: &gtk::IMMulticontext,
    queue: &EventQueue,
    ime_enabled: &Rc<Cell<bool>>,
    shell: &Rc<RefCell<Shell>>,
) {
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_button_press_event(move |_, event| {
            target.grab_focus();
            let (x, y) = event.position();
            let at = Point::new(x as f32, y as f32);
            let button = match event.button() {
                1 => MouseButton::Left,
                3 => MouseButton::Right,
                2 => MouseButton::Middle,
                8 => MouseButton::Back,
                9 => MouseButton::Forward,
                _ => return glib::Propagation::Proceed,
            };
            // GTK tags the press events of a double or triple click itself; counting here as well
            // would make every double click a triple one.
            let count = match event.event_type() {
                gdk::EventType::ButtonPress => 1,
                gdk::EventType::DoubleButtonPress => 2,
                gdk::EventType::TripleButtonPress => 3,
                _ => return glib::Propagation::Proceed,
            };
            queue.push(
                Event::MouseDown {
                    pos: at,
                    button,
                    clicks: count,
                    mods: modifiers(event.state()),
                },
                &target,
            );
            glib::Propagation::Proceed
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_button_release_event(move |_, event| {
            let (x, y) = event.position();
            let button = match event.button() {
                1 => MouseButton::Left,
                3 => MouseButton::Right,
                2 => MouseButton::Middle,
                8 => MouseButton::Back,
                9 => MouseButton::Forward,
                _ => return glib::Propagation::Proceed,
            };
            queue.push(
                Event::MouseUp {
                    pos: Point::new(x as f32, y as f32),
                    button,
                    mods: modifiers(event.state()),
                },
                &target,
            );
            glib::Propagation::Proceed
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_motion_notify_event(move |_, event| {
            let (x, y) = event.position();
            queue.push(Event::MouseMove(Point::new(x as f32, y as f32)), &target);
            glib::Propagation::Proceed
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_enter_notify_event(move |_, event| {
            let (x, y) = event.position();
            queue.push(Event::MouseMove(Point::new(x as f32, y as f32)), &target);
            glib::Propagation::Proceed
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_leave_notify_event(move |_, _| {
            queue.push(Event::MouseLeave, &target);
            glib::Propagation::Proceed
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_scroll_event(move |_, event| {
            wheel(&queue, &target, event);
            glib::Propagation::Stop
        });
    }
    let shift_held = Rc::new(Cell::new(false));
    {
        let queue = queue.clone();
        let target = area.clone();
        let im_context = im_context.clone();
        let ime_enabled = ime_enabled.clone();
        let shift_held = shift_held.clone();
        area.connect_key_press_event(move |_, event| {
            if ime_enabled.get() && im_context.filter_keypress(event) {
                return glib::Propagation::Stop;
            }
            // Auto-repeat produces a press per repeat; only the first one is a new press.
            let repeat = is_shift(event.keyval()) && shift_held.replace(true);
            key_pressed(&queue, &target, event, repeat);
            glib::Propagation::Stop
        });
    }
    {
        let shift_held = shift_held.clone();
        area.connect_key_release_event(move |_, event| {
            if is_shift(event.keyval()) {
                shift_held.set(false);
            }
            glib::Propagation::Proceed
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        let im_context = im_context.clone();
        let window = window.clone();
        area.connect_focus_in_event(move |_, _| {
            window.set_urgency_hint(false);
            im_context.focus_in();
            queue.push(Event::FocusChanged(true), &target);
            glib::Propagation::Proceed
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        let im_context = im_context.clone();
        area.connect_focus_out_event(move |_, _| {
            im_context.focus_out();
            queue.push(Event::FocusChanged(false), &target);
            glib::Propagation::Proceed
        });
    }
    {
        let weak = Rc::downgrade(shell);
        window.connect_delete_event(move |_, _| {
            if let Some(shell) = weak.upgrade() {
                let Ok(mut shell) = shell.try_borrow_mut() else {
                    return glib::Propagation::Stop;
                };
                if shell.close() {
                    gtk::main_quit();
                }
            }
            glib::Propagation::Stop
        });
    }
}

fn setup_ime(im_context: &gtk::IMMulticontext, queue: &EventQueue, area: &gtk::DrawingArea) {
    {
        let queue = queue.clone();
        let target = area.clone();
        im_context.connect_commit(move |_, text| {
            queue.push(Event::Text(text.to_string()), &target);
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        im_context.connect_preedit_changed(move |context| {
            let (text, _, cursor) = context.preedit_string();
            let text = text.to_string();
            let event = if text.is_empty() {
                Event::Preedit { text, caret: None }
            } else {
                Event::Preedit {
                    text,
                    caret: Some(cursor.max(0) as usize),
                }
            };
            queue.push(event, &target);
        });
    }
}

fn setup_drop(area: &gtk::DrawingArea, queue: &EventQueue) {
    area.drag_dest_set(
        gtk::DestDefaults::MOTION | gtk::DestDefaults::HIGHLIGHT,
        &[gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::OTHER_APP, 0)],
        gdk::DragAction::COPY,
    );
    area.connect_drag_drop(move |area, context, _, _, time| {
        area.drag_get_data(context, &gdk::Atom::intern("text/uri-list"), time);
        true
    });
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_drag_motion(move |_, context, x, y, time| {
            context.drag_status(gdk::DragAction::COPY, time);
            queue.push(Event::FilesHover(Some(Point::new(x as f32, y as f32))), &target);
            true
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_drag_leave(move |_, _, _| {
            queue.push(Event::FilesHover(None), &target);
        });
    }
    {
        let queue = queue.clone();
        let target = area.clone();
        area.connect_drag_data_received(move |_, context, _, _, data, _, time| {
            let paths: Vec<PathBuf> = data
                .uris()
                .iter()
                .filter_map(|uri| gtk::gio::File::for_uri(uri).path())
                .collect();
            context.drag_finish(!paths.is_empty(), false, time);
            if !paths.is_empty() {
                queue.push(Event::Files(paths), &target);
            }
            queue.push(Event::FilesHover(None), &target);
        });
    }
}

/// The desktop's dark/light choice can change while DiffCode runs.
fn connect_look(area: &gtk::DrawingArea, dark: &Rc<Cell<bool>>) {
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    let area = area.clone();
    let dark = dark.clone();
    settings.connect_notify_local(None, move |_, _| {
        let now = system_dark();
        if dark.get() != now {
            dark.set(now);
            area.queue_draw();
        }
    });
}

fn decode_icon(png: &[u8]) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let stream = gtk::gio::MemoryInputStream::from_bytes(&glib::Bytes::from(png));
    gtk::gdk_pixbuf::Pixbuf::from_stream(&stream, None::<&gtk::gio::Cancellable>).ok()
}

struct Shell {
    window: gtk::Window,
    area: gtk::DrawingArea,
    program: Box<dyn Program>,
    st: UiState,
    backend: Rc<LinuxBackend>,
    platform: LinuxPlatform,
    /// Input that arrived while a frame was running, and input not handled yet.
    incoming: EventQueue,
    events: VecDeque<Event>,
    dark: Rc<Cell<bool>>,
    closing: bool,
    /// Bumped every frame, so a timer scheduled by an earlier frame does nothing when it fires.
    generation: Rc<Cell<u64>>,
    focus: Option<Id>,
    ime_enabled: Rc<Cell<bool>>,
    im_context: gtk::IMMulticontext,
    cursors: HashMap<&'static str, gdk::Cursor>,
}

impl Shell {
    fn draw(&mut self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context) {
        let size = Size::new(area.allocated_width() as f32, area.allocated_height() as f32);
        if size.w < 1.0 || size.h < 1.0 {
            return;
        }
        self.incoming.drain_into(&mut self.events);
        let event = self.events.pop_front();
        if let Some(event) = &event {
            self.st.observe(event);
        }
        let theme = self.program.theme(self.dark.get());
        let clicked = matches!(event, Some(Event::MouseUp { .. }));
        {
            let mut painter = LinuxPainter::new(cr, &self.backend);
            let mut ui = Ui::begin(&mut self.st, &mut painter, self.backend.as_ref(), &theme, event, size);
            self.program.frame(&mut ui, &mut self.platform);
            ui.finish();
        }
        self.set_cursor();
        self.sync_ime();
        if self.st.repaint_now || clicked || !self.events.is_empty() {
            area.queue_draw();
        }
        if let Some(delay) = self.st.repaint_after {
            let area = area.clone();
            let generation = self.generation.clone();
            let frame = self.st.frame();
            generation.set(frame);
            glib::timeout_add_local_once(delay, move || {
                if generation.get() == frame {
                    area.queue_draw();
                }
            });
        }
        if self.platform.quit {
            self.platform.quit = false;
            let window = self.window.clone();
            glib::idle_add_local_once(move || window.close());
        }
        if let Some(command) = self.platform.command.take() {
            match command {
                WindowCommand::Minimize => self.window.iconify(),
                WindowCommand::ToggleMaximize if self.window.is_maximized() => self.window.unmaximize(),
                WindowCommand::ToggleMaximize => self.window.maximize(),
                WindowCommand::Close => {
                    if self.close() {
                        gtk::main_quit();
                    }
                }
            }
        }
    }

    /// The window is closing: `true` when the program agrees.
    fn close(&mut self) -> bool {
        if self.closing {
            return true;
        }
        if !self.program.close_requested(&mut self.platform) {
            return false;
        }
        let (x, y) = self.window.position();
        let (w, h) = self.window.size();
        self.program
            .save_window_placement((x, y, w, h, self.window.is_maximized()));
        self.closing = true;
        true
    }

    fn set_cursor(&mut self) {
        let Some(gdk_window) = self.area.window() else { return };
        let Some(display) = gdk::Display::default() else { return };
        let name = match self.st.cursor {
            CursorIcon::Arrow => "default",
            CursorIcon::Hand => "pointer",
            CursorIcon::Text => "text",
            CursorIcon::ResizeHorizontal => "ew-resize",
            CursorIcon::ResizeVertical => "ns-resize",
            CursorIcon::Busy => "wait",
        };
        if !self.cursors.contains_key(name) {
            let made = gdk::Cursor::from_name(&display, name)
                .or_else(|| gdk::Cursor::for_display(&display, gdk::CursorType::LeftPtr));
            let Some(made) = made else { return };
            self.cursors.insert(name, made);
        }
        if let Some(cursor) = self.cursors.get(name) {
            gdk_window.set_cursor(Some(cursor));
        }
    }

    fn sync_ime(&mut self) {
        let wants = self.st.ime_area.is_some();
        if wants != self.ime_enabled.replace(wants) {
            if wants {
                self.im_context.focus_in();
            } else {
                self.im_context.focus_out();
                self.im_context.reset();
            }
        }
        if self.st.focus != self.focus {
            self.focus = self.st.focus;
            self.im_context.reset();
        }
        if let Some(area) = self.st.ime_area {
            let rect = gdk::Rectangle::new(area.x.round() as i32, area.bottom().round() as i32, 1, 1);
            self.im_context.set_cursor_location(&rect);
        }
        if self.st.ime_reset {
            self.im_context.reset();
        }
    }
}

fn modifiers(state: gdk::ModifierType) -> Modifiers {
    Modifiers {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        alt: state.contains(gdk::ModifierType::MOD1_MASK),
    }
}

fn wheel(queue: &EventQueue, area: &gtk::DrawingArea, event: &gdk::EventScroll) {
    let (x, y) = event.position();
    let (dx, dy) = event.delta();
    // A wheel notch is 57 DIPs, the same distance as on Windows. Touchpads report smooth
    // deltas in DIPs already, so those must not be multiplied again.
    let smooth = event.direction() == gdk::ScrollDirection::Smooth;
    let step = if smooth { 1.0 } else { 57.0 };
    let (horizontal, delta) = match event.direction() {
        gdk::ScrollDirection::Up => (false, 1.0),
        gdk::ScrollDirection::Down => (false, -1.0),
        gdk::ScrollDirection::Left => (true, -1.0),
        gdk::ScrollDirection::Right => (true, 1.0),
        _ if dx != 0.0 => (true, dx),
        _ => (false, -dy),
    };
    let mods = modifiers(event.state());
    let delta = wheel_delta(horizontal, mods.shift, delta, step);
    queue.push(
        Event::Wheel {
            pos: Point::new(x as f32, y as f32),
            delta,
            mods,
        },
        area,
    );
}

const FKEYS: [gdk::keys::Key; 12] = [
    gdk::keys::constants::F1,
    gdk::keys::constants::F2,
    gdk::keys::constants::F3,
    gdk::keys::constants::F4,
    gdk::keys::constants::F5,
    gdk::keys::constants::F6,
    gdk::keys::constants::F7,
    gdk::keys::constants::F8,
    gdk::keys::constants::F9,
    gdk::keys::constants::F10,
    gdk::keys::constants::F11,
    gdk::keys::constants::F12,
];

fn is_shift(keyval: gdk::keys::Key) -> bool {
    matches!(keyval, gdk::keys::constants::Shift_L | gdk::keys::constants::Shift_R)
}

fn key_pressed(queue: &EventQueue, area: &gtk::DrawingArea, event: &gdk::EventKey, repeat: bool) {
    let mut mods = modifiers(event.state());
    // GDK reports the state before the key press, so Shift does not include itself yet.
    if is_shift(event.keyval()) {
        mods.shift = true;
    }
    if let Some(key) = key_of(event.keyval()) {
        queue.push(Event::Key { key, mods, repeat }, area);
    }
    if mods.ctrl || mods.alt {
        return;
    }
    if let Some(text) = typed(event.keyval()) {
        queue.push(Event::Text(text.to_string()), area);
    }
}

fn key_of(keyval: gdk::keys::Key) -> Option<Key> {
    use gdk::keys::constants as gk;
    if let Some(index) = FKEYS.iter().position(|candidate| *candidate == keyval) {
        return Some(Key::F(index as u8 + 1));
    }
    Some(match keyval {
        gk::Return | gk::KP_Enter | gk::ISO_Enter => Key::Enter,
        gk::Escape => Key::Escape,
        gk::Tab | gk::ISO_Left_Tab => Key::Tab,
        gk::BackSpace => Key::Backspace,
        gk::Delete | gk::KP_Delete => Key::Delete,
        gk::Insert | gk::KP_Insert => Key::Insert,
        gk::Left | gk::KP_Left => Key::Left,
        gk::Right | gk::KP_Right => Key::Right,
        gk::Up | gk::KP_Up => Key::Up,
        gk::Down | gk::KP_Down => Key::Down,
        gk::Home | gk::KP_Home => Key::Home,
        gk::End | gk::KP_End => Key::End,
        gk::Page_Up | gk::KP_Page_Up => Key::PageUp,
        gk::Page_Down | gk::KP_Page_Down => Key::PageDown,
        gk::Shift_L | gk::Shift_R => Key::Shift,
        gk::space => Key::Space,
        _ => {
            let letter = keyval.to_unicode()?;
            if letter.is_ascii_alphabetic() {
                Key::Char(letter.to_ascii_lowercase())
            } else if !letter.is_control() {
                Key::Char(letter)
            } else {
                return None;
            }
        }
    })
}

/// The characters a key press types, as opposed to the commands it may mean.
fn typed(keyval: gdk::keys::Key) -> Option<char> {
    let c = keyval.to_unicode()?;
    (!c.is_control()).then_some(c)
}

struct LinuxClipboard;

impl Clipboard for LinuxClipboard {
    fn get(&mut self) -> Option<String> {
        let display = gdk::Display::default()?;
        let board = gtk::Clipboard::default(&display)?;
        board.wait_for_text().map(|text| text.replace("\r\n", "\n"))
    }

    fn set(&mut self, text: &str) {
        let Some(display) = gdk::Display::default() else { return };
        let Some(board) = gtk::Clipboard::default(&display) else {
            return;
        };
        board.set_text(text);
        board.store();
    }
}

struct LinuxPlatform {
    window: gtk::Window,
    quit: bool,
    command: Option<WindowCommand>,
    waker: Waker,
    mono: (String, Vec<String>),
    clipboard: LinuxClipboard,
}

impl Platform for LinuxPlatform {
    fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }

    fn pick_folder(&mut self, title: &str) -> Option<PathBuf> {
        let dialog = gtk::FileChooserDialog::builder()
            .title(title)
            .transient_for(&self.window)
            .modal(true)
            .destroy_with_parent(true)
            .action(gtk::FileChooserAction::SelectFolder)
            .build();
        dialog.add_button(&diffcode_core::lang::l("Cancel", "取消"), gtk::ResponseType::Cancel);
        dialog.add_button(&diffcode_core::lang::l("Select", "选择"), gtk::ResponseType::Accept);
        dialog.set_default_response(gtk::ResponseType::Accept);
        let path = if dialog.run() == gtk::ResponseType::Accept {
            dialog.filename()
        } else {
            None
        };
        dialog.close();
        // The location entry completes a typed repository path to its ".git" folder;
        // the project is the work tree around it.
        path.map(|p| {
            let parent = p.parent().map(Path::to_path_buf);
            if p.file_name().is_some_and(|n| n == ".git") {
                parent.unwrap_or(p)
            } else {
                p
            }
        })
    }

    fn open_url(&mut self, url: &str) {
        let _ = gtk::gio::AppInfo::launch_default_for_uri(url, None::<&gtk::gio::AppLaunchContext>);
    }

    fn reveal(&mut self, path: &Path) {
        let folder = path.parent().unwrap_or(path);
        let uri = gtk::gio::File::for_path(folder).uri();
        let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);
    }

    fn waker(&self) -> Waker {
        self.waker.clone()
    }

    fn set_title(&mut self, title: &str) {
        self.window.set_title(title);
    }

    fn quit(&mut self) {
        self.quit = true;
    }

    fn memory_mb(&self) -> f64 {
        let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
            return 0.0;
        };
        let pages: f64 = statm
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        pages * page_size() / (1024.0 * 1024.0)
    }

    fn foreground_process(&self, shell: u32) -> Option<String> {
        let mut all: Vec<(u32, u32, String)> = Vec::new();
        let entries = std::fs::read_dir("/proc").ok()?;
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
                continue;
            };
            let mut name = String::new();
            let mut parent = 0;
            for line in status.lines() {
                if let Some(v) = line.strip_prefix("Name:") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("PPid:") {
                    parent = v.trim().parse().unwrap_or(0);
                }
            }
            if !name.is_empty() {
                all.push((pid, parent, name));
            }
        }
        diffcode_ui::terminal::foreground_among(&all, shell)
    }

    fn request_attention(&mut self) {
        self.window.set_urgency_hint(true);
    }

    fn is_maximized(&self) -> bool {
        self.window.is_maximized()
    }

    fn window_command(&mut self, command: WindowCommand) {
        self.command = Some(command);
    }

    fn http_get(&self) -> Option<HttpGet> {
        None
    }

    fn app_icon(&self, _pixels: u32) -> Option<Arc<Vec<u8>>> {
        Some(Arc::new(paint::APP_ICON_PNG.to_vec()))
    }

    fn monospace_families(&self) -> (String, Vec<String>) {
        self.mono.clone()
    }
}

fn page_size() -> f64 {
    // Linux reports memory in pages; the page size is fixed for the life of the process.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size <= 0 {
        4096.0
    } else {
        size as f64
    }
}

/// Wheel units: positive `delta` is a wheel turn up (or right for a horizontal wheel), as in the Windows backend.
fn wheel_delta(horizontal: bool, shift: bool, delta: f64, step: f64) -> Point {
    if horizontal {
        Point::new((delta * step) as f32, 0.0)
    } else if shift {
        Point::new((-delta * step) as f32, 0.0)
    } else {
        Point::new(0.0, (-delta * step) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_matches_the_windows_directions() {
        assert_eq!(wheel_delta(false, false, 1.0, 57.0), Point::new(0.0, -57.0));
        assert_eq!(wheel_delta(false, true, 1.0, 57.0), Point::new(-57.0, 0.0));
        assert_eq!(wheel_delta(true, false, 1.0, 57.0), Point::new(57.0, 0.0));
    }

    #[test]
    fn both_shift_keys_are_recognized() {
        assert!(is_shift(gdk::keys::constants::Shift_L));
        assert!(is_shift(gdk::keys::constants::Shift_R));
        assert!(!is_shift(gdk::keys::constants::Control_L));
    }

    #[test]
    fn page_size_is_usable() {
        assert!(page_size() > 0.0);
    }
}
