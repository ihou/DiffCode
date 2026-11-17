//! Context menus and menus dropped from buttons, drawn over everything but dialogs.

use crate::app::{App, Menu, MenuItem};
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::input::{Event, Key, Modifiers};
use crate::paint::Overflow;
use crate::ui::{layer, Align, Id, Ui};
use crate::widgets::vscrollbar;

impl App {
    /// Opens a menu with its top left at `at` (moved in to fit the window).
    pub fn open_menu(&mut self, at: Point, items: Vec<MenuItem>) {
        if items.iter().all(|i| i.separator) {
            return;
        }
        // No separator at either end or twice in a row.
        let mut tidy: Vec<MenuItem> = Vec::new();
        for item in items {
            if item.separator && tidy.last().is_none_or(|l| l.separator) {
                continue;
            }
            tidy.push(item);
        }
        while tidy.last().is_some_and(|l| l.separator) {
            tidy.pop();
        }
        self.menu = Some(Menu::new(at, tidy));
    }

    /// Opens a menu of choices scrolled to the current one (the ticked item), when it is too long for the window.
    pub fn open_choice_menu(&mut self, at: Point, items: Vec<MenuItem>) {
        self.open_menu(at, items);
        if let Some(menu) = self.menu.as_mut() {
            menu.reveal = menu.items.iter().position(|i| i.checked);
        }
    }
}

pub fn menu(app: &mut App, ui: &mut Ui) {
    let Some(menu) = app.menu.as_mut() else { return };
    let t = ui.theme;
    let row_h = t.s(26.0);
    let sep_h = t.s(9.0);
    let pad = t.s(5.0);
    let font = t.font(12.5);
    let short_font = t.font(12.0);
    let check_w = if menu.items.iter().any(|i| i.checked) { t.s(22.0) } else { t.s(10.0) };
    let mut title_w: f32 = 0.0;
    let mut short_w: f32 = 0.0;
    for item in &menu.items {
        if !item.separator {
            title_w = title_w.max(ui.text_width(&item.title, font).ceil());
            if !item.shortcut.is_empty() {
                short_w = short_w.max(ui.text_width(&item.shortcut, short_font).ceil());
            }
        }
    }
    let w = (check_w + title_w + if short_w > 0.0 { t.s(28.0) + short_w } else { 0.0 } + t.s(18.0) + pad * 2.0)
        .max(t.s(180.0))
        .min(ui.size.w - 16.0);
    let height = |i: &MenuItem| if i.separator { sep_h } else { row_h };
    let content: f32 = menu.items.iter().map(height).sum::<f32>() + pad * 2.0;
    let bounds = ui.bounds().inset(6.0);
    // Taller than the window (a long list of fonts), it scrolls.
    let h = content.min(bounds.h);
    let mut rect = Rect::new(menu.at.x, menu.at.y, w, h);
    if rect.bottom() > bounds.bottom() {
        // Opens upwards when there is no room below.
        rect.y = (menu.at.y - h).max(bounds.y);
    }
    let rect = rect.clamped_into(&bounds).snapped(ui.painter.scale());
    if let Some(i) = menu.reveal.take().filter(|i| *i < menu.items.len()) {
        let top: f32 = menu.items[..i].iter().map(height).sum();
        menu.scroll.reveal(top, row_h + pad * 2.0, rect.h);
    }
    menu.scroll.clamp(content, rect.h);

    let mut activate: Option<usize> = None;
    let mut close = false;

    ui.with_layer(layer::MENU, |ui| {
        // A press anywhere outside closes it, and goes no further.
        let back = ui.block(Id::new("menu-back"), ui.bounds());
        if back.pressed || back.secondary || back.middle {
            close = true;
        }
        ui.painter.shadow(rect, t.s(8.0), t.s(16.0), t.shadow);
        ui.round(rect, t.s(8.0), t.elevated);
        ui.painter.stroke_round_rect(rect.inset(0.5), t.s(8.0), 1.0, t.hairline);
        ui.block(Id::new("menu-panel"), rect);
        if let Some(delta) = ui.scroll_target(Id::new("menu-scroll"), rect) {
            menu.scroll.offset += delta.y;
            menu.scroll.clamp(content, rect.h);
        }
        let scrolls = content > rect.h + 0.5;
        if scrolls {
            ui.push_clip(rect.inset(1.0));
        }
        let mut y = rect.y + pad - menu.scroll.offset;
        let moved = matches!(ui.event, Some(Event::MouseMove(_)));
        for (i, item) in menu.items.iter().enumerate() {
            if y > rect.bottom() || y + height(item) < rect.y {
                y += height(item);
                continue;
            }
            if item.separator {
                ui.hline(rect.x + pad + t.s(6.0), y + sep_h / 2.0, rect.w - pad * 2.0 - t.s(12.0), t.hairline);
                y += sep_h;
                continue;
            }
            let row = Rect::new(rect.x + pad, y, rect.w - pad * 2.0, row_h);
            let r = ui.interact(Id::new("menu-item").with(i), row);
            if r.hovered && moved {
                menu.hovered = item.enabled.then_some(i);
            }
            let lit = item.enabled && (menu.hovered == Some(i) || r.hovered);
            if lit {
                ui.round(row, t.s(5.0), t.selection);
            }
            let color = if item.enabled { t.text } else { t.text3 };
            if item.checked {
                let s = t.s(13.0);
                ui.icon(Icon::Checkmark, Rect::new(row.x + t.s(6.0), row.center().y - s / 2.0, s, s), color);
            }
            let tx = row.x + check_w;
            let short_space = if short_w > 0.0 { short_w + t.s(28.0) } else { 0.0 };
            ui.label_with(Rect::new(tx, row.y, row.right() - tx - short_space - t.s(8.0), row.h), &item.title, font, color, Align::Left, Overflow::Ellipsis);
            if !item.shortcut.is_empty() {
                ui.label_with(
                    Rect::new(row.right() - t.s(10.0) - short_w, row.y, short_w, row.h),
                    &item.shortcut,
                    short_font,
                    if item.enabled { t.text2 } else { t.text3 },
                    Align::Right,
                    Overflow::Clip,
                );
            }
            if r.clicked && item.enabled {
                activate = Some(i);
            }
            y += row_h;
        }
        if scrolls {
            ui.pop_clip();
            vscrollbar(ui, Id::new("menu-scrollbar"), rect.inset(t.s(2.0)), content - t.s(4.0), &mut menu.scroll);
        }
    });

    finish(app, activate, close);
}

