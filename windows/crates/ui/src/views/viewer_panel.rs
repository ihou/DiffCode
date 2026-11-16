//! The right column: the diff or file viewer, with its header, the conflict bar, the find bar and the pane titles.

use std::sync::Arc;

use super::components::{self, ViewState};
use crate::app::project::{ProjectId, ProjectTab};
use crate::app::viewer::ViewerEvent;
use crate::app::{ids, App, ClipboardCommand};
use crate::core::diff::Content;
use crate::core::lang::l;
use crate::core::lf;
use crate::core::remote::ConflictBase;
use crate::core::util::{date_time_text, relative_text};
use crate::geom::{Color, Point, Rect, Size};
use crate::icons::Icon;
use crate::input::{Event, Key};
use crate::paint::{ImageSource, Overflow, TextStyle, Weight};
use crate::platform::Platform;
use crate::text_field::FieldOutcome;
use crate::ui::{Align, Id, Ui};
use crate::widgets::icon_button;

/// The decoded-image handles of the image on screen, kept while it stays.
#[derive(Default)]
pub struct ImageCache {
    key: (usize, usize),
    old: Option<ImageSource>,
    new: Option<ImageSource>,
}

pub fn viewer_panel(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect) {
    let Some(id) = app.selected else { return };
    let t = ui.theme;
    ui.fill(rect, t.window);
    let mut area = rect;
    let header = area.take_top(t.s(38.0));
    viewer_header(app, ui, id, header);
    ui.hline(area.x, area.y, area.w, t.hairline);
    area.take_top(1.0);

    let url = app.project(id).map(|p| p.url.clone());
    let conflict_path = app
        .viewer
        .request
        .as_ref()
        .filter(|r| app.viewer.shows_conflicts() && Some(&r.root) == url.as_ref())
        .map(|r| r.path.clone());
    if let Some(path) = conflict_path {
        let bar = area.take_top(t.s(36.0));
        conflict_bar(app, ui, id, &path, bar);
    }
    if app.viewer.find_visible {
        let bar = area.take_top(t.s(34.0));
        find_bar(app, ui, platform, bar);
    }
    let titles = app.viewer.document().filter(|d| matches!(d.content, Content::Text) && !d.is_plain_file).map(|d| (d.left_title.clone(), d.right_title.clone()));
    if let Some((left, right)) = titles {
        let bar = area.take_top(t.s(22.0));
        pane_titles(ui, bar, &left, &right, app.settings.unified);
    }

    // The canvas.
    let focused = ui.has_focus(ids::canvas());
    if app.viewer.request.is_none() {
        empty_state(app, ui, id, area);
        return;
    }
    let content = app.viewer.document().map(|d| match &d.content {
        Content::Text => 0,
        Content::Image { .. } => 1,
        Content::Message { .. } => 2,
    });
    match content {
        Some(0) => {
            if let Some(command) = app.pending_clipboard.take() {
                let clipboard = platform.clipboard();
                match command {
                    ClipboardCommand::Copy => app.viewer.canvas.copy(clipboard),
                    ClipboardCommand::Cut => app.viewer.canvas.cut(ui, clipboard),
                    ClipboardCommand::Paste => app.viewer.canvas.paste(clipboard),
                }
            }
            if focused {
                app.viewer.canvas.handle_input(ui, platform.clipboard());
            }
            let actions = app.viewer.canvas.show(ui, ids::canvas(), area, focused);
            let mut size = app.settings.font_size;
            if app.viewer.handle_canvas_actions(actions, &mut size) {
                app.settings.font_size = size;
                app.settings.save();
            }
            if let Some(note) = app.viewer.document().and_then(|d| d.note.clone()) {
                note_capsule(ui, area, &note);
            }
            if let Some((entry, anchor)) = app.viewer.canvas.annotation_hover.clone() {
                annotation_card(ui, area, &entry, anchor, (app.settings.font_size * 1.5).round());
            }
        }
        Some(1) => {
            let r = ui.interact(ids::canvas(), area);
            if r.pressed {
                ui.focus(ids::canvas());
            }
            image_diff(app, vs, ui, area);
            if focused && ui.take_key(Key::Escape, crate::input::Modifiers::NONE) {
                app.viewer.events.push(ViewerEvent::Exit);
            }
        }
        Some(_) => {
            let r = ui.interact(ids::canvas(), area);
            if r.pressed {
                ui.focus(ids::canvas());
            }
            if let Some(Content::Message { title, detail }) = app.viewer.document().map(|d| d.content.clone()) {
                components::empty_state(ui, area, Icon::Document, &title, &detail);
            }
            if focused && ui.take_key(Key::Escape, crate::input::Modifiers::NONE) {
                app.viewer.events.push(ViewerEvent::Exit);
            }
        }
        None => {
            if app.viewer.is_loading {
                components::spinner(ui, area.center(), t.s(8.0), t.text3);
            }
        }
    }
}

