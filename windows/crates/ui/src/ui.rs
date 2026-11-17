//! The immediate-mode context a frame is drawn with.
//!
//! Each frame the views are drawn from the app state top to bottom. A widget registers the area it reacts to with
//! an `Id`; which widget the mouse is over is decided from the areas of the frame before (topmost layer first, then
//! the last drawn), so popups drawn late still win. The shell runs one frame per discrete input event.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::geom::{Color, Point, Rect, Size};
use crate::input::{Event, Key, Modifiers, MouseButton};
use crate::paint::{Backend, ColorRun, CursorIcon, Font, Overflow, Painter, TextLayout, TextRequest, TextStyle};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Id(pub u64);

impl Id {
    pub fn new(name: &str) -> Id {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut h);
        Id(h.finish())
    }

    pub fn with(self, part: impl Hash) -> Id {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut h);
        part.hash(&mut h);
        Id(h.finish())
    }
}

/// Empty space in the top bar, where the window is dragged from when the interface draws its own title bar.
pub fn window_drag_id() -> Id {
    Id::new("window-drag")
}

/// The interface's maximize button: the shell tells Windows where it is, for the snap layouts that open over it.
pub fn maximize_button_id() -> Id {
    Id::new("caption-maximize")
}

/// Stacking order: later layers are drawn over and hit before earlier ones.
pub mod layer {
    pub const BASE: u8 = 0;
    pub const POPUP: u8 = 1;
    pub const MENU: u8 = 2;
    pub const DIALOG: u8 = 3;
    pub const TOOLTIP: u8 = 4;
}

#[derive(Clone, Copy, Debug)]
struct Hit {
    id: Id,
    rect: Rect,
    layer: u8,
}

struct CachedLayout {
    request: TextRequest,
    layout: Rc<dyn TextLayout>,
    used: u64,
}

/// What a widget's area did this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Response {
    pub id: Id,
    pub rect: Rect,
    pub hovered: bool,
    /// The left button went down on it this frame.
    pub pressed: bool,
    /// The left button was released on it after going down on it.
    pub clicked: bool,
    pub double_clicked: bool,
    pub triple_clicked: bool,
    /// The right button went down on it (a context menu opens on press, as in Windows lists).
    pub secondary: bool,
    pub middle: bool,
    /// The left button is held after going down on it.
    pub dragging: bool,
    pub mods: Modifiers,
}

/// State that lives across frames.
pub struct UiState {
    frame: u64,
    hits_prev: Vec<Hit>,
    hits: Vec<Hit>,
    scrolls_prev: Vec<Hit>,
    scrolls: Vec<Hit>,
    hovered: Option<Id>,
    active: Option<Id>,
    pub focus: Option<Id>,
    press_pos: Point,
    hover_start: (Option<Id>, Instant),
    cache: HashMap<u64, CachedLayout>,
    clip: Vec<Rect>,
    layer: u8,
    tooltip: Option<(String, Rect)>,
    // Output for the shell.
    pub cursor: CursorIcon,
    pub ime_area: Option<Rect>,
    /// Set when a composition was ended by the app (a click, a save): the input method drops its own.
    pub ime_reset: bool,
    pub repaint_after: Option<Duration>,
    /// Set when something wants another frame straight away (an animation, a state change from a click).
    pub repaint_now: bool,
    pub mouse: Point,
    pub mouse_inside: bool,
    pub buttons: [bool; 3],
    /// The value a drag started from (a splitter's width), by the widget being dragged.
    pub drag_value: Option<(Id, f32)>,
    /// The text field that had the keyboard last frame, and the one this frame (set as fields draw).
    text_input_prev: Option<Id>,
    text_input: Option<Id>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            frame: 0,
            hits_prev: Vec::new(),
            hits: Vec::new(),
            scrolls_prev: Vec::new(),
            scrolls: Vec::new(),
            hovered: None,
            active: None,
            focus: None,
            press_pos: Point::default(),
            hover_start: (None, Instant::now()),
            cache: HashMap::new(),
            clip: Vec::new(),
            layer: layer::BASE,
            tooltip: None,
            cursor: CursorIcon::Arrow,
            ime_area: None,
            ime_reset: false,
            repaint_after: None,
            repaint_now: false,
            mouse: Point::new(-1.0, -1.0),
            mouse_inside: false,
            buttons: [false; 3],
            drag_value: None,
            text_input_prev: None,
            text_input: None,
        }
    }
}

