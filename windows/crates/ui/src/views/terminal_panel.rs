//! The terminals along the bottom of the window: the current project's shells side by side, each under a header.
//!
//! The screen is drawn cell by cell from alacritty_terminal's grid: plain ASCII in runs, anything wider or from
//! another face one character at a time at its own column, so the columns stay true whatever the font falls back to.

use std::time::Duration;

use super::components::key_caps;
use crate::app::project::ProjectId;
use crate::app::{ids, App, MenuItem, PlatformRequest};
use crate::core::lang::l;
use crate::core::lf;
use crate::geom::{Color, Point, Rect, Size};
use crate::icons::Icon;
use crate::input::{Event, Key, Modifiers, MouseButton};
use crate::paint::{ColorRun, CursorIcon, Font, Overflow, TextRequest, TextStyle, Weight};
use crate::platform::Platform;
use crate::terminal::{
    key_bytes, mouse_report, mix, same_dir, Column, CursorShape, Dimensions, Flags, GridPoint, Line, Palette, Scroll,
    Selection, SelectionType, SessionId, Side, TermMode,
};
use crate::ui::{Align, Id, Ui};
use crate::widgets::icon_button;

/// Narrowest a terminal gets when the one beside it is widened.
pub const MIN_PANE_W: f32 = 180.0;

const MOUSE_MODES: TermMode = TermMode::MOUSE_REPORT_CLICK.union(TermMode::MOUSE_DRAG).union(TermMode::MOUSE_MOTION);

fn header_h(ui: &Ui) -> f32 {
    ui.theme.s(26.0)
}

fn orange(ui: &Ui) -> Color {
    Color::hex(if ui.theme.dark { 0xFF9F0A } else { 0xFF9500 })
}

pub fn terminal_panel(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect, project: ProjectId) {
    let t = ui.theme;
    ui.fill(rect, t.window);
    ui.block(Id::new("terminal-panel"), rect);
    app.terminals.pane_rects.clear();
    let panes: Vec<(SessionId, f32)> = app.terminals.for_project(project).map(|s| (s.id, s.weight)).collect();
    if panes.is_empty() {
        empty(app, ui, rect, project);
    } else {
        let n = panes.len();
        let available = (rect.w - (n - 1) as f32).max(0.0);
        let total: f32 = panes.iter().map(|p| p.1).sum::<f32>().max(0.001);
        let per_weight = available / total;
        let mut x = rect.x;
        let mut dividers = Vec::new();
        for (i, (id, weight)) in panes.iter().enumerate() {
            let last = i == n - 1;
            let w = if last { rect.right() - x } else { (weight * per_weight).floor() };
            pane(app, ui, platform, Rect::new(x, rect.y, w.max(0.0), rect.h), *id, project, last);
            x += w;
            if !last {
                ui.vline(x, rect.y, rect.h, t.border);
                dividers.push((x, *id, panes[i + 1].0));
                x += 1.0;
            }
        }
        // After the panes, so the strip either side of a line drags rather than selects.
        for (x, left, right) in dividers {
            divider(app, ui, Rect::new(x - 3.0, rect.y, 7.0, rect.h), left, right, per_weight);
        }
    }

    let b = t.s(22.0);
    let y = rect.y + ((header_h(ui) - b) / 2.0).round();
    let hide = Rect::new(rect.right() - t.s(6.0) - b, y, b, b);
    let add = hide.offset(-(b + t.s(2.0)), 0.0);
    if icon_button(ui, Id::new("terminal-new"), add, Icon::Add, &l("New Terminal (Ctrl+Shift+T)", "新建终端 (Ctrl+Shift+T)"), true, false).clicked {
        app.open_terminal(project, None);
    }
    if icon_button(ui, Id::new("terminal-hide"), hide, Icon::ChevronDown, &l("Hide Terminal (Alt+F12)", "隐藏终端 (Alt+F12)"), true, false)
        .clicked
    {
        app.hide_terminal();
    }
}