// MARK: - Header

fn viewer_header(app: &mut App, ui: &mut Ui, id: ProjectId, rect: Rect) {
    let t = ui.theme;
    let cy = rect.center().y;
    let size = Size::new(t.s(26.0), t.s(24.0));
    // The window's caption buttons take the right end of the bar when there is no system title bar.
    let mut right = rect.right() - if app.caption_width > 0.0 { app.caption_width + t.s(6.0) } else { t.s(10.0) };
    let has_request = app.viewer.request.is_some();
    let is_text = app.viewer.is_text_document();
    let is_diff = is_text && app.viewer.document().is_some_and(|d| !d.is_plain_file);
    let next = |right: &mut f32| {
        *right -= size.w;
        let r = Rect::new(*right, cy - size.h / 2.0, size.w, size.h);
        *right -= t.s(2.0);
        r
    };
    let divider = |ui: &mut Ui, right: &mut f32| {
        *right -= t.s(5.0);
        ui.vline(*right, cy - t.s(7.0), t.s(14.0), t.hairline);
        *right -= t.s(6.0);
    };

    // Buttons, from the right; in a narrow viewer the ones further left give way to the file's name.
    let min_left = rect.x + t.s(150.0);
    let fits = |right: f32, count: f32| right - count * (size.w + t.s(2.0)) - t.s(11.0) >= min_left;
    // The switches for the terminal and for find keep a short name beside them, and go after that (their
    // shortcuts and the sidebar's terminal button stay).
    let essential = |right: f32, count: f32| right - count * (size.w + t.s(2.0)) - t.s(11.0) >= rect.x + t.s(100.0);
    // The terminals' switch: lit while they show, with a dot when one rang while they were hidden.
    if app.has_terminal() && essential(right, 1.0) {
        let visible = app.terminals.visible;
        let tip = if visible { l("Hide Terminal (Alt+F12)", "隐藏终端 (Alt+F12)") } else { l("Show Terminal (Alt+F12)", "显示终端 (Alt+F12)") };
        let r = next(&mut right);
        if icon_button(ui, Id::new("viewer-terminal"), r, Icon::WindowConsole, &tip, true, visible).clicked {
            app.toggle_terminal();
        }
        if !visible && app.terminals.needs_attention(id) {
            let d = 6.0;
            let color = Color::hex(if t.dark { 0xFF9F0A } else { 0xFF9500 });
            ui.painter.fill_ellipse(Point::new(r.right() - 3.0 - d / 2.0, r.y + 3.0 + d / 2.0), d / 2.0, d / 2.0, color);
        }
        if has_request && essential(right, 1.0) {
            divider(ui, &mut right);
        }
    }
    if has_request && essential(right, 1.0) {
        let find = app.viewer.find_visible;
        if icon_button(ui, Id::new("viewer-find"), next(&mut right), Icon::Search, &l("Find in File (Ctrl+F)", "在文件中查找 (Ctrl+F)"), is_text, find).clicked {
            if find {
                app.viewer.hide_find();
            } else {
                app.viewer.show_find();
                app.focus_request = Some(ids::find_field());
            }
        }
        if is_text && fits(right, 1.0) {
            let annotating = app.viewer.annotating;
            let available = annotating || app.viewer.can_annotate();
            let r = next(&mut right);
            if icon_button(ui, Id::new("viewer-annotate"), r, Icon::Person, &l("Annotate with Git Blame", "逐行追溯 (Blame)：每行最后由谁在何时修改"), available, annotating).clicked {
                app.viewer.toggle_annotate();
            }
            if app.viewer.is_blaming {
                components::spinner(ui, Point::new(r.right() - t.s(3.0), r.bottom() - t.s(3.0)), t.s(4.0), t.text2);
            }
        }
        if is_diff && fits(right, 3.0) {
            divider(ui, &mut right);
            let ws = app.settings.ignore_whitespace;
            if icon_button(ui, Id::new("viewer-ws"), next(&mut right), Icon::TextWholeWord, &l("Ignore Whitespace (Ctrl+Alt+W)", "忽略空白差异 (Ctrl+Alt+W)"), true, ws).clicked {
                app.toggle_whitespace();
            }
            let unified = app.settings.unified;
            if icon_button(ui, Id::new("viewer-unified"), next(&mut right), Icon::SplitHorizontal, &l("Unified (Ctrl+Alt+V)", "合并视图 (Ctrl+Alt+V)"), true, unified).clicked && !unified {
                app.toggle_unified();
            }
            if icon_button(ui, Id::new("viewer-split"), next(&mut right), Icon::SplitVertical, &l("Side-by-Side (Ctrl+Alt+V)", "并排视图 (Ctrl+Alt+V)"), true, !unified).clicked && unified {
                app.toggle_unified();
            }
            if fits(right, 2.0) {
                divider(ui, &mut right);
                if icon_button(ui, Id::new("viewer-next"), next(&mut right), Icon::ArrowDown, &l("Next Difference (F7)", "下一处差异 (F7)"), true, false).clicked {
                    app.viewer.step_difference(1);
                }
                if icon_button(ui, Id::new("viewer-prev"), next(&mut right), Icon::ArrowUp, &l("Previous Difference (Shift+F7)", "上一处差异 (Shift+F7)"), true, false).clicked {
                    app.viewer.step_difference(-1);
                }
            }
        }
        let text = format!("{:.0} ms", app.viewer.load_milliseconds);
        let font = t.font_w(10.5, Weight::Medium);
        let w = ui.text_width(&text, font).ceil();
        if app.viewer.load_milliseconds > 0.0 && right - w - t.s(12.0) >= min_left + t.s(60.0) {
            // Load time, shown because being fast is the point.
            right -= w + t.s(6.0);
            let r = Rect::new(right, rect.y, w + 1.0, rect.h);
            ui.label_with(r, &text, font, t.text3, Align::Left, Overflow::Clip);
            let hover = ui.interact(Id::new("load-ms"), r);
            ui.tooltip(&hover, &l("Time to load, diff and highlight this file", "加载、比较并高亮这个文件所用的时间"));
            right -= t.s(6.0);
        }
    }

    // The file, from the left.
    let mut x = rect.x + t.s(14.0);
    let end = right - t.s(8.0);
    let Some(request) = app.viewer.request.clone() else {
        if let Some(p) = app.project(id) {
            ui.label(Rect::new(x, rect.y, (end - x).max(0.0), rect.h), &p.name, t.font_w(13.0, Weight::Medium), t.text2);
        }
        return;
    };
    components::file_icon(ui, Rect::new(x, rect.y, t.s(18.0), rect.h), &request.path, 13.0);
    x += t.s(18.0) + t.s(8.0);

    // What follows the name, measured first so the name gets what is left.
    let doc_stats = app.viewer.document().filter(|d| matches!(d.content, Content::Text) && !d.is_plain_file).map(|d| (d.additions(), d.deletions()));
    let difference = if app.viewer.hunk_count() > 0 && doc_stats.is_some() {
        let total = app.viewer.hunk_count();
        Some(match app.viewer.focused_hunk() {
            Some(f) => {
                let n = f + 1;
                lf!("{n} of {total} differences", "第 {n} / {total} 处差异")
            }
            None if total == 1 => l("1 difference", "1 处差异"),
            None => lf!("{total} differences", "{total} 处差异"),
        })
    } else {
        None
    };
    let small = t.font(11.0);
    let name = crate::core::util::file_name(&request.path).to_string();
    let directory = request.path.rfind('/').map(|i| request.path[..i].to_string()).unwrap_or_default();
    let name_font = t.font_w(13.0, Weight::Semibold);
    // What does not fit beside the name gives way: the difference count first, then the line counts, then the chip.
    let min_name = ui.text_width(&name, name_font).ceil().min(t.s(90.0));
    let difference_w = difference.as_ref().map_or(0.0, |d| ui.text_width(d, small).ceil() + t.s(8.0));
    let stats_w = if doc_stats.is_some() { t.s(70.0) } else { 0.0 };
    let mut reason = app.viewer.read_only_reason.clone();
    let reason_w = reason.as_ref().map_or(0.0, |r| ui.text_width(r, t.font(10.5)).ceil().min(t.s(200.0)) + t.s(30.0));
    let dirty_w = if app.viewer.is_dirty { t.s(15.0) } else { 0.0 };
    let avail = end - x - min_name;
    let mut difference = difference;
    let mut doc_stats = doc_stats;
    let mut tail = difference_w + stats_w + reason_w + dirty_w;
    if tail > avail {
        difference = None;
        tail -= difference_w;
    }
    if tail > avail {
        doc_stats = None;
        tail -= stats_w;
    }
    if tail > avail {
        reason = None;
        tail -= reason_w;
    }

    let room = (end - x - tail).max(t.s(40.0));
    let name_w = ui.text_width(&name, name_font).ceil().min(room);
    let label_start = x;
    ui.label_with(Rect::new(x, rect.y, name_w + 1.0, rect.h), &name, name_font, t.status(request.kind), Align::Left, Overflow::EllipsisMiddle);
    x += name_w + t.s(6.0);
    let mut rest = room - name_w - t.s(6.0);
    if let Some(old) = request.old_path.as_ref().filter(|o| **o != request.path) {
        let text = format!("← {}", crate::core::util::file_name(old));
        let w = ui.text_width(&text, t.font(11.5)).ceil().min(rest.max(0.0));
        if w > t.s(20.0) {
            ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), &text, t.font(11.5), t.text3, Align::Left, Overflow::Ellipsis);
            x += w + t.s(6.0);
            rest -= w + t.s(6.0);
        }
    }
    if !directory.is_empty() && rest > t.s(30.0) {
        let w = ui.text_width(&directory, t.font(11.5)).ceil().min(rest);
        ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), &directory, t.font(11.5), t.text3, Align::Left, Overflow::EllipsisMiddle);
        x += w + t.s(8.0);
    }
    let hover = ui.interact(Id::new("viewer-path"), Rect::new(label_start, rect.y, (x - label_start).max(0.0), rect.h));
    let tip = request.old_path.as_ref().map(|o| format!("{o} → {}", request.path)).unwrap_or_else(|| request.path.clone());
    ui.tooltip(&hover, &tip);

    if app.viewer.is_dirty {
        // Typed and not yet written; it goes to the file a moment after typing stops.
        let d = t.s(7.0);
        let dot = Rect::new(x, cy - d / 2.0, d, d);
        ui.painter.fill_ellipse(dot.center(), d / 2.0, d / 2.0, t.text2);
        let h = ui.interact(Id::new("dirty-dot"), dot.inset(-t.s(4.0)));
        ui.tooltip(&h, &l("Not saved yet · saved when you pause, or with Ctrl+S", "尚未写入 · 停止输入片刻后自动保存，或按 Ctrl+S"));
        x += d + t.s(8.0);
    }
    if let Some(reason) = &reason {
        let font = t.font(10.5);
        let w = ui.text_width(reason, font).ceil().min(t.s(200.0));
        let icon = t.s(10.0);
        let chip = Rect::new(x, cy - t.s(9.0), t.s(6.0) + icon + t.s(3.0) + w + t.s(6.0), t.s(18.0));
        ui.round(chip, chip.h / 2.0, t.text3.alpha(0.12));
        ui.icon(Icon::LockClosed, Rect::new(chip.x + t.s(6.0), cy - icon / 2.0, icon, icon), t.text3);
        ui.label_with(Rect::new(chip.x + t.s(6.0) + icon + t.s(3.0), chip.y, w + 1.0, chip.h), reason, font, t.text3, Align::Left, Overflow::Ellipsis);
        x = chip.right() + t.s(8.0);
    }
    if let Some((added, deleted)) = doc_stats {
        let font = t.mono(11.0).weight(Weight::Medium);
        let mut parts: Vec<(String, Color)> = Vec::new();
        if added > 0 {
            parts.push((format!("+{added}"), t.added));
        }
        if deleted > 0 {
            parts.push((format!("−{deleted}"), t.removed));
        }
        for (text, color) in parts {
            let w = ui.text_width(&text, font).ceil();
            ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), &text, font, color, Align::Left, Overflow::Clip);
            x += w + t.s(5.0);
        }
        x += t.s(3.0);
    }
    if let Some(d) = difference {
        ui.label(Rect::new(x, rect.y, (end - x).max(0.0), rect.h), &d, small, t.text3);
    }
}

