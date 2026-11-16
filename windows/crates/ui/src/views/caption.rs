//! The window's caption buttons, drawn at the right end of the top bar where the system title bar would put them,
//! so the bar is all one surface with the panels under it (the Mac's unified title bar, the Windows 11 way).

use crate::app::App;
use crate::core::lang::l;
use crate::geom::{Color, Point, Rect};
use crate::platform::{Platform, WindowCommand};
use crate::ui::{layer, maximize_button_id, Id, Ui};

/// Width of one caption button, as Windows draws them.
pub const BUTTON_W: f32 = 46.0;

/// Height of the top bar the buttons sit in (the viewer's and the project panel's headers).
pub fn bar_height(ui: &Ui) -> f32 {
    ui.theme.s(38.0)
}

pub fn caption_buttons(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform) {
    let t = ui.theme;
    let h = bar_height(ui);
    let maximized = platform.is_maximized();
    let right = ui.bounds().w;
    let ink = if app.window_active { t.text } else { t.text3 };
    let buttons = [
        (WindowCommand::Minimize, Id::new("caption-minimize"), l("Minimize", "最小化")),
        // No tooltip: Windows 11 opens its snap layouts over this one.
        (WindowCommand::ToggleMaximize, maximize_button_id(), String::new()),
        (WindowCommand::Close, Id::new("caption-close"), l("Close", "关闭")),
    ];
    let mut clicked = None;
    ui.with_layer(layer::DIALOG, |ui| {
        for (i, (command, id, tip)) in buttons.into_iter().enumerate() {
            let rect = Rect::new(right - BUTTON_W * (3 - i) as f32, 0.0, BUTTON_W, h);
            let r = ui.interact(id, rect);
            let lit = r.hovered || r.dragging;
            let pressed = r.hovered && r.dragging;
            let close = command == WindowCommand::Close;
            let mut color = ink;
            if close && lit {
                ui.fill(rect, Color::hex(0xC42B1C).alpha(if pressed { 0.88 } else { 1.0 }));
                color = Color::WHITE;
            } else if lit {
                ui.fill(rect, if pressed { t.pressed } else { t.hover });
            }
            glyph(ui, command, maximized, rect, color);
            ui.tooltip(&r, &tip);
            if r.clicked {
                clicked = Some(command);
            }
        }
    });
    if let Some(command) = clicked {
        platform.window_command(command);
    }
}

/// The 10-DIP glyphs Windows uses, in lines one device pixel wide on the pixel grid.
fn glyph(ui: &mut Ui, command: WindowCommand, maximized: bool, rect: Rect, color: Color) {
    let scale = ui.painter.scale();
    let px = 1.0 / scale;
    let snap = |v: f32| (v * scale).round() / scale;
    let (cx, cy) = (snap(rect.center().x), snap(rect.center().y));
    let half = 5.0;
    let (x0, y0) = (cx - half, cy - half);
    match command {
        WindowCommand::Minimize => ui.fill(Rect::new(x0, cy, half * 2.0, px), color),
        WindowCommand::ToggleMaximize if maximized => {
            // Restore: a square in front, the corner of another behind it.
            let front = Rect::new(x0, y0 + 2.0, 8.0, 8.0);
            ui.painter.stroke_round_rect(front.inset(px / 2.0), 1.0, px, color);
            let (bx, by) = (x0 + 2.0 + px / 2.0, y0 + px / 2.0);
            let end = x0 + 10.0 - px / 2.0;
            ui.painter.line(Point::new(bx, by), Point::new(end - 1.0, by), px, color);
            ui.painter.line(Point::new(end, by + 1.0), Point::new(end, y0 + 8.0), px, color);
            ui.painter.line(Point::new(end - 1.0, by), Point::new(end, by + 1.0), px, color);
        }
        WindowCommand::ToggleMaximize => {
            let r = Rect::new(x0, y0, half * 2.0, half * 2.0);
            ui.painter.stroke_round_rect(r.inset(px / 2.0), 1.0, px, color);
        }
        WindowCommand::Close => {
            let w = px * 1.1;
            ui.painter.line(Point::new(x0, y0), Point::new(x0 + 10.0, y0 + 10.0), w, color);
            ui.painter.line(Point::new(x0 + 10.0, y0), Point::new(x0, y0 + 10.0), w, color);
        }
    }
}
