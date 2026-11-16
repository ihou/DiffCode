//! Every view, as functions that draw the app state and change it directly.
//!
//! The window: the sidebar (projects over the file tree), the project panel (Changes or Log), and the viewer. Over
//! them, in layers: popups, menus, the toast.

mod caption;
pub mod components;
mod dialog;
mod file_tree;
mod git_graph;
mod menu;
mod popups;
mod project_panel;
mod commit_panel;
mod sidebar;
mod terminal_panel;
mod viewer_panel;

pub use components::ViewState;

use crate::app::settings::{self, Settings};
use crate::app::App;
use crate::geom::{Color, Rect, Size};
use crate::paint::{Overflow, Weight};
use crate::platform::Platform;
use crate::ui::{layer, Align, Id, Ui};
use crate::widgets::splitter;

/// Width of the Project panel's range, as on the Mac.
pub const LIST_RANGE: (f32, f32) = (236.0, 620.0);

pub fn root(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform) {
    let mut vs = std::mem::take(&mut app.views);
    menu::menu_keys(app, ui);
    let t = ui.theme;
    let bounds = ui.bounds();
    ui.fill(bounds, t.window);
    // With no system title bar, the top bar's empty space moves the window; everything drawn later sits over it.
    let caption = platform.caption_buttons();
    app.caption_width = if caption { caption::BUTTON_W * 3.0 } else { 0.0 };
    if caption {
        ui.window_drag_area(Rect::new(0.0, 0.0, bounds.w, caption::bar_height(ui)));
    }

    let mut area = bounds;
    let mut splitters: Vec<(Id, Rect, Which)> = Vec::new();
    // The terminals run the full width of the window, under the sidebar too.
    let terminal = app.current_project().map(|p| p.id).filter(|_| app.terminals.visible && !app.git_missing);
    if let Some(project) = terminal {
        // They may take most of the window, but always leave the diff a strip to live in.
        let ceiling = (area.h - 200.0).max(TERMINAL_MIN);
        let h = app.settings.terminal_height.clamp(TERMINAL_MIN, ceiling);
        let panel = area.take_bottom(h);
        ui.hline(area.x, panel.y - 1.0, area.w, t.border);
        area.h -= 1.0;
        terminal_panel::terminal_panel(app, ui, platform, panel, project);
        splitters.push((Id::new("split-terminal"), Rect::new(area.x, panel.y - 4.0, area.w, 7.0), Which::Terminal(ceiling)));
    } else {
        app.terminals.pane_rects.clear();
    }
    if app.sidebar_visible {
        let max = (area.w - 360.0).max(settings::SIDEBAR_RANGE.0);
        let w = app.settings.sidebar_width.clamp(settings::SIDEBAR_RANGE.0, settings::SIDEBAR_RANGE.1.min(max));
        let side = area.take_left(w);
        sidebar::sidebar(app, &mut vs, ui, platform, side);
        ui.vline(area.x, area.y, area.h, t.border);
        area.take_left(1.0);
        splitters.push((Id::new("split-sidebar"), Rect::new(area.x - 4.0, area.y, 7.0, area.h), Which::Sidebar));
    }
    if app.git_missing {
        sidebar::git_missing(app, ui, area);
    } else if app.current_project().is_some() {
        if app.list_visible {
            let max = (area.w - 280.0).max(LIST_RANGE.0);
            let w = app.settings.list_width.clamp(LIST_RANGE.0, LIST_RANGE.1.min(max));
            let list = area.take_left(w);
            project_panel::project_panel(app, &mut vs, ui, platform, list);
            ui.vline(area.x, area.y, area.h, t.border);
            area.take_left(1.0);
            splitters.push((Id::new("split-list"), Rect::new(area.x - 4.0, area.y, 7.0, area.h), Which::List));
        }
        viewer_panel::viewer_panel(app, &mut vs, ui, platform, area);
    } else {
        sidebar::welcome(app, ui, platform, area);
    }

    // Over the panels' edges, so a drag there resizes rather than selects.
    for (id, rect, which) in splitters {
        let s: &mut Settings = &mut app.settings;
        let (value, lo, hi, vertical, sign) = match which {
            Which::Sidebar => (&mut s.sidebar_width, settings::SIDEBAR_RANGE.0, settings::SIDEBAR_RANGE.1, true, 1.0),
            Which::List => (&mut s.list_width, LIST_RANGE.0, LIST_RANGE.1, true, 1.0),
            Which::Terminal(ceiling) => {
                s.terminal_height = s.terminal_height.clamp(TERMINAL_MIN, ceiling);
                (&mut s.terminal_height, TERMINAL_MIN, ceiling, false, -1.0)
            }
        };
        if splitter(ui, id, rect, vertical, value, lo, hi, sign) {
            app.settings.save();
        }
    }

    popups::popups(app, &mut vs, ui, platform);
    menu::menu(app, ui);
    if caption {
        caption::caption_buttons(app, ui, platform);
    }
    toast(app, ui);
    if app.drop_targeted {
        // Over a terminal, the paths are typed there: that terminal lights up. Anywhere else they are added as projects.
        let mouse = ui.mouse();
        match app.terminals.pane_rects.iter().find(|(_, r)| r.contains(mouse)) {
            Some((_, pane)) => {
                ui.fill(*pane, t.accent.alpha(0.06));
                ui.painter.stroke_rect(pane.inset(1.0), 2.0, t.accent);
            }
            None => {
                let r = bounds.inset(10.0);
                ui.round(r, t.s(12.0), t.accent.alpha(0.08));
                ui.painter.stroke_round_rect(r.inset(1.5), t.s(12.0), 3.0, t.accent);
            }
        }
    }
    vs.end_frame();
    app.views = vs;
}