// MARK: - Bars

/// Over a file git is merging: how many conflicts are left, which version the left side shows, and Mark as Resolved.
fn conflict_bar(app: &mut App, ui: &mut Ui, id: ProjectId, path: &str, rect: Rect) {
    let t = ui.theme;
    let count = app.viewer.conflict_count;
    let tint = if count > 0 { Color::hex(0xF0A030) } else { t.added };
    ui.fill(rect, tint.alpha(0.1));
    ui.hline(rect.x, rect.bottom() - 1.0, rect.w, t.hairline);
    let cy = rect.center().y;
    let mut x = rect.x + t.s(12.0);
    let icon = t.s(13.0);
    ui.icon(if count > 0 { Icon::Warning } else { Icon::CheckmarkCircle }, Rect::new(x, cy - icon / 2.0, icon, icon), tint);
    x += icon + t.s(10.0);
    let title = match count {
        0 => l("No conflicts left", "冲突已全部处理"),
        1 => l("1 conflict", "1 处冲突"),
        n => lf!("{n} conflicts", "{n} 处冲突"),
    };
    let font = t.font_w(12.0, Weight::Semibold);
    let w = ui.text_width(&title, font).ceil();
    ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), &title, font, t.text, Align::Left, Overflow::Clip);
    x += w + t.s(10.0);
    let left = l("Left:", "左侧：");
    let w = ui.text_width(&left, t.font(11.5)).ceil();
    ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), &left, t.font(11.5), t.text3, Align::Left, Overflow::Clip);
    x += w + t.s(6.0);

    // A segmented control: which version the left side shows.
    let current = app.project(id).map(|p| p.conflict_base).unwrap_or(ConflictBase::Head);
    let seg_font = t.font_w(11.0, Weight::Medium);
    let widths: Vec<f32> = ConflictBase::ALL.iter().map(|b| ui.text_width(&b.title(), seg_font).ceil() + t.s(16.0)).collect();
    let total: f32 = widths.iter().sum();
    let seg = Rect::new(x, cy - t.s(11.0), total + t.s(4.0), t.s(22.0));
    ui.round(seg, t.s(6.0), t.text.alpha(0.08));
    let mut sx = seg.x + t.s(2.0);
    let mut picked = None;
    for (i, base) in ConflictBase::ALL.iter().enumerate() {
        let r = Rect::new(sx, seg.y + t.s(2.0), widths[i], seg.h - t.s(4.0));
        let hit = ui.interact(Id::new("conflict-base").with(i), r);
        if *base == current {
            ui.round(r, t.s(5.0), t.elevated);
        } else if hit.hovered {
            ui.round(r, t.s(5.0), t.hover);
        }
        ui.label_with(r, &base.title(), seg_font, if *base == current { t.text } else { t.text2 }, Align::Center, Overflow::Clip);
        ui.tooltip(&hit, &l("What the left side shows: HEAD’s version, the common ancestor, or the incoming one", "左侧显示的版本：HEAD 的、共同祖先的，或传入的"));
        if hit.clicked {
            picked = Some(*base);
        }
        sx += widths[i];
    }
    if let Some(base) = picked {
        if let Some(p) = app.project_mut(id) {
            p.conflict_base = base;
        }
        app.sync_viewer();
    }

    let label = l("Mark as Resolved", "标记为已解决");
    let font = t.font_w(11.5, Weight::Semibold);
    let w = ui.text_width(&label, font).ceil() + t.s(20.0);
    let b = Rect::new(rect.right() - t.s(12.0) - w, cy - t.s(12.0), w, t.s(24.0));
    let r = ui.interact(Id::new("mark-resolved"), b);
    let (fill, color) = if count == 0 { (t.accent, Color::WHITE) } else { (t.text.alpha(0.1), t.text) };
    ui.round(b, t.s(6.0), if r.hovered { t.hover.over(fill) } else { fill });
    ui.label_with(b, &label, font, color, Align::Center, Overflow::Clip);
    if r.clicked {
        app.mark_resolved(path.to_string(), id);
    }
}