impl UiState {
    /// Takes in what an event says about the mouse; call before the frame that handles it.
    pub fn observe(&mut self, event: &Event) {
        match event {
            Event::MouseMove(p) => {
                self.mouse = *p;
                self.mouse_inside = true;
            }
            Event::MouseDown { pos, button, .. } => {
                self.mouse = *pos;
                if let Some(i) = button_index(*button) {
                    self.buttons[i] = true;
                }
            }
            Event::MouseUp { pos, button, .. } => {
                self.mouse = *pos;
                if let Some(i) = button_index(*button) {
                    self.buttons[i] = false;
                }
            }
            Event::MouseLeave => {
                self.mouse_inside = false;
                self.mouse = Point::new(-1.0, -1.0);
            }
            Event::Wheel { pos, .. } => self.mouse = *pos,
            Event::FilesHover(Some(p)) => self.mouse = *p,
            Event::FocusChanged(false) => self.buttons = [false; 3],
            _ => {}
        }
    }

    pub fn is_active(&self, id: Id) -> bool {
        self.active == Some(id)
    }

    pub fn has_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }
}

fn button_index(b: MouseButton) -> Option<usize> {
    match b {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        _ => None,
    }
}

pub struct Ui<'a> {
    pub st: &'a mut UiState,
    pub painter: &'a mut dyn Painter,
    pub backend: &'a dyn Backend,
    pub theme: &'a Theme,
    /// The one discrete event this frame handles, if any.
    pub event: Option<Event>,
    pub size: Size,
    pub now: Instant,
    consumed: bool,
}