/// No terminal yet in this project: a line saying so and a button to open one.
fn empty(app: &mut App, ui: &mut Ui, rect: Rect, project: ProjectId) {
    let t = ui.theme;
    let name = app.project(project).map(|p| p.name.clone()).unwrap_or_default();
    let line_h = t.s(18.0);
    let button_h = t.s(30.0);
    let gap = t.s(10.0);
    let top = rect.y + ((rect.h - line_h - gap - button_h) / 2.0).max(t.s(8.0)).round();
    let text = lf!("No terminal open in {name}", "{name} 里还没有打开终端");
    ui.label_with(Rect::new(rect.x, top, rect.w, line_h), &text, t.font(12.5), t.text3, Align::Center, Overflow::Ellipsis);

    let font = t.font_w(12.5, Weight::Medium);
    let title = l("New Terminal", "新建终端");
    let keys = "Ctrl+Shift+T";
    let icon = t.s(12.0);
    let gap_in = t.s(7.0);
    let title_w = ui.text_width(&title, font).ceil();
    let keys_w = super::components::key_caps_width(ui, keys, 11.0);
    let pad = t.s(13.0);
    let w = pad * 2.0 + icon + gap_in + title_w + gap_in + keys_w;
    let button = Rect::new((rect.center().x - w / 2.0).round(), top + line_h + gap, w, button_h);
    let r = ui.interact(Id::new("terminal-empty-new"), button);
    ui.round(button, t.s(8.0), t.selection);
    if r.hovered {
        ui.round(button, t.s(8.0), t.hover);
    }
    let cy = button.center().y;
    let mut x = button.x + pad;
    ui.icon(Icon::Add, Rect::new(x, cy - icon / 2.0, icon, icon), t.text);
    x += icon + gap_in;
    ui.label(Rect::new(x, button.y, title_w + 1.0, button_h), &title, font, t.text);
    x += title_w + gap_in;
    key_caps(ui, x, cy, keys, 11.0);
    if r.clicked {
        app.open_terminal(project, None);
    }
}

/// The line between two terminals: dragging it moves width from one to the other.
fn divider(app: &mut App, ui: &mut Ui, rect: Rect, left: SessionId, right: SessionId, per_weight: f32) {
    let id = Id::new("terminal-divider").with(left);
    let r = ui.interact(id, rect);
    if r.hovered || r.dragging {
        ui.set_cursor(CursorIcon::ResizeHorizontal);
    }
    let (Some(lw), Some(rw)) = (app.terminals.session(left).map(|s| s.weight), app.terminals.session(right).map(|s| s.weight)) else {
        return;
    };
    if r.pressed {
        ui.st.drag_value = Some((id, lw));
    }
    if !r.dragging || per_weight <= 0.0 {
        return;
    }
    let start = match ui.st.drag_value {
        Some((i, v)) if i == id => v,
        _ => lw,
    };
    let sum = lw + rw;
    let floor = MIN_PANE_W / per_weight;
    if sum <= floor * 2.0 {
        return;
    }
    let moved = start + (ui.mouse().x - ui.press_pos().x) / per_weight;
    let clamped = moved.clamp(floor, sum - floor);
    if let Some(s) = app.terminals.session_mut(left) {
        s.weight = clamped;
    }
    if let Some(s) = app.terminals.session_mut(right) {
        s.weight = sum - clamped;
    }
}

// MARK: - A terminal

/// Cell size for the terminal font.
fn cell_size(ui: &Ui, font: Font) -> Size {
    let m = ui.backend.metrics(font);
    Size::new(m.char_width.max(1.0), (m.ascent + m.descent).max(font.size * 1.2).ceil())
}

#[allow(clippy::too_many_arguments)]
fn pane(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect, id: SessionId, project: ProjectId, last: bool) {
    let tid = ids::terminal(id);
    let focused = ui.has_focus(tid) || app.focus == Some(tid);
    let header = Rect::new(rect.x, rect.y, rect.w, header_h(ui));
    let body = Rect::new(rect.x, header.bottom(), rect.w, (rect.h - header.h).max(0.0));
    pane_header(app, ui, platform, header, id, project, focused, last);
    pane_body(app, ui, platform, body, id, project, focused);
}