fn find_bar(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect) {
    let t = ui.theme;
    ui.fill(rect, t.panel);
    ui.hline(rect.x, rect.bottom() - 1.0, rect.w, t.hairline);
    if std::mem::take(&mut app.viewer.find_focus_request) {
        ui.focus(ids::find_field());
    }
    let cy = rect.center().y;
    let size = Size::new(t.s(26.0), t.s(24.0));
    let mut right = rect.right() - t.s(12.0);
    let mut next = || {
        right -= size.w;
        let r = Rect::new(right, cy - size.h / 2.0, size.w, size.h);
        right -= t.s(2.0);
        r
    };
    let close = next();
    let down = next();
    let up = next();
    if icon_button(ui, Id::new("find-close"), close, Icon::Dismiss, &l("Close (Esc)", "关闭 (Esc)"), true, false).clicked {
        app.viewer.hide_find();
    }
    if icon_button(ui, Id::new("find-next"), down, Icon::ArrowDown, &l("Next (F3)", "下一个 (F3)"), true, false).clicked {
        app.viewer.step_find(1);
    }
    if icon_button(ui, Id::new("find-prev"), up, Icon::ArrowUp, &l("Previous (Shift+F3)", "上一个 (Shift+F3)"), true, false).clicked {
        app.viewer.step_find(-1);
    }

    let count_text = if app.viewer.find_field.text.is_empty() {
        String::new()
    } else if app.viewer.find_count == 0 {
        l("No results", "无结果")
    } else {
        format!("{}/{}", app.viewer.find_index, app.viewer.find_count)
    };
    let font = t.font(11.0);
    let cw = ui.text_width(&count_text, font).ceil();
    let count_rect = Rect::new(up.x - t.s(8.0) - cw, rect.y, cw + 1.0, rect.h);
    ui.label_with(count_rect, &count_text, font, if app.viewer.find_count == 0 { t.removed } else { t.text3 }, Align::Left, Overflow::Clip);

    let icon = t.s(13.0);
    let x = rect.x + t.s(12.0);
    ui.icon(Icon::Search, Rect::new(x, cy - icon / 2.0, icon, icon), t.text3);
    let field = Rect::new(x + icon + t.s(8.0), cy - t.s(11.0), (count_rect.x - t.s(8.0) - x - icon - t.s(8.0)).max(t.s(40.0)), t.s(22.0));
    let id = ids::find_field();
    // Up and Down step through the matches; Enter goes on, Shift+Enter back.
    if ui.has_focus(id) {
        if let Some((key, mods)) = ui.key() {
            match key {
                Key::Down | Key::Up if mods.none() => {
                    ui.consume();
                    app.viewer.step_find(if key == Key::Down { 1 } else { -1 });
                }
                Key::Enter if mods.none() || mods == crate::input::Modifiers::SHIFT => {
                    ui.consume();
                    app.viewer.step_find(if mods.shift { -1 } else { 1 });
                }
                _ => {}
            }
        }
    }
    let outcome = components::field_input(ui, platform, id, &mut app.viewer.find_field);
    let focused = ui.has_focus(id);
    let r = ui.interact(id, field);
    if r.hovered {
        ui.set_cursor(crate::paint::CursorIcon::Text);
    }
    app.viewer.find_field.draw_text(ui, id, field, &l("Find in file", "在文件中查找"), t.font(13.0), focused, &r);
    match outcome {
        FieldOutcome::Changed => app.viewer.run_find(true),
        FieldOutcome::Cancel => app.viewer.hide_find(),
        _ => {}
    }
}

