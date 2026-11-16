//! Input events as the shell delivers them, platform-neutral.

use std::path::PathBuf;

use crate::geom::Point;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Ctrl on Windows (⌘ in the Mac harness): the shortcut modifier.
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { ctrl: false, shift: false, alt: false };
    pub const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false };
    pub const SHIFT: Modifiers = Modifiers { ctrl: false, shift: true, alt: false };
    pub const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false };
    pub const ALT: Modifiers = Modifiers { ctrl: false, shift: false, alt: true };

    pub fn none(&self) -> bool {
        !self.ctrl && !self.shift && !self.alt
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    /// Shift on its own (for double Shift); no widget acts on it.
    Shift,
    /// A letter (lowercase), digit or punctuation key, as on a US layout.
    Char(char),
    F(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    MouseMove(Point),
    /// `clicks`: 1, 2 for a double click, 3 for a triple click.
    MouseDown { pos: Point, button: MouseButton, clicks: u8, mods: Modifiers },
    MouseUp { pos: Point, button: MouseButton, mods: Modifiers },
    MouseLeave,
    /// In DIPs; positive `y` moves the content up (scrolls towards the end), as a wheel turned towards the user.
    Wheel { pos: Point, delta: Point, mods: Modifiers },
    Key { key: Key, mods: Modifiers, repeat: bool },
    /// Text typed or committed by an input method.
    Text(String),
    /// An input method's composition so far, with its caret (byte offset); empty text ends it.
    Preedit { text: String, caret: Option<usize> },
    Files(Vec<PathBuf>),
    /// Files are being dragged over the window, here; None when they leave it or are dropped.
    FilesHover(Option<Point>),
    FocusChanged(bool),
}

impl Event {
    /// Moves and wheel turns are coalesced; everything else is handled one at a time.
    pub fn is_discrete(&self) -> bool {
        !matches!(self, Event::MouseMove(_))
    }
}

/// Text editing commands, the same whichever widget has the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    Left { word: bool, select: bool },
    Right { word: bool, select: bool },
    Up { select: bool },
    Down { select: bool },
    Home { select: bool },
    End { select: bool },
    DocStart { select: bool },
    DocEnd { select: bool },
    PageUp { select: bool },
    PageDown { select: bool },
    Backspace { word: bool },
    Delete { word: bool },
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Newline,
    Tab { back: bool },
}

impl EditCommand {
    /// The editing command a key press means on Windows.
    pub fn from_key(key: Key, m: Modifiers) -> Option<EditCommand> {
        let select = m.shift;
        let word = m.ctrl;
        Some(match key {
            Key::Left => EditCommand::Left { word, select },
            Key::Right => EditCommand::Right { word, select },
            Key::Up if !m.ctrl => EditCommand::Up { select },
            Key::Down if !m.ctrl => EditCommand::Down { select },
            Key::Home if m.ctrl => EditCommand::DocStart { select },
            Key::End if m.ctrl => EditCommand::DocEnd { select },
            Key::Home => EditCommand::Home { select },
            Key::End => EditCommand::End { select },
            Key::PageUp => EditCommand::PageUp { select },
            Key::PageDown => EditCommand::PageDown { select },
            Key::Backspace => EditCommand::Backspace { word },
            Key::Delete if m.shift && !m.ctrl => EditCommand::Cut,
            Key::Delete => EditCommand::Delete { word },
            Key::Insert if m.ctrl => EditCommand::Copy,
            Key::Insert if m.shift => EditCommand::Paste,
            Key::Char('a') if m.ctrl && !m.shift && !m.alt => EditCommand::SelectAll,
            Key::Char('c') if m.ctrl && !m.shift && !m.alt => EditCommand::Copy,
            Key::Char('x') if m.ctrl && !m.shift && !m.alt => EditCommand::Cut,
            Key::Char('v') if m.ctrl && !m.shift && !m.alt => EditCommand::Paste,
            Key::Char('z') if m.ctrl && !m.shift && !m.alt => EditCommand::Undo,
            Key::Char('z') if m.ctrl && m.shift && !m.alt => EditCommand::Redo,
            Key::Char('y') if m.ctrl && !m.shift && !m.alt => EditCommand::Redo,
            Key::Enter if !m.ctrl && !m.alt => EditCommand::Newline,
            Key::Tab if !m.ctrl && !m.alt => EditCommand::Tab { back: m.shift },
            _ => return None,
        })
    }
}
