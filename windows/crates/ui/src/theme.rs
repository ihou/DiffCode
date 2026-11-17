//! Colours and metrics, light and dark, as the Mac app has them.

use diffcode_core::diff::HunkKind;
use diffcode_core::git::ChangeKind;
use diffcode_core::syntax::TokenKind;

use std::sync::Arc;

use crate::geom::Color;
use crate::paint::{Font, Weight};

/// Colours of the code canvas.
#[derive(Clone, Debug)]
pub struct CanvasColors {
    pub background: Color,
    pub text: Color,
    pub gutter_text: Color,
    pub gutter_text_strong: Color,
    pub syntax: [Color; TokenKind::COUNT],
    // Side by side follows the IDE convention: blue = modified, green = inserted, grey = deleted.
    pub modified: Color,
    pub modified_word: Color,
    pub modified_mark: Color,
    pub added: Color,
    pub added_word: Color,
    pub added_mark: Color,
    pub deleted: Color,
    pub deleted_word: Color,
    pub deleted_mark: Color,
    // The unified view uses the universal red/green.
    pub removed_line: Color,
    pub removed_word: Color,
    pub selection: Color,
    pub search_match: Color,
    pub search_current: Color,
    pub focus_line: Color,
    pub separator: Color,
    pub ruler_thumb: Color,
    pub accent: Color,
}

impl CanvasColors {
    pub fn line(&self, kind: HunkKind) -> Color {
        match kind {
            HunkKind::Modified => self.modified,
            HunkKind::Added => self.added,
            HunkKind::Deleted => self.deleted,
        }
    }

    pub fn word(&self, kind: HunkKind) -> Color {
        match kind {
            HunkKind::Modified => self.modified_word,
            HunkKind::Added => self.added_word,
            HunkKind::Deleted => self.deleted_word,
        }
    }

    pub fn mark(&self, kind: HunkKind) -> Color {
        match kind {
            HunkKind::Modified => self.modified_mark,
            HunkKind::Added => self.added_mark,
            HunkKind::Deleted => self.deleted_mark,
        }
    }

    pub fn token(&self, kind: TokenKind) -> Color {
        self.syntax[kind as usize]
    }

    fn dark() -> Self {
        let c = Color::hex;
        let a = Color::hexa;
        CanvasColors {
            background: c(0x1E1F22),
            text: c(0xBCBEC4),
            gutter_text: c(0x4E525A),
            gutter_text_strong: c(0xA1A5AD),
            syntax: [
                c(0xBCBEC4), // plain
                c(0xCF8E6D), // keyword
                c(0x6AAB73), // string
                c(0x7A7E85), // comment
                c(0x2AACB8), // number
                c(0xB3AE60), // annotation
                c(0x56A8F5), // function
                c(0xC77DBB), // constant
                c(0xD5B778), // tag
                c(0xBABABA), // attribute
            ],
            modified: a(0x3574F0, 0.20),
            modified_word: a(0x3574F0, 0.40),
            modified_mark: c(0x5C92F7),
            added: a(0x3FA65A, 0.20),
            added_word: a(0x3FA65A, 0.42),
            added_mark: c(0x57B46F),
            deleted: a(0x868A93, 0.20),
            deleted_word: a(0x868A93, 0.40),
            deleted_mark: c(0x8E929B),
            removed_line: a(0xE5534B, 0.17),
            removed_word: a(0xE5534B, 0.38),
            selection: a(0x2F65CA, 0.55),
            search_match: a(0xE8B339, 0.35),
            search_current: a(0xF5A623, 0.85),
            focus_line: a(0xFFFFFF, 0.06),
            separator: c(0x393B40),
            ruler_thumb: a(0xFFFFFF, 0.12),
            accent: c(0x548AF7),
        }
    }