/// Revision labels above each pane.
fn pane_titles(ui: &mut Ui, rect: Rect, left: &str, right: &str, unified: bool) {
    let t = ui.theme;
    ui.fill(rect, t.window);
    ui.hline(rect.x, rect.bottom() - 1.0, rect.w, t.hairline.alpha(0.6));
    let font = t.font_w(10.5, Weight::Medium);
    let pad = t.s(12.0);
    if unified {
        ui.label(rect.inset_xy(pad, 0.0), &format!("{left} → {right}"), font, t.text3);
    } else {
        let half = (rect.w - 44.0 - 12.0) / 2.0;
        ui.label(Rect::new(rect.x + pad, rect.y, half - pad * 2.0, rect.h), left, font, t.text3);
        ui.label(Rect::new(rect.x + half + 44.0 + pad, rect.y, half - pad * 2.0, rect.h), right, font, t.text3);
    }
}

// MARK: - Overlays

fn note_capsule(ui: &mut Ui, area: Rect, note: &str) {
    let t = ui.theme;
    let font = t.font_w(11.5, Weight::Medium);
    let w = ui.text_width(note, font).ceil().min(area.w - t.s(40.0)) + t.s(24.0);
    let h = t.s(24.0);
    let r = Rect::new(area.center().x - w / 2.0, area.y + t.s(10.0), w, h).snapped(ui.painter.scale());
    ui.painter.shadow(r, h / 2.0, t.s(6.0), Color::BLACK.alpha(0.2));
    ui.round(r, h / 2.0, t.elevated);
    ui.label_with(r.inset_xy(t.s(12.0), 0.0), note, font, t.text2, Align::Center, Overflow::Ellipsis);
}

