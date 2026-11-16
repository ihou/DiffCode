//! The project's files as a tree, in the sidebar under the project list. Built to stay readable in a narrow column:
//! single-child directory chains share a row, indentation tightens as the tree gets deeper, folders are a chevron
//! and a name with no second icon, and a path bar says where the top row lives.

use super::components::{self, ListView, ViewState, LIST_PAD};
use crate::app::edits::{can_delete, can_rollback};
use crate::app::file_tree::{self, TreeRow};
use crate::app::project::ProjectId;
use crate::app::{ids, App, MenuItem, PlatformRequest};
use crate::core::git::ChangeKind;
use crate::core::lang::l;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::input::Key;
use crate::paint::{ColorRun, Overflow, TextRequest, TextStyle, Weight};
use crate::platform::Platform;
use crate::text_field::FieldOutcome;
use crate::ui::{Align, Id, Ui};
use crate::widgets::icon_button;

const ROW_HEIGHT: f32 = 22.0;
const LEADING: f32 = 8.0;

pub fn file_tree(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect) {
    let Some(id) = app.selected else { return };
    let t = ui.theme;
    let mut area = rect;

    // The filter and its buttons.
    let top = area.take_top(t.s(36.0));
    let b = t.s(26.0);
    let collapse = Rect::new(top.right() - t.s(6.0) - b, top.y + t.s(9.0), b, t.s(24.0));
    let locate = collapse.offset(-(b + t.s(1.0)), 0.0);
    let field_rect = Rect::new(top.x + t.s(8.0), top.y + t.s(8.0), locate.x - t.s(4.0) - top.x - t.s(8.0), t.s(26.0));
    let follow = app.settings.follow_file;
    let r = icon_button(ui, Id::new("tree-locate"), locate, Icon::Target, &l("Select Opened File (Alt+F1)", "定位当前文件 (Alt+F1)"), true, follow);
    if r.clicked {
        app.locate_in_tree();
    }
    if r.secondary {
        app.open_menu(
            ui.mouse(),
            vec![MenuItem::new(l("Always Select Opened File", "始终定位打开的文件"), |app| {
                app.settings.follow_file = !app.settings.follow_file;
                app.settings.save();
            })
            .checked(follow)],
        );
    }
    if icon_button(ui, Id::new("tree-collapse"), collapse, Icon::ArrowCollapseAll, &l("Collapse All", "全部折叠"), true, false).clicked {
        if let Some(p) = app.project_mut(id) {
            p.tree_expanded.clear();
        }
    }
    let Some(p) = app.project_mut(id) else { return };
    let was_filtering = !p.tree_filter.text.is_empty();
    let outcome = components::filter_field(ui, platform, Id::new("tree-filter"), field_rect, &mut p.tree_filter, &l("Filter files", "筛选文件"));
    if outcome == FieldOutcome::Changed {
        filter_changed(app, id, was_filtering);
    } else if matches!(outcome, FieldOutcome::Submit | FieldOutcome::Cancel) {
        if outcome == FieldOutcome::Cancel {
            if let Some(p) = app.project_mut(id) {
                p.tree_filter.set_text("");
            }
            filter_changed(app, id, was_filtering);
        }
        app.focus_request = Some(ids::tree());
    }

    if vs.appeared(Id::new("file-tree").with(id)) {
        if let Some(p) = app.project_mut(id) {
            p.load_tree();
        }
    }
    let Some(p) = app.project(id) else { return };
    if p.tree_index().is_none() {
        if p.is_loading_tree {
            components::spinner(ui, area.center(), t.s(8.0), t.text3);
        }
        return;
    }
    let filtering = !p.tree_filter.text.trim().is_empty();
    let rows = p.tree_rows();
    if rows.is_empty() {
        let (icon, title) = if filtering { (Icon::Filter, l("No files match", "没有匹配的文件")) } else { (Icon::Folder, l("No files", "没有文件")) };
        components::empty_state(ui, area, icon, &title, "");
        return;
    }
    let list_key = ids::tree().with(("memory", id));
    let row_h = t.s(ROW_HEIGHT);
    if !filtering {
        let bar = area.take_top(t.s(20.0));
        let top_index = ((vs.list(list_key).scroll.offset - t.s(LIST_PAD)) / row_h).max(0.0) as usize;
        if let Some(target) = path_bar(ui, bar, &rows, top_index.min(rows.len() - 1), &p.name) {
            if let Some(p) = app.project_mut(id) {
                p.tree_selected_id = Some(target);
                p.tree_reveal = true;
            }
            return;
        }
    }

    let Some(p) = app.project(id) else { return };
    let deepest = rows.iter().map(|r| r.depth).max().unwrap_or(0).max(1) as f32;
    let indent = t.s(12.0).min((app.settings.sidebar_width * 0.3 / deepest).floor().max(5.0));
    let selected = p.tree_selected_id.clone();
    let guide = if filtering { None } else { active_guide(&rows, selected.as_deref()) };
    let on_screen = app.viewer.request.as_ref().filter(|r| r.root == p.url).map(|r| TreeRow::file_id(&r.path));
    let flash = p.tree_flash.as_ref().map(|(f, _)| f.clone());
    let center = p.tree_reveal;
    let row_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let focused = ui.has_focus(ids::tree());

    // Typing in the tree filters it (speed search).
    let mut filter_text_changed = false;
    if let Some(p) = app.project_mut(id) {
        filter_text_changed = components::speed_search(ui, focused, &mut p.tree_filter);
    }
    if filter_text_changed {
        filter_changed(app, id, was_filtering);
        return;
    }

    let view = ListView::new(ids::tree(), area, row_h, &row_ids, selected.as_deref()).center(center).draggable();
    let result = components::list(ui, vs.list(list_key), view, |ui, i, rect, _selected, _focused| {
        let row = &rows[i];
        let guide_level = guide.as_ref().and_then(|(level, prefix)| (row.depth > *level && row.path.starts_with(prefix.as_str())).then_some(*level));
        draw_row(ui, row, rect, indent, filtering, guide_level, flash.as_deref() == Some(row.id.as_str()), on_screen.as_deref() == Some(row.id.as_str()));
    });
    if center {
        if let Some(p) = app.project_mut(id) {
            p.tree_reveal = false;
        }
    }

    if let Some(Some(sel)) = result.select.clone() {
        if let Some(p) = app.project_mut(id) {
            p.tree_selected_id = Some(sel);
        }
        app.tree_selection_changed(id);
    }
    if let Some(i) = result.clicked {
        if rows[i].is_directory {
            toggle(app, id, &rows[i]);
        }
    }
    if let Some(i) = result.drag {
        app.drag_out(id, vec![rows[i].path.clone()]);
    }
    if let Some(i) = result.double_clicked {
        if !rows[i].is_directory {
            open(app, id, &rows[i]);
        }
    }
    if let Some((i, at)) = result.menu {
        let items = row_menu(app, id, &rows[i]);
        app.open_menu(at, items);
    }
    if let Some((key, mods, index)) = result.key {
        if mods.none() && handle_key(app, id, key, index.map(|i| &rows[i]), &rows, filtering) {
            ui.consume();
        }
    }
}