#[allow(clippy::too_many_arguments)]
fn pane_header(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform, header: Rect, id: SessionId, project: ProjectId, focused: bool, last: bool) {
    let t = ui.theme;
    let hid = Id::new("terminal-header").with(id);
    let close_id = Id::new("terminal-close").with(id);
    ui.fill(header, t.panel);
    if focused {
        ui.fill(Rect::new(header.x, header.y, header.w, 1.5), t.accent);
    }
    let r = ui.interact(hid, header);
    let hovering = r.hovered || ui.is_hovered(close_id);
    let Some(s) = app.terminals.session(id) else { return };
    let dot = if s.needs_attention {
        orange(ui)
    } else if s.is_busy {
        t.added
    } else {
        t.text3.alpha(0.6)
    };
    let name = s.process_name.clone();
    let subtitle = s.subtitle().to_string();
    let root = app.project(project).map(|p| p.url.clone());
    let folder = match &root {
        Some(root) if !same_dir(&s.directory, root) => s.directory.file_name().map(|n| n.to_string_lossy().into_owned()),
        _ => None,
    };

    let cy = header.center().y;
    // The last header shares its right end with the panel's own buttons.
    let trailing = if last { t.s(26.0) * 2.0 + t.s(12.0) } else { t.s(6.0) };
    let close_w = t.s(18.0);
    let close = Rect::new(header.right() - trailing - close_w, cy - close_w / 2.0, close_w, close_w);
    let limit = close.x - t.s(4.0);
    let mut x = header.x + t.s(12.0);
    let d = 6.0;
    ui.painter.fill_ellipse(Point::new(x + d / 2.0, cy), d / 2.0, d / 2.0, dot);
    x += d + t.s(6.0);
    let name_font = t.font_w(11.5, if focused { Weight::Semibold } else { Weight::Medium });
    let name_w = ui.text_width(&name, name_font).ceil().min((limit - x).max(0.0));
    ui.label(Rect::new(x, header.y, name_w + 1.0, header.h), &name, name_font, if focused { t.text } else { t.text2 });
    x += name_w + t.s(6.0);
    if let Some(folder) = folder {
        let font = t.font_w(10.5, Weight::Medium);
        let w = ui.text_width(&folder, font).ceil() + t.s(10.0);
        if x + w <= limit {
            let chip = Rect::new(x, cy - t.s(8.0), w, t.s(16.0));
            ui.round(chip, chip.h / 2.0, t.chip);
            ui.label_with(chip, &folder, font, t.text2, Align::Center, Overflow::Clip);
            x += w + t.s(6.0);
        }
    }
    if !subtitle.is_empty() && limit - x > t.s(24.0) {
        ui.label(Rect::new(x, header.y, limit - x, header.h), &subtitle, t.font(11.0), t.text3);
    }
    if r.pressed {
        app.focus_terminal_session(id);
    }
    if r.secondary {
        context_menu(app, platform, id, project, ui.mouse());
    }

    if hovering || focused {
        let cr = ui.interact(close_id, close);
        if cr.hovered {
            ui.round(close, t.s(5.0), t.hover);
        }
        let g = t.s(9.0);
        ui.icon(Icon::Dismiss, close.centered(Size::new(g, g)), t.text3);
        ui.tooltip(&cr, &l("Close Terminal (Ctrl+Shift+W)", "关闭终端 (Ctrl+Shift+W)"));
        if cr.clicked {
            app.close_terminal_session(id);
        }
    }
}