/// The commit of the line under the pointer in the Annotate column: what it was, who made it and when. Placed under
/// the line, beside the column; it never takes the pointer.
fn annotation_card(ui: &mut Ui, area: Rect, entry: &crate::core::annotations::AnnotationEntry, anchor: Point, line_h: f32) {
    let t = ui.theme;
    let width = t.s(360.0).min((area.w - 24.0).max(200.0));
    let pad = Size::new(t.s(11.0), t.s(9.0));
    let mut style = TextStyle::new(t.font_w(12.5, Weight::Semibold));
    style.max_width = Some(width - pad.w * 2.0);
    style.overflow = Overflow::Wrap;
    let summary = if entry.summary.is_empty() { entry.hash.chars().take(7).collect() } else { entry.summary.clone() };
    let title = ui.text_layout(&summary, style);
    let title_h = title.size().h.min((t.font(12.5).size * 1.35 * 3.0).ceil());
    let height = pad.h * 2.0 + title_h + t.s(5.0) + t.s(15.0) + t.s(5.0) + t.s(15.0);
    let x = (anchor.x + 4.0).max(area.x + 8.0).min((area.right() - width - 8.0).max(area.x + 8.0));
    let below = anchor.y + line_h + 4.0;
    let y = if below + height > area.bottom() { (anchor.y - height - 4.0).max(area.y + 4.0) } else { below };
    let card = Rect::new(x, y, width, height).snapped(ui.painter.scale());
    ui.with_layer(crate::ui::layer::POPUP, |ui| {
        ui.painter.shadow(card, t.s(9.0), t.s(10.0), Color::BLACK.alpha(0.25));
        ui.round(card, t.s(9.0), t.elevated);
        ui.painter.stroke_round_rect(card.inset(0.5), t.s(9.0), 1.0, t.hairline);
        let mut cy = card.y + pad.h;
        ui.clipped(Rect::new(card.x, cy, card.w, title_h), |ui| {
            ui.draw_layout(&title, Rect::new(card.x + pad.w, cy, title.size().w, title.size().h), t.text, Align::Left);
        });
        cy += title_h + t.s(5.0);
        let info = format!("{} · {} ({})", entry.author, date_time_text(entry.date), relative_text(entry.date));
        ui.label(Rect::new(card.x + pad.w, cy, card.w - pad.w * 2.0, t.s(15.0)), &info, t.font(11.0), t.text2);
        cy += t.s(15.0) + t.s(5.0);
        let hash: String = entry.hash.chars().take(7).collect();
        let hf = t.mono(10.5);
        let hw = ui.text_width(&hash, hf).ceil();
        ui.label_with(Rect::new(card.x + pad.w, cy, hw + 1.0, t.s(15.0)), &hash, hf, t.accent, Align::Left, Overflow::Clip);
        ui.label(
            Rect::new(card.x + pad.w + hw + t.s(6.0), cy, card.w - pad.w * 2.0 - hw - t.s(6.0), t.s(15.0)),
            &l("Click to show this commit", "点击查看这个提交"),
            t.font(10.5),
            t.text3,
        );
    });
}