fn filter_changed(app: &mut App, id: ProjectId, was_filtering: bool) {
    let Some(p) = app.project_mut(id) else { return };
    p.update_tree_filter();
    // Leaving the filter: show the chosen file where it lives.
    if p.tree_filter.text.is_empty() && was_filtering {
        if let Some(path) = p.tree_selected_id.clone().and_then(|s| s.strip_prefix("f:").map(str::to_string)) {
            p.reveal_in_tree(&path, false);
        }
    }
}

/// The guide line to accent: the one that runs down from the selected row's parent.
fn active_guide(rows: &[TreeRow], selected: Option<&str>) -> Option<(usize, String)> {
    let row = rows.iter().find(|r| Some(r.id.as_str()) == selected)?;
    if row.depth == 0 {
        return None;
    }
    Some((row.depth - 1, format!("{}/", row.parent_path())))
}

/// Shows a file and hands it the keyboard.
fn open(app: &mut App, id: ProjectId, row: &TreeRow) {
    if let Some(p) = app.project_mut(id) {
        p.tree_selected_id = Some(row.id.clone());
    }
    app.tree_selection_changed(id);
    app.focus_viewer();
}

fn toggle(app: &mut App, id: ProjectId, row: &TreeRow) {
    if !row.is_directory {
        return;
    }
    if let Some(p) = app.project_mut(id) {
        if !p.tree_expanded.remove(&row.path) {
            p.tree_expanded.insert(row.path.clone());
        }
    }
}