impl<'a> Ui<'a> {
    pub fn begin(
        st: &'a mut UiState,
        painter: &'a mut dyn Painter,
        backend: &'a dyn Backend,
        theme: &'a Theme,
        event: Option<Event>,
        size: Size,
    ) -> Ui<'a> {
        st.frame += 1;
        // A face picked in Settings since the last frame: what was laid out in the old one goes.
        if backend.set_families(theme.code_family.as_deref(), theme.terminal_family.as_deref()) {
            st.cache.clear();
        }
        st.hits_prev = std::mem::take(&mut st.hits);
        st.scrolls_prev = std::mem::take(&mut st.scrolls);
        st.hovered = if st.mouse_inside { topmost(&st.hits_prev, st.mouse) } else { None };
        if st.hover_start.0 != st.hovered {
            st.hover_start = (st.hovered, Instant::now());
        }
        st.cursor = CursorIcon::Arrow;
        st.ime_area = None;
        st.ime_reset = false;
        st.repaint_after = None;
        st.repaint_now = false;
        st.tooltip = None;
        st.clip.clear();
        st.layer = layer::BASE;
        st.text_input_prev = st.text_input.take();
        Ui { st, painter, backend, theme, event, size, now: Instant::now(), consumed: false }
    }

    /// Ends the frame: tooltips on top, and the bookkeeping for the next one.
    pub fn finish(mut self) {
        if let Some((text, anchor)) = self.st.tooltip.take() {
            crate::widgets::draw_tooltip(&mut self, &text, anchor);
        }
        if matches!(self.event, Some(Event::MouseUp { button: MouseButton::Left, .. })) || !self.st.buttons[0] {
            self.st.active = None;
        }
        let frame = self.st.frame;
        if self.st.cache.len() > 3000 {
            self.st.cache.retain(|_, c| frame - c.used < 120);
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size.w, self.size.h)
    }

    pub fn mouse(&self) -> Point {
        self.st.mouse
    }

    pub fn mods(&self) -> Modifiers {
        match &self.event {
            Some(Event::MouseDown { mods, .. }) | Some(Event::MouseUp { mods, .. }) | Some(Event::Key { mods, .. }) => {
                *mods
            }
            Some(Event::Wheel { mods, .. }) => *mods,
            _ => Modifiers::NONE,
        }
    }

    /// Marks this frame's event as handled, so later widgets leave it alone.
    pub fn consume(&mut self) {
        self.consumed = true;
    }

    pub fn is_consumed(&self) -> bool {
        self.consumed
    }

    /// This frame's key press, when nothing has handled it yet.
    pub fn key(&self) -> Option<(Key, Modifiers)> {
        if self.consumed {
            return None;
        }
        match &self.event {
            Some(Event::Key { key, mods, .. }) => Some((*key, *mods)),
            _ => None,
        }
    }

    /// Whether this frame's key press is `key` with exactly `mods`; consumes it when it is.
    pub fn take_key(&mut self, key: Key, mods: Modifiers) -> bool {
        if self.key() == Some((key, mods)) {
            self.consumed = true;
            true
        } else {
            false
        }
    }

    pub fn request_repaint(&mut self) {
        self.st.repaint_now = true;
    }

    pub fn repaint_after(&mut self, d: Duration) {
        self.st.repaint_after = Some(self.st.repaint_after.map_or(d, |e| e.min(d)));
    }

    pub fn set_cursor(&mut self, cursor: CursorIcon) {
        self.st.cursor = cursor;
    }

    // MARK: Layers and clipping

    pub fn layer(&self) -> u8 {
        self.st.layer
    }

    pub fn with_layer<R>(&mut self, layer: u8, f: impl FnOnce(&mut Ui) -> R) -> R {
        let saved = self.st.layer;
        let saved_clip = std::mem::take(&mut self.st.clip);
        self.st.layer = layer;
        let r = f(self);
        self.st.layer = saved;
        self.st.clip = saved_clip;
        r
    }

    pub fn clip_rect(&self) -> Rect {
        self.st.clip.last().copied().unwrap_or_else(|| self.bounds())
    }

    pub fn push_clip(&mut self, r: Rect) {
        let c = self.clip_rect().intersect(&r);
        self.st.clip.push(c);
        self.painter.push_clip(c);
    }

    pub fn pop_clip(&mut self) {
        self.st.clip.pop();
        self.painter.pop_clip();
    }

    pub fn clipped<R>(&mut self, r: Rect, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.push_clip(r);
        let out = f(self);
        self.pop_clip();
        out
    }

    pub fn is_visible(&self, r: &Rect) -> bool {
        self.clip_rect().intersects(r)
    }

    // MARK: Interaction

    /// Registers an area that reacts to the mouse and reports what happened to it this frame.
    pub fn interact(&mut self, id: Id, rect: Rect) -> Response {
        let visible = rect.intersect(&self.clip_rect());
        if !visible.is_empty() {
            self.st.hits.push(Hit { id, rect: visible, layer: self.st.layer });
        }
        let hovered = self.st.hovered == Some(id) && visible.contains(self.st.mouse);
        let mut r = Response { id, rect, hovered, ..Default::default() };
        if self.consumed {
            r.dragging = self.st.active == Some(id) && self.st.buttons[0];
            return r;
        }
        match &self.event {
            Some(Event::MouseDown { pos, button, clicks, mods }) if hovered => {
                r.mods = *mods;
                match button {
                    MouseButton::Left => {
                        r.pressed = true;
                        r.double_clicked = *clicks == 2;
                        r.triple_clicked = *clicks >= 3;
                        self.st.active = Some(id);
                        self.st.press_pos = *pos;
                    }
                    MouseButton::Right => r.secondary = true,
                    MouseButton::Middle => r.middle = true,
                    _ => {}
                }
            }
            Some(Event::MouseUp { pos, button: MouseButton::Left, mods }) if self.st.active == Some(id) => {
                r.mods = *mods;
                if visible.contains(*pos) {
                    r.clicked = true;
                }
            }
            _ => {}
        }
        r.dragging = self.st.active == Some(id) && self.st.buttons[0];
        r
    }

    /// Where the left button went down for the widget being dragged.
    pub fn press_pos(&self) -> Point {
        self.st.press_pos
    }

    /// Where the window can be dragged from; drawn first, so whatever is drawn over it keeps its clicks.
    pub fn window_drag_area(&mut self, rect: Rect) {
        self.interact(window_drag_id(), rect);
    }

    /// An area that takes the mouse so nothing under it does (a popup's backdrop, a panel's empty space).
    pub fn block(&mut self, id: Id, rect: Rect) -> Response {
        self.interact(id, rect)
    }

    /// Registers a scroll area; returns the wheel movement aimed at it this frame.
    pub fn scroll_target(&mut self, id: Id, rect: Rect) -> Option<Point> {
        let visible = rect.intersect(&self.clip_rect());
        if !visible.is_empty() {
            self.st.scrolls.push(Hit { id, rect: visible, layer: self.st.layer });
        }
        if self.consumed {
            return None;
        }
        if let Some(Event::Wheel { pos, delta, .. }) = &self.event {
            // A popup above takes the wheel even where it has no scroll area of its own.
            let top_layer = self.st.hits_prev.iter().filter(|h| h.rect.contains(*pos)).map(|h| h.layer).max();
            if topmost(&self.st.scrolls_prev, *pos) == Some(id)
                && top_layer.is_none_or(|l| l <= self.st.layer)
            {
                self.consumed = true;
                return Some(*delta);
            }
        }
        None
    }

    pub fn is_hovered(&self, id: Id) -> bool {
        self.st.hovered == Some(id)
    }

    /// How long the mouse has rested on what it is over.
    pub fn hover_time(&self, id: Id) -> Option<Duration> {
        (self.st.hover_start.0 == Some(id)).then(|| self.st.hover_start.1.elapsed())
    }

    /// Shows a tooltip for a widget the mouse has rested on.
    pub fn tooltip(&mut self, response: &Response, text: &str) {
        const DELAY: Duration = Duration::from_millis(600);
        if !response.hovered || self.st.buttons[0] || text.is_empty() {
            return;
        }
        match self.hover_time(response.id) {
            Some(t) if t >= DELAY => self.st.tooltip = Some((text.to_string(), response.rect)),
            Some(t) => self.repaint_after(DELAY - t),
            None => {}
        }
    }

    pub fn has_focus(&self, id: Id) -> bool {
        self.st.focus == Some(id)
    }

    pub fn focus(&mut self, id: Id) {
        self.st.focus = Some(id);
    }

    /// A text field with the keyboard says so as it draws, so shortcuts that mean something in text (Ctrl+Z,
    /// Delete) are left to it.
    pub fn mark_text_input(&mut self, id: Id) {
        self.st.text_input = Some(id);
    }

    /// Whether the keyboard is in a text field (as of the last frame).
    pub fn focus_is_text(&self) -> bool {
        self.st.focus.is_some() && (self.st.text_input_prev == self.st.focus || self.st.text_input == self.st.focus)
    }

    // MARK: Text

    pub fn layout(&mut self, request: TextRequest) -> Rc<dyn TextLayout> {
        let key = request.cache_key();
        let frame = self.st.frame;
        if let Some(c) = self.st.cache.get_mut(&key) {
            if c.request == request {
                c.used = frame;
                return c.layout.clone();
            }
        }
        let layout = self.backend.layout(&request);
        self.st.cache.insert(key, CachedLayout { request, layout: layout.clone(), used: frame });
        layout
    }

    pub fn text_layout(&mut self, text: &str, style: TextStyle) -> Rc<dyn TextLayout> {
        self.layout(TextRequest { text: Arc::from(text), style, runs: Vec::new(), bold_ranges: Vec::new() })
    }

    pub fn text_width(&mut self, text: &str, font: Font) -> f32 {
        self.text_layout(text, TextStyle::new(font)).size().w
    }

    /// One line of text, vertically centred in `rect`, cut with an ellipsis if it does not fit.
    pub fn label(&mut self, rect: Rect, text: &str, font: Font, color: Color) -> Rc<dyn TextLayout> {
        self.label_with(rect, text, font, color, Align::Left, Overflow::Ellipsis)
    }

    pub fn label_with(
        &mut self,
        rect: Rect,
        text: &str,
        font: Font,
        color: Color,
        align: Align,
        overflow: Overflow,
    ) -> Rc<dyn TextLayout> {
        let mut style = TextStyle::new(font);
        style.max_width = Some(rect.w.max(1.0));
        style.overflow = overflow;
        let layout = self.text_layout(text, style);
        self.draw_layout(&layout, rect, color, align);
        layout
    }

    /// Text with colour runs, drawn as `label` draws.
    pub fn label_runs(&mut self, rect: Rect, text: &str, font: Font, color: Color, runs: Vec<ColorRun>) -> Rc<dyn TextLayout> {
        let mut style = TextStyle::new(font);
        style.max_width = Some(rect.w.max(1.0));
        style.overflow = Overflow::Ellipsis;
        let layout = self.layout(TextRequest { text: Arc::from(text), style, runs, bold_ranges: Vec::new() });
        self.draw_layout(&layout, rect, color, Align::Left);
        layout
    }

    pub fn draw_layout(&mut self, layout: &Rc<dyn TextLayout>, rect: Rect, color: Color, align: Align) {
        let size = layout.size();
        let x = match align {
            Align::Left => rect.x,
            Align::Center => rect.x + (rect.w - size.w) / 2.0,
            Align::Right => rect.right() - size.w,
        };
        let y = rect.y + ((rect.h - size.h) / 2.0).round();
        if self.is_visible(&Rect::new(x, y, size.w, size.h)) {
            self.painter.text(layout.as_ref(), Point::new(x, y), color);
        }
    }

    /// Byte ranges of `text` (for highlighting) given as char indices.
    pub fn byte_ranges(text: &str, positions: &[usize]) -> Vec<Range<u32>> {
        positions
            .iter()
            .filter_map(|&p| {
                let c = text[p..].chars().next()?;
                Some(p as u32..(p + c.len_utf8()) as u32)
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl UiState {
    /// What the last frame drew under `p`, for the shell's hit testing between frames.
    pub fn hit_at(&self, p: Point) -> Option<Id> {
        topmost(&self.hits, p)
    }
}

fn topmost(hits: &[Hit], p: Point) -> Option<Id> {
    let mut best: Option<&Hit> = None;
    for h in hits {
        if h.rect.contains(p) && best.is_none_or(|b| h.layer >= b.layer) {
            best = Some(h);
        }
    }
    best.map(|h| h.id)
}