/// The menu's keys, taken before anything under it sees them: the arrows move, Enter picks, Escape closes.
pub fn menu_keys(app: &mut App, ui: &mut Ui) {
    let Some(menu) = app.menu.as_mut() else { return };
    let mut activate: Option<usize> = None;
    let mut close = false;
    let enabled: Vec<usize> = menu.items.iter().enumerate().filter(|(_, i)| i.enabled && !i.separator).map(|(i, _)| i).collect();
    if let Some((key, mods)) = ui.key() {
        if mods == Modifiers::NONE || mods == Modifiers::SHIFT {
            let pos = menu.hovered.and_then(|h| enabled.iter().position(|e| *e == h));
            match key {
                Key::Down if !enabled.is_empty() => {
                    menu.hovered = Some(enabled[pos.map_or(0, |p| (p + 1) % enabled.len())]);
                    menu.reveal = menu.hovered;
                    ui.consume();
                }
                Key::Up if !enabled.is_empty() => {
                    menu.hovered = Some(enabled[pos.map_or(enabled.len() - 1, |p| (p + enabled.len() - 1) % enabled.len())]);
                    menu.reveal = menu.hovered;
                    ui.consume();
                }
                Key::Enter | Key::Space => {
                    activate = menu.hovered;
                    close = activate.is_none();
                    ui.consume();
                }
                Key::Escape => {
                    close = true;
                    ui.consume();
                }
                _ => {}
            }
        }
        if !ui.is_consumed() {
            // Any other key closes the menu and goes on to whatever it is for.
            close = true;
        }
    }

    finish(app, activate, close);
}

fn finish(app: &mut App, activate: Option<usize>, close: bool) {
    if let Some(i) = activate {
        let action = app.menu.as_mut().and_then(|m| m.items.get_mut(i)).and_then(|item| item.action.take());
        app.menu = None;
        if let Some(action) = action {
            action(app);
        }
    } else if close {
        app.menu = None;
    }
}