fn handle_key(app: &mut App, id: ProjectId, key: Key, row: Option<&TreeRow>, rows: &[TreeRow], filtering: bool) -> bool {
    match key {
        // Close the folder, or step out to the one above.
        Key::Left => {
            let Some(row) = row.filter(|_| !filtering) else { return true };
            if row.is_directory && row.expanded {
                if let Some(p) = app.project_mut(id) {
                    p.tree_expanded.remove(&row.path);
                }
            } else if let Some(index) = rows.iter().position(|r| r.id == row.id) {
                if let Some(parent) = rows[..index].iter().rev().find(|r| r.depth < row.depth) {
                    if let Some(p) = app.project_mut(id) {
                        p.tree_selected_id = Some(parent.id.clone());
                    }
                }
            }
            true
        }
        // Open the folder, or step into it.
        Key::Right => {
            let Some(row) = row.filter(|r| r.is_directory && !filtering) else { return true };
            if !row.expanded {
                if let Some(p) = app.project_mut(id) {
                    p.tree_expanded.insert(row.path.clone());
                }
            } else if let Some(index) = rows.iter().position(|r| r.id == row.id) {
                if let Some(next) = rows.get(index + 1) {
                    if let Some(p) = app.project_mut(id) {
                        p.tree_selected_id = Some(next.id.clone());
                    }
                }
            }
            true
        }
        Key::Enter => {
            match row {
                Some(r) if r.is_directory => toggle(app, id, r),
                Some(r) => open(app, id, r),
                None => {}
            }
            true
        }
        _ => false,
    }
}

fn row_menu(app: &mut App, id: ProjectId, row: &TreeRow) -> Vec<MenuItem> {
    let Some(p) = app.project(id) else { return Vec::new() };
    let full = p.url.join(row.path.replace('/', "\\"));
    let mut items = Vec::new();
    if row.is_directory {
        let r = row.clone();
        items.push(MenuItem::new(if row.expanded { l("Collapse", "折叠") } else { l("Expand", "展开") }, move |app| toggle(app, id, &r)));
        items.push(MenuItem::new(l("Collapse All", "全部折叠"), move |app| {
            if let Some(p) = app.project_mut(id) {
                p.tree_expanded.clear();
            }
        }));
        if app.has_terminal() {
            items.push(MenuItem::separator());
            let dir = full.clone();
            items.push(MenuItem::new(l("Open Terminal Here", "在这里打开终端"), move |app| {
                app.open_terminal(id, Some(dir));
            }));
        }
    } else {
        let r = row.clone();
        items.push(MenuItem::new(l("Open", "打开"), move |app| open(app, id, &r)));
        if !matches!(row.change.as_ref().map(|c| c.kind), Some(ChangeKind::Untracked | ChangeKind::Added)) {
            let path = row.path.clone();
            items.push(MenuItem::new(l("Show History", "查看历史"), move |app| app.show_history_of_path(&path, id)));
        }
        if app.has_terminal() {
            let path = row.path.clone();
            items.push(MenuItem::new(l("Insert Path into Terminal", "把路径插入终端"), move |app| app.insert_path_in_terminal(&path, id)));
        }
        if let Some(change) = &row.change {
            items.push(MenuItem::separator());
            if can_rollback(change) {
                let c = change.clone();
                items.push(MenuItem::new(l("Rollback…", "回滚…"), move |app| app.rollback(vec![c], id, true)));
            }
            if can_delete(change) {
                let c = change.clone();
                items.push(MenuItem::new(l("Delete…", "删除…"), move |app| app.delete(vec![c], id, true)));
            }
        }
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::new(l("Reveal in File Explorer", "在资源管理器中显示"), move |app| app.requests.push(PlatformRequest::Reveal(full))));
    let path = row.path.clone();
    items.push(MenuItem::new(l("Copy Path", "复制路径"), move |app| app.copy_text(path)));
    items
}