fn context_menu(app: &mut App, platform: &mut dyn Platform, id: SessionId, project: ProjectId, at: Point) {
    let Some(s) = app.terminals.session(id) else { return };
    let selected = s.selected_text();
    let clip = platform.clipboard().get().filter(|c| !c.is_empty());
    let has_selection = selected.is_some();
    let has_clip = clip.is_some();
    let items = vec![
        MenuItem::new(l("Copy", "复制"), move |app: &mut App| {
            if let Some(text) = selected {
                app.requests.push(PlatformRequest::Copy(text));
            }
        })
        .shortcut("Ctrl+Shift+C")
        .disabled(!has_selection),
        MenuItem::new(l("Paste", "粘贴"), move |app: &mut App| {
            if let (Some(text), Some(s)) = (clip, app.terminals.session(id)) {
                s.paste(&text);
            }
            app.focus_terminal_session(id);
        })
        .shortcut("Ctrl+Shift+V")
        .disabled(!has_clip),
        MenuItem::separator(),
        MenuItem::new(l("New Terminal", "新建终端"), move |app: &mut App| {
            app.open_terminal(project, None);
        })
        .shortcut("Ctrl+Shift+T"),
        MenuItem::new(l("Clear", "清屏"), move |app: &mut App| {
            app.focus_terminal_session(id);
            if let Some(s) = app.terminals.session(id) {
                s.clear();
            }
        }),
        MenuItem::separator(),
        MenuItem::new(l("Close Terminal", "关闭终端"), move |app: &mut App| app.close_terminal_session(id)).shortcut("Ctrl+Shift+W"),
    ];
    app.open_menu(at, items);
}

/// What one row of the screen draws, gathered under the lock and drawn after it is let go.
#[derive(Default)]
struct Row {
    /// Background runs: first column, columns, colour.
    backgrounds: Vec<(usize, usize, u32)>,
    /// Selected runs: first column, columns.
    selected: Vec<(usize, usize)>,
    texts: Vec<Run>,
    /// Underlines and strikeouts: first column, columns, colour, strikeout.
    lines: Vec<(usize, usize, u32, bool)>,
}

struct Run {
    column: usize,
    text: String,
    bold: bool,
    italic: bool,
    colors: Vec<ColorRun>,
    /// One character drawn on its own (wide, or not ASCII): no more is added to it.
    single: bool,
}

struct Screen {
    rows: Vec<Row>,
    cursor: Option<(usize, usize, CursorShape, bool)>,
    /// The character under the cursor, to draw over a block cursor.
    cursor_char: Option<(String, bool, bool)>,
    display_offset: usize,
    history: usize,
    mode: TermMode,
}

fn read_screen(term: &crate::terminal::Session, palette: &Palette, columns: usize, lines: usize) -> Screen {
    let term = term.term.lock();
    let content = term.renderable_content();
    let display_offset = content.display_offset;
    let mut rows: Vec<Row> = (0..lines).map(|_| Row::default()).collect();
    let cursor_point = content.cursor.point;
    let cursor_row = cursor_point.line.0 + display_offset as i32;
    let cursor = (content.cursor.shape != CursorShape::Hidden && cursor_row >= 0 && (cursor_row as usize) < lines)
        .then(|| {
            let wide = term.grid()[cursor_point].flags.contains(Flags::WIDE_CHAR);
            (cursor_point.column.0, cursor_row as usize, content.cursor.shape, wide)
        });
    let mut cursor_char = None;
    let selection = content.selection;
    for cell in content.display_iter {
        let row = cell.point.line.0 + display_offset as i32;
        if row < 0 || row as usize >= lines || cell.point.column.0 >= columns {
            continue;
        }
        let flags = cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let r = &mut rows[row as usize];
        let col = cell.point.column.0;
        let width = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        let mut fg = palette.resolve(cell.fg, content.colors);
        let mut bg = palette.resolve(cell.bg, content.colors);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::DIM) {
            fg = mix(fg, bg, 0.35);
        }
        if bg != palette.background {
            match r.backgrounds.last_mut() {
                Some((c, w, color)) if *color == bg && *c + *w == col => *w += width,
                _ => r.backgrounds.push((col, width, bg)),
            }
        }
        if selection.is_some_and(|s| s.contains(cell.point)) {
            match r.selected.last_mut() {
                Some((c, w)) if *c + *w == col => *w += width,
                _ => r.selected.push((col, width)),
            }
        }
        let underline = flags.intersects(Flags::ALL_UNDERLINES);
        let strike = flags.contains(Flags::STRIKEOUT);
        for (on, is_strike) in [(underline, false), (strike, true)] {
            if on {
                let color = cell.underline_color().filter(|_| !is_strike).map(|c| palette.resolve(c, content.colors)).unwrap_or(fg);
                match r.lines.iter_mut().rev().find(|l| l.3 == is_strike) {
                    Some((c, w, lc, _)) if *lc == color && *c + *w == col => *w += width,
                    _ => r.lines.push((col, width, color, is_strike)),
                }
            }
        }
        let bold = flags.contains(Flags::BOLD);
        let italic = flags.contains(Flags::ITALIC);
        let c = cell.c;
        if cursor.is_some_and(|(cc, cr, ..)| cc == col && cr == row as usize) {
            let mut text = String::from(if flags.contains(Flags::HIDDEN) { ' ' } else { c });
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            cursor_char = Some((text, bold, italic));
        }
        if c == ' ' || c == '\0' || flags.contains(Flags::HIDDEN) {
            continue;
        }
        let color = Color::hex(fg);
        let ascii = c.is_ascii_graphic() && width == 1 && cell.zerowidth().is_none();
        let joins = r.texts.last().is_some_and(|run| {
            !run.single && ascii && run.bold == bold && run.italic == italic && run.column + run.text.len() <= col
        });
        if joins {
            let run = r.texts.last_mut().unwrap();
            // Spaces skipped since the run's end keep the columns.
            let gap = col - (run.column + run.text.len());
            run.text.extend(std::iter::repeat_n(' ', gap));
            let start = run.text.len() as u32;
            run.text.push(c);
            match run.colors.last_mut() {
                Some(last) if last.color == color && last.range.end == start => last.range.end = start + 1,
                _ => run.colors.push(ColorRun { range: start..start + 1, color }),
            }
        } else {
            let mut text = String::from(c);
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            let len = text.len() as u32;
            r.texts.push(Run { column: col, text, bold, italic, colors: vec![ColorRun { range: 0..len, color }], single: !ascii });
        }
    }
    Screen { rows, cursor, cursor_char, display_offset, history: term.grid().history_size(), mode: *term.mode() }
}