    fn light() -> Self {
        let c = Color::hex;
        let a = Color::hexa;
        CanvasColors {
            background: c(0xFFFFFF),
            text: c(0x1F2329),
            gutter_text: c(0xB4B8C2),
            gutter_text_strong: c(0x5A5E69),
            syntax: [
                c(0x1F2329), // plain
                c(0x0033B3), // keyword
                c(0x067D17), // string
                c(0x8C8C8C), // comment
                c(0x1750EB), // number
                c(0x9E880D), // annotation
                c(0x00627A), // function
                c(0x871094), // constant
                c(0x0033B3), // tag
                c(0x174AD4), // attribute
            ],
            modified: a(0x3574F0, 0.13),
            modified_word: a(0x3574F0, 0.28),
            modified_mark: c(0x3574F0),
            added: a(0x2DA44E, 0.15),
            added_word: a(0x2DA44E, 0.32),
            added_mark: c(0x2DA44E),
            deleted: a(0x6E7481, 0.14),
            deleted_word: a(0x6E7481, 0.28),
            deleted_mark: c(0x868B97),
            removed_line: a(0xE5534B, 0.12),
            removed_word: a(0xE5534B, 0.28),
            selection: a(0x3574F0, 0.28),
            search_match: a(0xF5C542, 0.45),
            search_current: a(0xF59E0B, 0.90),
            focus_line: a(0x000000, 0.045),
            separator: c(0xEBECF0),
            ruler_thumb: a(0x000000, 0.10),
            accent: c(0x3574F0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    /// Font families picked in Settings for code and for terminals; None: the default monospace face.
    pub code_family: Option<Arc<str>>,
    pub terminal_family: Option<Arc<str>>,
    /// Interface font size / 13: lengths written for the default size grow with it.
    pub scale: f32,
    pub code_size: f32,
    pub window: Color,
    pub sidebar: Color,
    pub panel: Color,
    pub elevated: Color,
    pub border: Color,
    pub hairline: Color,
    pub text: Color,
    pub text2: Color,
    pub text3: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub hover: Color,
    pub pressed: Color,
    pub selection: Color,
    pub chip: Color,
    pub added: Color,
    pub removed: Color,
    pub warning: Color,
    pub shadow: Color,
    pub canvas: CanvasColors,
}

pub const BASE_UI_SIZE: f32 = 13.0;

impl Theme {
    pub fn new(dark: bool, ui_size: f32, code_size: f32) -> Theme {
        let d = |light: u32, dark_: u32| Color::hex(if dark { dark_ } else { light });
        let da = |light: u32, la: f32, dark_: u32, dalpha: f32| {
            if dark {
                Color::hexa(dark_, dalpha)
            } else {
                Color::hexa(light, la)
            }
        };
        Theme {
            dark,
            code_family: None,
            terminal_family: None,
            scale: ui_size / BASE_UI_SIZE,
            code_size,
            window: d(0xFFFFFF, 0x1E1F22),
            sidebar: d(0xF0F1F4, 0x232428),
            panel: d(0xF7F8FA, 0x2B2D30),
            elevated: d(0xFFFFFF, 0x2F3136),
            border: d(0xE1E3E8, 0x14151A),
            hairline: d(0xEBECF0, 0x393B40),
            text: d(0x1F2329, 0xDFE1E5),
            text2: d(0x6C707E, 0x9DA0A8),
            text3: d(0xA2A6B3, 0x868A91),
            accent: d(0x3574F0, 0x548AF7),
            on_accent: Color::WHITE,
            hover: da(0x000000, 0.05, 0xFFFFFF, 0.06),
            pressed: da(0x000000, 0.09, 0xFFFFFF, 0.10),
            selection: da(0x3574F0, 0.16, 0x548AF7, 0.28),
            chip: da(0x000000, 0.06, 0xFFFFFF, 0.09),
            added: d(0x1E8A34, 0x73BD79),
            removed: d(0xC7392F, 0xE5736B),
            warning: d(0xB7791F, 0xE5B567),
            shadow: Color::hexa(0x000000, if dark { 0.45 } else { 0.18 }),
            canvas: if dark { CanvasColors::dark() } else { CanvasColors::light() },
        }
    }

    /// A length written for the default interface size, scaled, in whole DIPs.
    pub fn s(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }

    /// The interface face at a size written for the default interface size.
    pub fn font(&self, size: f32) -> Font {
        Font::ui(size * self.scale)
    }

    pub fn font_w(&self, size: f32, weight: Weight) -> Font {
        Font::ui(size * self.scale).weight(weight)
    }

    /// The monospace face in the interface (hashes, paths in search results).
    pub fn mono(&self, size: f32) -> Font {
        Font::mono(size * self.scale)
    }

    pub fn code_font(&self) -> Font {
        Font::mono(self.code_size)
    }

    pub fn status(&self, kind: Option<ChangeKind>) -> Color {
        let d = |light: u32, dark_: u32| Color::hex(if self.dark { dark_ } else { light });
        match kind {
            Some(ChangeKind::Modified) => d(0x1F5FD0, 0x6CA6F5),
            Some(ChangeKind::Added) => self.added,
            Some(ChangeKind::Deleted) => d(0x7A7E87, 0x8C9199),
            Some(ChangeKind::Renamed) => d(0x17858A, 0x4EC9B0),
            Some(ChangeKind::Untracked) => d(0xB3402E, 0xE0766B),
            Some(ChangeKind::Conflicted) => d(0xD12F2F, 0xF26C6C),
            None => self.text,
        }
    }

    /// Colours of the commit graph's lanes and of project badges.
    pub fn lane(&self, index: u32) -> Color {
        const DARK: [u32; 8] = [0x548AF7, 0x5FB865, 0xE5A84B, 0xC27BD6, 0xE5736B, 0x4EC9B0, 0xD6C35A, 0x8C9BF2];
        const LIGHT: [u32; 8] = [0x3574F0, 0x2DA44E, 0xC9821D, 0x9A4FC0, 0xC7392F, 0x17858A, 0xA88B0E, 0x5A6AD8];
        let table = if self.dark { &DARK } else { &LIGHT };
        Color::hex(table[index as usize % table.len()])
    }
}