/// Shown when nothing is open: the shortcuts worth knowing, as in an empty editor of the IDE.
fn empty_state(app: &mut App, ui: &mut Ui, id: ProjectId, area: Rect) {
    let t = ui.theme;
    let r = ui.interact(ids::canvas(), area);
    if r.pressed {
        ui.focus(ids::canvas());
    }
    let hints = [
        (l("Search Everywhere", "随处搜索"), l("Double Shift", "双击 Shift")),
        (l("Go to File", "查找文件"), "Ctrl+Shift+N".to_string()),
        (l("Find in Files", "在文件中查找"), "Ctrl+Shift+F".to_string()),
        (l("Recent Files", "最近的文件"), "Ctrl+E".to_string()),
        (l("Next / Previous Difference", "下一处 / 上一处差异"), "F7 / Shift+F7".to_string()),
        (l("Git Log", "提交历史"), "Alt+9".to_string()),
    ];
    let clean = app.project(id).is_some_and(|p| p.has_loaded && p.changes().is_empty() && p.tab == ProjectTab::Changes);
    let font = t.font(13.0);
    let row_h = t.s(28.0);
    let label_w = hints.iter().map(|(a, _)| ui.text_width(a, font).ceil()).fold(0.0, f32::max);
    let caps_w = hints.iter().map(|(_, b)| components::key_caps_width(ui, b, 12.0)).fold(0.0, f32::max);
    let grid_w = label_w + t.s(18.0) + caps_w;
    let grid_h = row_h * hints.len() as f32;
    let head_h = if clean { t.s(34.0) + t.s(8.0) + t.s(22.0) + t.s(22.0) } else { 0.0 };
    let top = area.y + ((area.h - grid_h - head_h) / 2.0).max(t.s(20.0));
    let mut y = top;
    if clean {
        let s = t.s(34.0);
        ui.icon(Icon::CheckmarkCircle, Rect::new(area.center().x - s / 2.0, y, s, s), t.added.alpha(0.85));
        y += s + t.s(8.0);
        ui.label_with(Rect::new(area.x, y, area.w, t.s(22.0)), &l("Working tree clean", "工作区很干净"), t.font_w(15.0, Weight::Medium), t.text2, Align::Center, Overflow::Ellipsis);
        y += t.s(22.0) + t.s(22.0);
    }
    let x = area.center().x - grid_w / 2.0;
    for (label, caps) in hints {
        ui.label_with(Rect::new(x, y, label_w + 1.0, row_h), &label, font, t.text2, Align::Left, Overflow::Clip);
        components::key_caps(ui, x + label_w + t.s(18.0), y + row_h / 2.0, &caps, 12.0);
        y += row_h;
    }
    if r.hovered && matches!(ui.event, Some(Event::MouseMove(_))) {
        ui.set_cursor(crate::paint::CursorIcon::Arrow);
    }
}