/// One line that names the folder the top of the list is inside, so a thinly indented tree never loses its place.
/// Returns the row to select when it is clicked.
fn path_bar(ui: &mut Ui, rect: Rect, rows: &[TreeRow], top: usize, root: &str) -> Option<String> {
    let t = ui.theme;
    let enclosing = file_tree::enclosing(rows, top).pop();
    let r = ui.interact(Id::new("tree-path-bar"), rect);
    let icon = t.s(11.0);
    let x = rect.x + t.s(12.0);
    ui.icon(if enclosing.is_some() { Icon::Folder } else { Icon::Cube }, Rect::new(x, rect.center().y - icon / 2.0, icon, icon), t.text3);
    let text = enclosing.as_ref().map(|e| e.path.replace('/', " › ")).unwrap_or_else(|| root.to_string());
    let tr = Rect::new(x + icon + t.s(5.0), rect.y, rect.right() - x - icon - t.s(17.0), rect.h);
    ui.label_with(tr, &text, t.font_w(10.5, Weight::Medium), t.text3, Align::Left, Overflow::EllipsisMiddle);
    ui.hline(rect.x, rect.bottom() - 1.0, rect.w, t.hairline.alpha(0.7));
    let tip = enclosing.as_ref().map(|e| e.path.clone()).unwrap_or_else(|| root.to_string());
    ui.tooltip(&r, &tip);
    if r.clicked {
        return enclosing.map(|e| e.id);
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn draw_row(ui: &mut Ui, row: &TreeRow, rect: Rect, indent: f32, filtering: bool, guide_level: Option<usize>, flashing: bool, on_screen: bool) {
    let t = ui.theme;
    let leading = t.s(LEADING);
    if flashing {
        ui.round(rect.inset_xy(t.s(6.0), 0.0), t.s(6.0), t.accent.alpha(0.32));
    }
    // Hairlines that run down from each open folder's chevron.
    if row.depth > 0 {
        let origin = rect.x + leading + t.s(12.0) / 2.0;
        for level in 0..row.depth {
            let x = (origin + level as f32 * indent).round();
            let color = if Some(level) == guide_level { t.accent.alpha(0.8) } else { t.text3.alpha(0.28) };
            ui.vline(x, rect.y, rect.h, color);
        }
    }
    let x = rect.x + leading + row.depth as f32 * indent;
    let right = rect.right() - t.s(12.0);
    let cy = rect.center().y;
    let slot = t.s(12.0);
    if row.is_directory {
        let s = t.s(10.0);
        ui.icon(if row.expanded { Icon::ChevronDown } else { Icon::ChevronRight }, Rect::new(x + (slot - s) / 2.0, cy - s / 2.0, s, s), t.text3);
        let mut end = right;
        if row.changed_below > 0 {
            let color = t.status(Some(ChangeKind::Modified));
            let font = t.font_w(10.0, Weight::Semibold);
            let text = row.changed_below.to_string();
            let w = ui.text_width(&text, font).ceil() + t.s(10.0);
            let h = t.s(15.0);
            let b = Rect::new(right - w, cy - h / 2.0, w, h);
            ui.round(b, h / 2.0, color.alpha(0.14));
            ui.label_with(b, &text, font, color, Align::Center, Overflow::Clip);
            end = b.x - t.s(4.0);
        }
        // "app/src/main" keeps its last folder at full strength and lets the rest recede.
        let name_x = x + slot + t.s(5.0);
        let lead = row.name.rfind('/').map_or(0, |i| i + 1);
        let mut style = TextStyle::new(t.font_w(12.5, Weight::Medium));
        style.max_width = Some((end - name_x).max(1.0));
        style.overflow = Overflow::EllipsisMiddle;
        let runs = if lead > 0 { vec![ColorRun { range: 0..lead as u32, color: t.text3 }] } else { Vec::new() };
        let layout = ui.layout(TextRequest { text: std::sync::Arc::from(row.name.as_str()), style, runs, bold_ranges: Vec::new() });
        ui.draw_layout(&layout, Rect::new(name_x, rect.y, end - name_x, rect.h), t.text, Align::Left);
        return;
    }
    components::file_icon(ui, Rect::new(x, rect.y, slot, rect.h), &row.path, 11.5);
    let mut end = right;
    if let Some(change) = &row.change {
        let font = t.font_w(10.5, Weight::Bold);
        ui.label_with(Rect::new(right - slot, rect.y, slot, rect.h), change.kind.letter(), font, t.status(Some(change.kind)), Align::Center, Overflow::Clip);
        end = right - slot - t.s(4.0);
    }
    let name_x = x + slot + t.s(5.0);
    let font = t.font_w(12.5, if on_screen { Weight::Semibold } else { Weight::Regular });
    let base = t.status(row.change.as_ref().map(|c| c.kind));
    let offset = row.path.len() - row.name.len();
    let name_w = ui.text_width(&row.name, font).ceil().min((end - name_x).max(0.0));
    let layout = components::fuzzy_label(ui, Rect::new(name_x, rect.y, name_w + 1.0, rect.h), &row.name, &row.positions, offset, base, font, Overflow::EllipsisMiddle);
    if row.change.as_ref().is_some_and(|c| c.kind == ChangeKind::Deleted) {
        let w = layout.size().w;
        ui.hline(name_x, cy, w, t.status(Some(ChangeKind::Deleted)).alpha(0.7));
    }
    if filtering {
        let directory = row.path.rfind('/').map(|i| &row.path[..i]).unwrap_or("");
        if !directory.is_empty() {
            let dx = name_x + name_w + t.s(5.0);
            if end - dx > t.s(20.0) {
                // Keeps both ends: the module a file belongs to and the folder it sits in.
                components::fuzzy_label(ui, Rect::new(dx, rect.y, end - dx, rect.h), directory, &row.positions, 0, t.text3, t.font(11.0), Overflow::EllipsisMiddle);
            }
        }
    }
}