fn pane_body(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform, body: Rect, id: SessionId, project: ProjectId, focused: bool) {
    let t = ui.theme;
    let tid = ids::terminal(id);
    let scroll_id = Id::new("terminal-scrollbar").with(id);
    let palette = Palette::new(t.dark);
    ui.fill(body, Color::hex(palette.background));
    let inner = body.inset_by(10.0, 4.0, 4.0, 4.0);
    let font = Font::terminal(app.settings.terminal_font_size);
    let cell = cell_size(ui, font);
    let columns = ((inner.w / cell.w).floor() as usize).max(2);
    let lines = ((inner.h / cell.h).floor() as usize).max(1);
    app.terminals.pane_rects.push((id, body));
    {
        let Some(s) = app.terminals.session_mut(id) else { return };
        if !s.is_started && !s.has_ended {
            if inner.w > 80.0 && inner.h > 40.0 {
                s.start(columns, lines, (cell.w, cell.h));
            }
        } else if !s.has_ended {
            s.resize(columns, lines, (cell.w, cell.h));
        }
    }

    let r = ui.interact(tid, body);
    let Some(s) = app.terminals.session(id) else { return };
    let (columns, lines) = s.grid;
    let screen = read_screen(s, &palette, columns, lines);
    let mouse_mode = screen.mode.intersects(MOUSE_MODES);
    let to_cell = |p: Point| -> (usize, usize, Side) {
        let fx = ((p.x - inner.x) / cell.w).max(0.0);
        let col = (fx as usize).min(columns - 1);
        let side = if fx >= columns as f32 || fx.fract() > 0.5 { Side::Right } else { Side::Left };
        let row = ((p.y - inner.y) / cell.h).floor().clamp(0.0, (lines - 1) as f32) as usize;
        (col, row, side)
    };
    let grid_point = |col: usize, row: usize| GridPoint::new(Line(row as i32 - screen.display_offset as i32), Column(col));

    // Drawing.
    ui.painter.push_clip(body);
    let x_of = |col: usize| inner.x + col as f32 * cell.w;
    let y_of = |row: usize| inner.y + row as f32 * cell.h;
    let selection = Color::hexa(palette.selection.0, palette.selection.1);
    for (row, data) in screen.rows.iter().enumerate() {
        let y = y_of(row);
        for &(col, w, color) in &data.backgrounds {
            ui.fill(Rect::new(x_of(col), y, w as f32 * cell.w, cell.h), Color::hex(color));
        }
        for &(col, w) in &data.selected {
            ui.fill(Rect::new(x_of(col), y, w as f32 * cell.w, cell.h), selection);
        }
    }
    let foreground = Color::hex(palette.foreground);
    let run_font = |bold: bool, italic: bool| {
        let f = if bold { font.weight(Weight::Bold) } else { font };
        if italic {
            f.italic()
        } else {
            f
        }
    };
    for (row, data) in screen.rows.iter().enumerate() {
        let y = y_of(row);
        for run in &data.texts {
            let request = TextRequest {
                text: run.text.as_str().into(),
                style: TextStyle::new(run_font(run.bold, run.italic)),
                runs: run.colors.clone(),
                bold_ranges: Vec::new(),
            };
            let layout = ui.layout(request);
            let h = layout.size().h;
            ui.painter.text(layout.as_ref(), Point::new(x_of(run.column), y + ((cell.h - h) / 2.0).round()), foreground);
        }
        for &(col, w, color, strike) in &data.lines {
            let ly = if strike { y + (cell.h / 2.0).round() } else { y + cell.h - 2.0 };
            ui.fill(Rect::new(x_of(col), ly, w as f32 * cell.w, 1.0), Color::hex(color));
        }
    }

    // The cursor: solid where typing goes, hollow when the keyboard is elsewhere.
    let active = focused && app.window_active;
    let preedit = s.preedit.clone();
    if let Some((col, row, shape, wide)) = screen.cursor {
        let w = if wide { cell.w * 2.0 } else { cell.w };
        let c = Rect::new(x_of(col), y_of(row), w, cell.h);
        let color = Color::hex(palette.cursor);
        if focused {
            ui.st.ime_area = Some(c);
        }
        if !active {
            ui.painter.stroke_rect(c.inset(0.5), 1.0, color);
        } else if preedit.is_empty() {
            match shape {
                CursorShape::Beam => ui.fill(Rect::new(c.x, c.y, 2.0, c.h), color),
                CursorShape::Underline => ui.fill(Rect::new(c.x, c.bottom() - 2.0, c.w, 2.0), color),
                CursorShape::HollowBlock => ui.painter.stroke_rect(c.inset(0.5), 1.0, color),
                _ => {
                    ui.fill(c, color);
                    if let Some((text, bold, italic)) = &screen.cursor_char {
                        let layout = ui.text_layout(text, TextStyle::new(run_font(*bold, *italic)));
                        let h = layout.size().h;
                        ui.painter.text(layout.as_ref(), Point::new(c.x, c.y + ((cell.h - h) / 2.0).round()), Color::hex(palette.background));
                    }
                }
            }
        }
        // An input method's composition, over the line at the cursor.
        if !preedit.is_empty() && focused {
            let layout = ui.text_layout(&preedit, TextStyle::new(font));
            let size = layout.size();
            let p = Rect::new(c.x, c.y, size.w.max(cell.w), cell.h);
            ui.fill(p, Color::hex(palette.background));
            ui.painter.text(layout.as_ref(), Point::new(p.x, p.y + ((cell.h - size.h) / 2.0).round()), foreground);
            ui.fill(Rect::new(p.x, p.bottom() - 1.0, p.w, 1.0), foreground);
        }
    }
    ui.painter.pop_clip();

    // The scrollbar: over the right edge while the mouse is on the terminal, when there is history.
    let total = screen.history + lines;
    let track = Rect::new(body.right() - t.s(9.0), inner.y, t.s(7.0), inner.h);
    let thumb_h = (track.h * lines as f32 / total as f32).max(t.s(24.0)).min(track.h);
    let top_line = screen.history - screen.display_offset.min(screen.history);
    let thumb_y = if screen.history > 0 { track.y + (track.h - thumb_h) * top_line as f32 / screen.history as f32 } else { track.y };
    let thumb = Rect::new(track.x, thumb_y, track.w, thumb_h);
    let grab = s.scrollbar_grab;
    let mut new_grab = grab;
    if screen.history > 0 && (r.hovered || ui.is_hovered(scroll_id) || grab.is_some()) {
        let sr = ui.interact(scroll_id, track);
        let strong = sr.hovered || sr.dragging;
        ui.round(thumb.inset_xy(if strong { 0.0 } else { 1.0 }, 0.0), t.s(3.5), t.text3.alpha(if strong { 0.55 } else { 0.35 }));
        if sr.pressed {
            let m = ui.mouse().y;
            new_grab = Some(if thumb.contains(ui.mouse()) { m - thumb.y } else { thumb_h / 2.0 });
        }
        if sr.dragging {
            if let Some(g) = new_grab {
                let span = (track.h - thumb_h).max(1.0);
                let top = ((ui.mouse().y - g - track.y) / span).clamp(0.0, 1.0) * screen.history as f32;
                let offset = screen.history as i32 - top.round() as i32;
                let delta = offset - screen.display_offset as i32;
                if delta != 0 {
                    s.scroll(delta);
                }
            }
        } else {
            new_grab = None;
        }
    } else {
        new_grab = None;
    }

    // Mouse on the screen.
    if r.hovered && !(mouse_mode && !ui.mods().shift) {
        ui.set_cursor(CursorIcon::Text);
    }
    let mods = ui.mods();
    let report = mouse_mode && !mods.shift;
    if (r.pressed || r.secondary || r.middle) && !focused {
        app.focus_terminal_session(id);
        ui.focus(tid);
    }
    let Some(s) = app.terminals.session_mut(id) else { return };
    s.scrollbar_grab = new_grab;
    if r.pressed {
        let (col, row, side) = to_cell(ui.mouse());
        if report {
            if let Some(bytes) = mouse_report(screen.mode, Some(MouseButton::Left), None, true, false, col, row, mods) {
                s.write(bytes);
            }
            s.mouse_cell = Some((col, row));
        } else {
            let point = grid_point(col, row);
            let mut term = s.term.lock();
            if mods.shift && term.selection.is_some() {
                if let Some(sel) = term.selection.as_mut() {
                    sel.update(point, side);
                }
            } else {
                let kind = if r.triple_clicked {
                    SelectionType::Lines
                } else if r.double_clicked {
                    SelectionType::Semantic
                } else {
                    SelectionType::Simple
                };
                term.selection = Some(Selection::new(kind, point, side));
            }
            drop(term);
            s.selecting = true;
        }
    } else if r.dragging {
        let m = ui.mouse();
        let (col, row, side) = to_cell(m);
        if s.selecting {
            // Past the top or bottom edge, the history scrolls under the drag.
            if m.y < inner.y {
                s.scroll(1);
                ui.repaint_after(Duration::from_millis(50));
            } else if m.y > inner.bottom() {
                s.scroll(-1);
                ui.repaint_after(Duration::from_millis(50));
            }
            let offset = s.term.lock().grid().display_offset();
            let point = GridPoint::new(Line(row as i32 - offset as i32), Column(col));
            if let Some(sel) = s.term.lock().selection.as_mut() {
                sel.update(point, side);
            }
        } else if report && screen.mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) && s.mouse_cell != Some((col, row)) {
            if let Some(bytes) = mouse_report(screen.mode, Some(MouseButton::Left), None, true, true, col, row, mods) {
                s.write(bytes);
            }
            s.mouse_cell = Some((col, row));
        }
    } else if r.hovered && report && screen.mode.contains(TermMode::MOUSE_MOTION) && matches!(ui.event, Some(Event::MouseMove(_))) {
        let (col, row, _) = to_cell(ui.mouse());
        if s.mouse_cell != Some((col, row)) {
            if let Some(bytes) = mouse_report(screen.mode, None, None, true, true, col, row, mods) {
                s.write(bytes);
            }
            s.mouse_cell = Some((col, row));
        }
    }
    if let Some(Event::MouseUp { button: MouseButton::Left, pos, mods }) = ui.event.clone() {
        if s.selecting {
            s.selecting = false;
            let mut term = s.term.lock();
            if term.selection.as_ref().is_some_and(|sel| sel.is_empty()) {
                term.selection = None;
            }
        } else if (r.clicked || ui.st.is_active(tid)) && report {
            let (col, row, _) = to_cell(pos);
            if let Some(bytes) = mouse_report(screen.mode, Some(MouseButton::Left), None, false, false, col, row, mods) {
                s.write(bytes);
            }
        }
    }
    if r.middle && report {
        let (col, row, _) = to_cell(ui.mouse());
        for pressed in [true, false] {
            if let Some(bytes) = mouse_report(screen.mode, Some(MouseButton::Middle), None, pressed, false, col, row, mods) {
                s.write(bytes);
            }
        }
    }

    // The wheel: to the program when it asked for the mouse, as arrow keys in a full-screen program, else through
    // the history.
    if let Some(delta) = ui.scroll_target(Id::new("terminal-wheel").with(id), body) {
        s.wheel_rest += delta.y / cell.h;
        let steps = s.wheel_rest.trunc() as i32;
        s.wheel_rest -= steps as f32;
        if steps != 0 {
            let wheel_mods = ui.mods();
            if mouse_mode && !wheel_mods.shift {
                let (col, row, _) = to_cell(ui.mouse());
                for _ in 0..steps.unsigned_abs().min(10) {
                    if let Some(bytes) = mouse_report(screen.mode, None, Some(steps < 0), true, false, col, row, wheel_mods) {
                        s.write(bytes);
                    }
                }
            } else if screen.mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !wheel_mods.shift {
                let key = if steps < 0 { Key::Up } else { Key::Down };
                if let Some(bytes) = key_bytes(key, Modifiers::NONE, screen.mode) {
                    s.write(bytes.repeat(steps.unsigned_abs().min(20) as usize));
                }
            } else {
                s.scroll(-steps);
            }
        }
    }

    if r.secondary {
        context_menu(app, platform, id, project, ui.mouse());
    }

    // The keyboard.
    if !ui.has_focus(tid) {
        return;
    }
    ui.mark_text_input(tid);
    let Some(event) = ui.event.clone() else { return };
    if ui.is_consumed() {
        return;
    }
    let Some(s) = app.terminals.session_mut(id) else { return };
    const CTRL: Modifiers = Modifiers::CTRL;
    const SHIFT: Modifiers = Modifiers::SHIFT;
    const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false };
    match event {
        Event::Key { key, mods, .. } => {
            let selected = || s.selected_text();
            match (key, mods) {
                // Ctrl+C copies what is selected; with nothing selected it interrupts the program.
                (Key::Char('c'), CTRL) if s.term.lock().selection.is_some() => {
                    if let Some(text) = selected() {
                        app.requests.push(PlatformRequest::Copy(text));
                    }
                    s.term.lock().selection = None;
                }
                (Key::Char('c'), CTRL_SHIFT) | (Key::Insert, CTRL) => {
                    if let Some(text) = selected() {
                        app.requests.push(PlatformRequest::Copy(text));
                    }
                }
                (Key::Char('v'), CTRL) | (Key::Char('v'), CTRL_SHIFT) | (Key::Insert, SHIFT) => {
                    if let Some(text) = platform.clipboard().get() {
                        s.paste(&text);
                    }
                }
                (Key::PageUp, SHIFT) => s.term.lock().scroll_display(Scroll::PageUp),
                (Key::PageDown, SHIFT) => s.term.lock().scroll_display(Scroll::PageDown),
                (Key::Home, SHIFT) if !screen.mode.contains(TermMode::ALT_SCREEN) => s.term.lock().scroll_display(Scroll::Top),
                (Key::End, SHIFT) if !screen.mode.contains(TermMode::ALT_SCREEN) => s.term.lock().scroll_display(Scroll::Bottom),
                _ => match key_bytes(key, mods, screen.mode) {
                    Some(bytes) => s.write_input(bytes),
                    None => return,
                },
            }
            ui.consume();
        }
        Event::Text(text) => {
            s.preedit.clear();
            s.write_input(text.into_bytes());
            ui.consume();
        }
        Event::Preedit { text, .. } => {
            s.preedit = text;
            ui.consume();
        }
        _ => {}
    }
}