/// Before and after for image files.
fn image_diff(app: &mut App, vs: &mut ViewState, ui: &mut Ui, area: Rect) {
    let t = ui.theme;
    let Some(doc) = app.viewer.document() else { return };
    let Content::Image { old, new } = &doc.content else { return };
    let key = (old.as_ref().map_or(0, |v| v.as_ptr() as usize), new.as_ref().map_or(0, |v| v.as_ptr() as usize));
    let cache = &mut vs.images;
    if cache.key != key {
        cache.key = key;
        cache.old = old.as_ref().map(|b| ImageSource::new(Arc::new(b.clone())));
        cache.new = new.as_ref().map(|b| ImageSource::new(Arc::new(b.clone())));
    }
    let left = if doc.left_title.is_empty() { l("Before", "之前") } else { doc.left_title.clone() };
    let right = if doc.right_title.is_empty() { l("Current", "当前") } else { doc.right_title.clone() };
    let panes: Vec<(ImageSource, String, Color)> = [(cache.old.clone(), left, t.removed), (cache.new.clone(), right, t.added)]
        .into_iter()
        .filter_map(|(s, title, tint)| s.map(|s| (s, title, tint)))
        .collect();
    if panes.is_empty() {
        return;
    }
    let inner = area.inset(t.s(20.0));
    let gap = t.s(16.0);
    let w = (inner.w - gap * (panes.len() as f32 - 1.0)) / panes.len() as f32;
    for (i, (source, title, tint)) in panes.into_iter().enumerate() {
        let pane = Rect::new(inner.x + i as f32 * (w + gap), inner.y, w, inner.h);
        image_pane(ui, pane, &source, &title, tint);
    }
}

fn image_pane(ui: &mut Ui, rect: Rect, source: &ImageSource, title: &str, tint: Color) {
    let t = ui.theme;
    let caption_h = t.s(26.0);
    let frame = Rect::new(rect.x, rect.y, rect.w, rect.h - caption_h);
    let pixels = ui.backend.image_size(source);
    // The transparency grid behind it.
    ui.clipped(frame, |ui| {
        ui.fill(frame, t.panel);
        let cell = 10.0;
        let mut yy = frame.y;
        let mut row = 0;
        while yy < frame.bottom() {
            let mut xx = frame.x + if row % 2 == 0 { 0.0 } else { cell };
            while xx < frame.right() {
                ui.fill(Rect::new(xx, yy, cell, cell), t.hover);
                xx += cell * 2.0;
            }
            yy += cell;
            row += 1;
        }
    });
    match pixels {
        Some(px) if px.w > 0.0 && px.h > 0.0 => {
            // Fit the pane, but never magnify beyond 4x: icons stay inspectable, nothing turns to mush.
            let dip = 1.0 / ui.painter.scale();
            let fit = ((frame.w - 24.0) / (px.w * dip)).min((frame.h - 24.0) / (px.h * dip));
            let scale = fit.clamp(0.01, 4.0);
            let size = Size::new(px.w * dip * scale, px.h * dip * scale);
            ui.painter.image(source, frame.centered(size).snapped(ui.painter.scale()));
            let pct = format!("{}%", (scale * 100.0).round() as i32);
            let font = t.font_w(10.5, Weight::Medium);
            let pw = ui.text_width(&pct, font).ceil() + t.s(12.0);
            let badge = Rect::new(frame.right() - t.s(8.0) - pw, frame.bottom() - t.s(8.0) - t.s(18.0), pw, t.s(18.0));
            ui.round(badge, badge.h / 2.0, t.elevated.alpha(0.9));
            ui.label_with(badge, &pct, font, t.text2, Align::Center, Overflow::Clip);
        }
        _ => {
            ui.label_with(frame, &l("Cannot preview this image", "无法预览该图片"), t.font(12.0), t.text3, Align::Center, Overflow::Ellipsis);
        }
    }
    ui.painter.stroke_round_rect(frame.inset(0.5), t.s(10.0), 1.0, tint.alpha(0.5));
    let mut caption = vec![title.to_string()];
    if let Some(px) = pixels {
        caption.push(format!("{} × {}", px.w as i64, px.h as i64));
    }
    caption.push(byte_size(source.bytes.len()));
    let text = caption.join("   ");
    let cap = Rect::new(rect.x, frame.bottom() + t.s(6.0), rect.w, caption_h - t.s(6.0));
    let runs = vec![crate::paint::ColorRun { range: 0..title.len() as u32, color: tint }];
    let mut style = TextStyle::new(t.font(11.5));
    style.max_width = Some(cap.w);
    style.overflow = Overflow::Ellipsis;
    let layout = ui.layout(crate::paint::TextRequest { text: Arc::from(text.as_str()), style, runs, bold_ranges: std::iter::once(0..title.len() as u32).collect() });
    ui.draw_layout(&layout, cap, t.text2, Align::Center);
}

fn byte_size(n: usize) -> String {
    let n = n as f64;
    if n < 1000.0 {
        format!("{n:.0} bytes")
    } else if n < 1_000_000.0 {
        format!("{:.1} KB", n / 1000.0)
    } else {
        format!("{:.1} MB", n / 1_000_000.0)
    }
}
