//! A question or notice in DiffCode's own style: what it is about, the files it touches, and clear buttons.
//! Enter answers yes, Esc (or a click outside) no.

use super::components::{self, ViewState};
use crate::app::dialog::DialogStyle;
use crate::app::{App, MenuItem};
use crate::core::lang::l;
use crate::core::lf;
use crate::geom::{Color, Rect};
use crate::input::Key;
use crate::paint::{Overflow, TextStyle, Weight};
use crate::platform::Platform;
use crate::text_field::FieldOutcome;
use crate::ui::{layer, Align, Id, Ui};
use crate::widgets::{draw_checkbox, scroll_area, Check};

const PAD: f32 = 20.0;

fn input_id() -> Id {
    Id::new("dialog-input")
}

pub fn dialog(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform) {
    if app.dialog.is_none() {
        return;
    }
    let t = ui.theme;
    ui.with_layer(layer::DIALOG, |ui| {
        let bounds = ui.bounds();
        ui.fill(bounds, Color::BLACK.alpha(0.28));
        if ui.block(Id::new("dialog-back"), bounds).pressed {
            app.answer_dialog(false);
            return;
        }
        if std::mem::take(&mut app.dialog_focus) {
            let has_input = app.dialog.as_ref().is_some_and(|d| d.input.is_some());
            ui.focus(if has_input { input_id() } else { crate::app::ids::dialog() });
        }

        // Keys: the field keeps Enter and Esc for itself while it has the keyboard.
        let field_focused = ui.has_focus(input_id());
        let mut answer = None;
        if let Some((key, mods)) = ui.key() {
            let d = app.dialog.as_ref().expect("dialog");
            match key {
                Key::Enter if mods.none() && !field_focused => answer = Some(true),
                Key::Escape if mods.none() && !field_focused => answer = Some(false),
                Key::Down | Key::Up if mods.none() && !d.choices.is_empty() => {
                    let step = if key == Key::Down { 1 } else { -1 };
                    let pick = d.choice as isize + step;
                    ui.consume();
                    app.pick_dialog_choice(pick);
                }
                _ => {}
            }
            if answer.is_some() {
                ui.consume();
            }
        }
        if let Some(confirmed) = answer {
            app.answer_dialog(confirmed);
            return;
        }

        let w = t.s(440.0).min(bounds.w - 48.0);
        let inner_w = w - t.s(PAD) * 2.0;
        let d = app.dialog.as_ref().expect("dialog");
        let tint = match d.style {
            DialogStyle::Destructive => t.removed,
            DialogStyle::Caution => t.warning,
            DialogStyle::Info => t.accent,
        };

        // Measure first: the panel is centred in the window.
        let glyph = t.s(38.0);
        let text_w = inner_w - glyph - t.s(14.0);
        let wrap = |font, width: f32| {
            let mut s = TextStyle::new(font);
            s.max_width = Some(width.max(1.0));
            s.overflow = Overflow::Wrap;
            s
        };
        let title = ui.text_layout(&d.title, wrap(t.font_w(15.0, Weight::Semibold), text_w));
        let message = (!d.message.is_empty()).then(|| ui.text_layout(&d.message, wrap(t.font(12.5), text_w)));
        let text_h = title.size().h + message.as_ref().map_or(0.0, |m| t.s(6.0) + m.size().h);
        let header_h = text_h.max(glyph);

        let item_h = t.s(30.0);
        let items_h = if d.items.is_empty() { 0.0 } else { t.s(16.0) + d.items.len() as f32 * item_h + if d.hidden_count > 0 { t.s(28.0) } else { 0.0 } };

        let choice_text_w = inner_w - t.s(12.0) * 2.0 - t.s(14.0) - t.s(10.0);
        let choices: Vec<_> = d
            .choices
            .iter()
            .map(|c| {
                let title = ui.text_layout(&c.title, wrap(t.font_w(12.5, Weight::Semibold), choice_text_w));
                let detail = (!c.detail.is_empty()).then(|| ui.text_layout(&c.detail, wrap(t.font(11.5), choice_text_w)));
                let h = t.s(8.0) * 2.0 + title.size().h + detail.as_ref().map_or(0.0, |x| t.s(2.0) + x.size().h);
                (title, detail, h)
            })
            .collect();
        let choices_h = if choices.is_empty() { 0.0 } else { t.s(14.0) + choices.iter().map(|c| c.2).sum::<f32>() };

        let field_h = t.s(30.0);
        let problem_h = t.s(15.0);
        let input_h = if d.input.is_some() { t.s(14.0) + field_h + t.s(5.0) + problem_h } else { 0.0 };
        let option_h = if d.option.is_some() { (if d.input.is_none() { t.s(14.0) } else { t.s(2.0) }) + t.s(20.0) } else { 0.0 };

        let output = d.output.as_ref().filter(|o| !o.is_empty()).map(|o| {
            let mut style = wrap(t.code_font().size(11.5 * t.scale), inner_w - t.s(10.0) * 2.0);
            style.tab_width = 4;
            ui.text_layout(o, style)
        });
        let output_content = output.as_ref().map_or(0.0, |o| o.size().h + t.s(10.0) * 2.0);
        let output_h = if output.is_some() { t.s(14.0) + output_content.min(t.s(180.0)) } else { 0.0 };

        let button_h = t.s(30.0);
        let buttons_h = t.s(20.0) + button_h + t.s(18.0);
        let total = t.s(PAD) + header_h + items_h + choices_h + input_h + option_h + output_h + buttons_h;
        let h = total.min(bounds.h - 48.0);
        let rect = Rect::new((bounds.w - w) / 2.0, ((bounds.h - h) / 2.0).max(24.0), w, h).snapped(ui.painter.scale());
        components::popup_panel(ui, rect, t.s(14.0));
        let panel = ui.block(crate::app::ids::dialog(), rect);
        if panel.pressed && !ui.has_focus(input_id()) {
            ui.focus(crate::app::ids::dialog());
        }

        let x = rect.x + t.s(PAD);
        let mut y = rect.y + t.s(PAD);

        // The icon, the title and the message. A title alone sits level with the icon.
        let g = Rect::new(x, y + if message.is_none() { (header_h - glyph) / 2.0 } else { 0.0 }, glyph, glyph);
        ui.round(g, glyph / 2.0, tint.alpha(0.15));
        let gi = t.s(18.0);
        ui.icon(d.icon, Rect::new(g.center().x - gi / 2.0, g.center().y - gi / 2.0, gi, gi), tint);
        let tx = g.right() + t.s(14.0);
        let ty = if message.is_none() { y + (header_h - title.size().h) / 2.0 } else { y };
        ui.draw_layout(&title, Rect::new(tx, ty, text_w, title.size().h), t.text, Align::Left);
        if let Some(m) = &message {
            let my = ty + title.size().h + t.s(6.0);
            ui.draw_layout(m, Rect::new(tx, my, text_w, m.size().h), t.text2, Align::Left);
        }
        y += header_h;

        // The files it touches.
        if !d.items.is_empty() {
            y += t.s(16.0);
            let list = Rect::new(x, y, inner_w, items_h - t.s(16.0));
            ui.round(list, t.s(9.0), t.chip);
            for (i, item) in d.items.iter().enumerate() {
                let row = Rect::new(x, y + i as f32 * item_h, inner_w, item_h);
                if i > 0 {
                    ui.hline(row.x + t.s(36.0), row.y, row.w - t.s(36.0), t.hairline);
                }
                let r = row.inset_xy(t.s(12.0), 0.0);
                let slot = Rect::new(r.x, r.y, t.s(17.0), r.h);
                match (item.icon, item.kind) {
                    (Some(icon), _) => {
                        let s = t.s(12.0);
                        ui.icon(icon, Rect::new(slot.center().x - s / 2.0, slot.center().y - s / 2.0, s, s), t.text2);
                    }
                    (None, Some(_)) => components::file_icon(ui, slot, &item.title, 12.0),
                    (None, None) => {
                        let s = t.s(12.0);
                        ui.icon(crate::icons::Icon::Document, Rect::new(slot.center().x - s / 2.0, slot.center().y - s / 2.0, s, s), t.text2);
                    }
                }
                let mut right = r.right();
                if let Some(kind) = item.kind {
                    let lw = t.s(12.0);
                    ui.label_with(Rect::new(right - lw, r.y, lw, r.h), kind.letter(), t.font_w(10.5, Weight::Bold), t.status(Some(kind)), Align::Center, Overflow::Clip);
                    right -= lw + t.s(4.0);
                }
                let nx = slot.right() + t.s(8.0);
                let font = t.font_w(12.5, Weight::Medium);
                let nw = ui.text_width(&item.title, font).ceil().min((right - nx).max(0.0));
                let color = item.kind.map_or(t.text, |k| t.status(Some(k)));
                ui.label_with(Rect::new(nx, r.y, nw + 1.0, r.h), &item.title, font, color, Align::Left, Overflow::Ellipsis);
                let dx = nx + nw + t.s(8.0);
                if !item.detail.is_empty() && right - dx > t.s(16.0) {
                    ui.label_with(Rect::new(dx, r.y, right - dx - t.s(4.0), r.h), &item.detail, t.font(11.5), t.text3, Align::Left, Overflow::EllipsisMiddle);
                }
                let hit = ui.interact(Id::new("dialog-item").with(i), row);
                let tip = if item.kind.is_some() && !item.detail.is_empty() { format!("{}/{}", item.detail, item.title) } else if item.detail.is_empty() { item.title.clone() } else { item.detail.clone() };
                ui.tooltip(&hit, &tip);
            }
            if d.hidden_count > 0 {
                let row = Rect::new(x, y + d.items.len() as f32 * item_h, inner_w, t.s(28.0));
                ui.hline(row.x, row.y, row.w, t.hairline);
                let count = d.hidden_count;
                ui.label(row.inset_xy(t.s(12.0), 0.0), &lf!("…and {count} more", "…以及另外 {count} 个"), t.font(11.5), t.text3);
            }
            y += items_h - t.s(16.0);
        }

        // The ways to do it.
        let mut pick = None;
        if !choices.is_empty() {
            y += t.s(14.0);
            let list = Rect::new(x, y, inner_w, choices_h - t.s(14.0));
            ui.round(list, t.s(9.0), t.chip);
            for (i, (title, detail, ch)) in choices.iter().enumerate() {
                let row = Rect::new(x, y, inner_w, *ch);
                if i > 0 {
                    ui.hline(row.x + t.s(36.0), row.y, row.w - t.s(36.0), t.hairline);
                }
                let hit = ui.interact(Id::new("dialog-choice").with(i), row);
                if hit.hovered {
                    ui.round(row.inset_xy(t.s(3.0), t.s(3.0)), t.s(7.0), t.hover.alpha(0.6));
                }
                let picked = i == d.choice;
                let rs = t.s(14.0);
                let radio = Rect::new(row.x + t.s(12.0), row.y + t.s(8.0) + t.s(1.5), rs, rs);
                if picked {
                    ui.round(radio, rs / 2.0, t.accent);
                    let dot = t.s(5.0);
                    ui.round(Rect::new(radio.center().x - dot / 2.0, radio.center().y - dot / 2.0, dot, dot), dot / 2.0, Color::WHITE);
                } else {
                    ui.painter.stroke_round_rect(radio.inset_xy(0.6, 0.6), rs / 2.0, t.s(1.2), t.text3);
                }
                let cx = radio.right() + t.s(10.0);
                let cy = row.y + t.s(8.0);
                ui.draw_layout(title, Rect::new(cx, cy, choice_text_w, title.size().h), t.text, Align::Left);
                if let Some(dl) = detail {
                    ui.draw_layout(dl, Rect::new(cx, cy + title.size().h + t.s(2.0), choice_text_w, dl.size().h), t.text2, Align::Left);
                }
                if hit.clicked {
                    pick = Some(i);
                }
                y += ch;
            }
        }

        // A line to type.
        let mut submit = false;
        let mut cancel = false;
        let mut blocked = false;
        if d.input.is_some() {
            y += t.s(14.0);
            let field = Rect::new(x, y, inner_w, field_h);
            let id = input_id();
            let input = app.dialog.as_mut().and_then(|d| d.input.as_mut()).expect("input");
            match components::field_input(ui, platform, id, &mut input.field) {
                FieldOutcome::Submit => submit = true,
                FieldOutcome::Cancel => cancel = true,
                _ => {}
            }
            let text = input.field.text.trim().to_string();
            let problem = if text.is_empty() { None } else { (input.problem)(&text) };
            blocked = (input.problem)(&text).is_some();
            let focused = ui.has_focus(id);
            ui.round(field, t.s(7.0), t.chip);
            let border = if problem.is_some() {
                Some(t.removed.alpha(0.8))
            } else if focused {
                Some(t.accent.alpha(0.7))
            } else {
                None
            };
            if let Some(c) = border {
                ui.painter.stroke_round_rect(field.inset_xy(0.75, 0.75), t.s(7.0), t.s(1.5), c);
            }
            let inner = field.inset_xy(t.s(10.0), 0.0);
            let r = ui.interact(id, inner);
            if r.hovered {
                ui.set_cursor(crate::paint::CursorIcon::Text);
            }
            let placeholder = input.placeholder.clone();
            input.field.draw_text(ui, id, inner, &placeholder, t.font(13.0), focused, &r);
            y += field_h + t.s(5.0);
            if let Some(p) = problem {
                ui.label(Rect::new(x, y, inner_w, problem_h), &p, t.font(11.0), t.removed);
            }
            y += problem_h;
        }
        let d = app.dialog.as_ref().expect("dialog");

        // A box to tick.
        let mut toggle = false;
        if let Some(option) = &d.option {
            y += if d.input.is_none() { t.s(14.0) } else { t.s(2.0) };
            let font = t.font(12.5);
            let tw = ui.text_width(&option.title, font).ceil();
            let row = Rect::new(x, y, t.s(20.0) + tw, t.s(20.0));
            let hit = ui.interact(Id::new("dialog-option"), row);
            draw_checkbox(ui, Rect::new(x, y, t.s(16.0), t.s(20.0)), if option.is_on { Check::On } else { Check::Off }, true, hit.hovered);
            ui.label_with(Rect::new(x + t.s(22.0), y, tw + 1.0, row.h), &option.title, font, t.text, Align::Left, Overflow::Clip);
            if hit.clicked {
                toggle = true;
            }
            y += t.s(20.0);
        }

        // A program's own words.
        let mut copy_menu = None;
        if let Some(layout) = &output {
            y += t.s(14.0);
            let area = Rect::new(x, y, inner_w, output_h - t.s(14.0));
            ui.round(area, t.s(9.0), t.chip);
            let scroll = vs.scroll(Id::new("dialog-output"));
            scroll_area(ui, Id::new("dialog-output"), area, output_content, scroll);
            let offset = scroll.offset;
            ui.clipped(area, |ui| {
                let r = Rect::new(area.x + t.s(10.0), area.y + t.s(10.0) - offset, inner_w - t.s(20.0), layout.size().h);
                ui.draw_layout(layout, r, t.text2, Align::Left);
            });
            let hit = ui.interact(Id::new("dialog-output-hit"), area);
            if hit.secondary {
                copy_menu = Some(ui.mouse());
            }
        }

        // The buttons, right to left: confirm, alternate, cancel.
        let by = rect.bottom() - t.s(18.0) - button_h;
        let mut right = rect.right() - t.s(PAD);
        let destructive = d.is_destructive();
        let (confirm_rect, confirm_w) = dialog_button_rect(ui, right, by, &d.confirm_title, "↵");
        right -= confirm_w + t.s(10.0);
        let alternate = d.alternate_title.clone().map(|a| {
            let (r, w) = dialog_button_rect(ui, right, by, &a, "");
            right -= w + t.s(10.0);
            (r, a)
        });
        let cancel_button = d.cancel_title.clone().map(|c| {
            let (r, _) = dialog_button_rect(ui, right, by, &c, "Esc");
            (r, c)
        });
        let confirm_title = d.confirm_title.clone();
        let kind = if destructive { ButtonStyle::Destructive } else { ButtonStyle::Primary };
        if dialog_button(ui, Id::new("dialog-confirm"), confirm_rect, &confirm_title, "↵", kind, !blocked) {
            submit = true;
        }
        let mut alt = false;
        if let Some((r, title)) = alternate {
            alt = dialog_button(ui, Id::new("dialog-alternate"), r, &title, "", ButtonStyle::Plain, true);
        }
        if let Some((r, title)) = cancel_button {
            if dialog_button(ui, Id::new("dialog-cancel"), r, &title, "Esc", ButtonStyle::Plain, true) {
                cancel = true;
            }
        }

        if let Some(i) = pick {
            app.pick_dialog_choice(i as isize);
        }
        if toggle {
            app.toggle_dialog_option();
        }
        if let Some(at) = copy_menu {
            let text = app.dialog.as_ref().and_then(|d| d.output.clone()).unwrap_or_default();
            app.open_menu(at, vec![MenuItem::new(l("Copy", "复制"), move |app| app.copy_text(text))]);
        }
        if alt {
            app.answer_dialog_alternate();
        } else if submit {
            app.answer_dialog(true);
        } else if cancel {
            app.answer_dialog(false);
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum ButtonStyle {
    Plain,
    Primary,
    Destructive,
}

fn dialog_button_rect(ui: &mut Ui, right: f32, y: f32, title: &str, keys: &str) -> (Rect, f32) {
    let t = ui.theme;
    let tw = ui.text_width(title, t.font_w(12.5, Weight::Semibold)).ceil();
    let kw = if keys.is_empty() { 0.0 } else { ui.text_width(keys, t.font_w(10.5, Weight::Medium)).ceil() + t.s(7.0) };
    let w = (tw + kw + t.s(14.0) * 2.0).max(t.s(84.0));
    (Rect::new(right - w, y, w, t.s(30.0)), w)
}

/// One of the dialog's buttons: its title and the key that presses it. Returns whether it was clicked.
fn dialog_button(ui: &mut Ui, id: Id, rect: Rect, title: &str, keys: &str, style: ButtonStyle, enabled: bool) -> bool {
    let t = ui.theme;
    let hit = ui.interact(id, rect);
    let hovering = hit.hovered && enabled;
    let fill = match style {
        ButtonStyle::Plain => {
            if hovering {
                t.text.alpha(0.14)
            } else {
                t.chip
            }
        }
        ButtonStyle::Primary => t.accent.alpha(if hovering { 0.85 } else { 1.0 }),
        ButtonStyle::Destructive => Color::hex(0xD9453C).alpha(if hovering { 0.85 } else { 1.0 }),
    };
    let alpha = if enabled { 1.0 } else { 0.45 };
    ui.round(rect, t.s(8.0), fill.alpha(alpha));
    let color = if style == ButtonStyle::Plain { t.text } else { Color::WHITE };
    let tf = t.font_w(12.5, Weight::Semibold);
    let kf = t.font_w(10.5, Weight::Medium);
    let tw = ui.text_width(title, tf).ceil();
    let kw = if keys.is_empty() { 0.0 } else { ui.text_width(keys, kf).ceil() + t.s(7.0) };
    let x = rect.x + (rect.w - tw - kw) / 2.0;
    ui.label_with(Rect::new(x, rect.y, tw + 1.0, rect.h), title, tf, color.alpha(alpha), Align::Left, Overflow::Clip);
    if !keys.is_empty() {
        ui.label_with(Rect::new(x + tw + t.s(7.0), rect.y, kw, rect.h), keys, kf, color.alpha(0.6 * alpha), Align::Left, Overflow::Clip);
    }
    enabled && hit.clicked
}