enum Which {
    Sidebar,
    List,
    /// The terminals' top edge, with the most height they may take.
    Terminal(f32),
}

/// Least height of the terminal panel.
const TERMINAL_MIN: f32 = 120.0;

/// The capsule at the bottom. Only a toast with a button takes clicks; a plain one never gets in the way.
fn toast(app: &mut App, ui: &mut Ui) {
    let Some(toast) = &app.toast else { return };
    let t = ui.theme;
    let text_font = t.font_w(12.5, Weight::Medium);
    let action_font = t.font_w(12.5, Weight::Semibold);
    let bounds = ui.bounds();
    let max_text = (bounds.w * 0.7).min(t.s(640.0));
    let text_w = ui.text_width(&toast.text, text_font).min(max_text).ceil();
    let action_w = toast.action.as_ref().map(|a| ui.text_width(&a.title, action_font).ceil() + t.s(12.0)).unwrap_or(0.0);
    let pad = t.s(14.0);
    let h = t.s(32.0);
    let w = pad * 2.0 + text_w + action_w;
    let rect = Rect::new((bounds.w - w) / 2.0, bounds.bottom() - t.s(28.0) - h, w, h).snapped(ui.painter.scale());
    let text = toast.text.clone();
    let action_title = toast.action.as_ref().map(|a| a.title.clone());
    let mut fire = false;
    ui.with_layer(layer::DIALOG, |ui| {
        ui.painter.shadow(rect, h / 2.0, t.s(12.0), Color::BLACK.alpha(0.3));
        ui.round(rect, h / 2.0, t.elevated);
        ui.painter.stroke_round_rect(rect.inset(0.5), h / 2.0, 1.0, t.hairline);
        ui.label_with(Rect::new(rect.x + pad, rect.y, text_w + 1.0, h), &text, text_font, t.text, Align::Left, Overflow::Ellipsis);
        if let Some(title) = action_title {
            let b = Rect::new(rect.x + pad + text_w + t.s(6.0), rect.y, action_w, h);
            let r = ui.interact(Id::new("toast-action"), b);
            let color = if r.hovered { t.accent.lerp(t.text, 0.2) } else { t.accent };
            ui.label_with(b, &title, action_font, color, Align::Center, Overflow::Clip);
            if r.clicked {
                fire = true;
            }
        }
    });
    if fire {
        if let Some(action) = app.toast.take().and_then(|t| t.action) {
            (action.handler)(app);
        }
    }
}

/// A centred notice with a big glyph, a title, a line of detail and buttons under it (welcome, Git missing).
pub(crate) fn notice_layout(ui: &mut Ui, area: Rect, title: &str, detail: &str, buttons_h: f32) -> (Rect, f32) {
    let t = ui.theme;
    let title_font = t.font_w(22.0, Weight::Semibold);
    let mut style = crate::paint::TextStyle::new(t.font(13.0));
    style.max_width = Some(t.s(420.0).min(area.w - t.s(40.0)).max(100.0));
    style.overflow = Overflow::Wrap;
    let detail_layout = ui.text_layout(detail, style);
    let glyph = t.s(54.0);
    let title_h = t.s(30.0);
    let d = detail_layout.size();
    let total = glyph + t.s(18.0) + title_h + t.s(6.0) + d.h + t.s(18.0) + buttons_h;
    let top = area.y + ((area.h - total) / 2.0).max(t.s(20.0));
    let glyph_rect = Rect::new(area.center().x - glyph / 2.0, top, glyph, glyph);
    let y = top + glyph + t.s(18.0);
    ui.label_with(Rect::new(area.x, y, area.w, title_h), title, title_font, t.text, Align::Center, Overflow::Ellipsis);
    let y = y + title_h + t.s(6.0);
    let size = Size::new(d.w, d.h);
    ui.draw_layout(&detail_layout, Rect::new(area.center().x - size.w / 2.0, y, size.w, size.h), t.text2, Align::Left);
    (glyph_rect, y + d.h + t.s(18.0))
}
