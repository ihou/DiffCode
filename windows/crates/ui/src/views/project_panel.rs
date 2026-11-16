//! The middle column: a project's local changes (or a comparison, or a stash), or its commit log.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::commit_panel;
use super::components::{self, ListView, ViewState};
use crate::app::change_tree::{self, ChangeRow, RowKind};
use crate::app::edits::{can_delete, can_rollback};
use crate::app::project::{Comparison, ProjectId, ProjectTab};
use crate::app::{ids, App, MenuItem, PlatformRequest};
use crate::canvas::Inclusion;
use crate::core::git::{ChangeKind, Commit, FileChange};
use crate::core::history_edits::StashEntry;
use crate::core::lang::l;
use crate::core::lf;
use crate::core::remote::RepoOperation;
use crate::core::util::{date_time_text, relative_text};
use crate::geom::{Color, Point, Rect, Size};
use crate::icons::Icon;
use crate::input::Key;
use crate::paint::{Overflow, Weight};
use crate::platform::Platform;
use crate::text_field::FieldOutcome;
use crate::ui::{Align, Id, Ui};
use crate::widgets::{icon_button, splitter, Check};

pub fn project_panel(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect) {
    let Some(id) = app.selected else { return };
    let t = ui.theme;
    ui.fill(rect, t.panel);
    let mut area = rect;
    let tabs_rect = area.take_top(t.s(38.0));
    tabs(app, ui, id, tabs_rect);
    ui.hline(area.x, area.y, area.w, t.hairline);
    area.take_top(1.0);
    let Some(p) = app.project(id) else { return };
    match p.tab {
        ProjectTab::Changes if p.comparison.is_some() => comparison_list(app, vs, ui, platform, id, area),
        ProjectTab::Changes => changes_list(app, vs, ui, platform, id, area),
        ProjectTab::Log => log_panel(app, vs, ui, platform, id, area),
    }
}

// MARK: - Tabs

/// The top row: the two tabs, then the list's own buttons. With little room it sheds the buttons that have a
/// shortcut elsewhere, least needed first.
fn tabs(app: &mut App, ui: &mut Ui, id: ProjectId, rect: Rect) {
    let t = ui.theme;
    let Some(p) = app.project(id) else { return };
    let tab = p.tab;
    let comparing = p.comparison.is_some();
    let count = if comparing { p.compare_changes.len() } else { p.changes().len() };
    let refreshing = p.is_refreshing;
    let mut x = rect.x + t.s(8.0);
    let cy = rect.center().y;
    if tab_button(ui, &mut x, cy, &l("Changes", "变更"), Some(count), "Alt+0", tab == ProjectTab::Changes) {
        app.set_tab(ProjectTab::Changes);
    }
    x += t.s(4.0);
    if tab_button(ui, &mut x, cy, &l("Log", "历史"), None, "Alt+9", tab == ProjectTab::Log) {
        app.set_tab(ProjectTab::Log);
    }

    let size = Size::new(t.s(26.0), t.s(24.0));
    let room = rect.right() - t.s(8.0) - x;
    let all = room >= size.w * 4.0 + t.s(16.0);
    let some = room >= size.w + t.s(8.0);
    let mut right = rect.right() - t.s(8.0);
    let next = |right: &mut f32| {
        *right -= size.w;
        let r = Rect::new(*right, cy - size.h / 2.0, size.w, size.h);
        *right -= t.s(4.0);
        r
    };
    if all {
        let flat = app.settings.flat_list;
        let r = next(&mut right);
        let icon = if flat { Icon::List } else { Icon::TextBulletListTree };
        if icon_button(ui, Id::new("tab-flat"), r, icon, &l("Toggle Tree / Flat List", "切换 目录树 / 平铺列表"), true, false).clicked {
            app.settings.flat_list = !app.settings.flat_list;
            app.settings.save();
        }
        if tab == ProjectTab::Changes {
            let r = next(&mut right);
            if icon_button(ui, Id::new("tab-stashes"), r, Icon::Archive, &l("Stashes…", "Stash 列表…"), true, false).clicked {
                app.show_stashes();
            }
        }
    }
    if some {
        // The spinner takes the button's place, so nothing shifts while a refresh runs.
        let r = next(&mut right);
        if refreshing {
            components::spinner(ui, r.center(), t.s(6.0), t.text2);
        } else if icon_button(ui, Id::new("tab-refresh"), r, Icon::ArrowClockwise, &l("Refresh (Ctrl+Alt+Y)", "刷新 (Ctrl+Alt+Y)"), true, false).clicked {
            app.refresh_current();
        }
    }
    if all && tab == ProjectTab::Changes && !comparing {
        let targets: Vec<FileChange> = app.selected_changes(id).into_iter().filter(can_rollback).collect();
        let r = next(&mut right);
        if icon_button(ui, Id::new("tab-rollback"), r, Icon::ArrowUndo, &l("Rollback… (Ctrl+Alt+Z)", "回滚… (Ctrl+Alt+Z)"), !targets.is_empty(), false).clicked {
            app.rollback(targets, id, true);
        }
    }
}

fn tab_button(ui: &mut Ui, x: &mut f32, cy: f32, title: &str, count: Option<usize>, shortcut: &str, selected: bool) -> bool {
    let t = ui.theme;
    let font = t.font_w(12.0, if selected { Weight::Semibold } else { Weight::Medium });
    let count_font = t.font_w(10.5, Weight::Semibold);
    let title_w = ui.text_width(title, font).ceil();
    let count_text = count.filter(|c| *c > 0).map(|c| c.to_string());
    let count_w = count_text.as_ref().map(|c| ui.text_width(c, count_font).ceil() + t.s(5.0)).unwrap_or(0.0);
    let w = t.s(9.0) * 2.0 + title_w + count_w;
    let h = t.s(24.0);
    let rect = Rect::new(*x, cy - h / 2.0, w, h);
    let r = ui.interact(Id::new("tab").with(title), rect);
    if selected {
        ui.round(rect, t.s(6.0), t.chip);
    } else if r.hovered {
        ui.round(rect, t.s(6.0), t.hover);
    }
    let tx = rect.x + t.s(9.0);
    ui.label_with(Rect::new(tx, rect.y, title_w + 1.0, h), title, font, if selected { t.text } else { t.text2 }, Align::Left, Overflow::Clip);
    if let Some(c) = count_text {
        ui.label_with(
            Rect::new(tx + title_w + t.s(5.0), rect.y, count_w, h),
            &c,
            count_font,
            if selected { t.accent } else { t.text3 },
            Align::Left,
            Overflow::Clip,
        );
    }
    ui.tooltip(&r, &format!("{title} ({shortcut})"));
    *x += w;
    r.clicked
}

// MARK: - Change rows

/// Where a click on a row landed besides selecting it.
#[derive(Default)]
struct RowClicks {
    chevron: Option<usize>,
    tick: Option<usize>,
}

/// One row of a list of changed files: a group, a folder or a file. `inclusion`: the commit tick, where there is
/// something to commit.
#[allow(clippy::too_many_arguments)]
fn draw_change_row(
    ui: &mut Ui,
    list: Id,
    row: &ChangeRow,
    index: usize,
    rect: Rect,
    show_directory: bool,
    inclusion: Option<Inclusion>,
    clicks: &mut RowClicks,
) {
    let t = ui.theme;
    let cy = rect.center().y;
    let gap = t.s(5.0);
    let mut x = rect.x + row.depth as f32 * t.s(12.0) + t.s(6.0) + gap;
    let right = rect.right() - t.s(14.0);
    let slot = t.s(12.0);
    let chevron = |ui: &mut Ui, x: &mut f32, clicks: &mut RowClicks| {
        let s = t.s(10.0);
        let r = Rect::new(*x, cy - t.s(8.0), slot, t.s(16.0));
        let hit = ui.interact(list.with(("chevron", &row.id)), r.inset(-t.s(2.0)));
        ui.icon(if row.expanded { Icon::ChevronDown } else { Icon::ChevronRight }, r.centered(Size::new(s, s)), if hit.hovered { t.text2 } else { t.text3 });
        if hit.pressed {
            clicks.chevron = Some(index);
        }
        *x += slot + gap;
    };
    let tick = |ui: &mut Ui, x: &mut f32, clicks: &mut RowClicks| {
        if let Some(state) = inclusion {
            let r = Rect::new(*x - t.s(3.0), cy - t.s(9.0), t.s(18.0), t.s(18.0));
            let hit = ui.interact(list.with(("tick", &row.id)), r);
            let check = match state {
                Inclusion::On => Check::On,
                Inclusion::Off => Check::Off,
                Inclusion::Mixed => Check::Mixed,
            };
            crate::widgets::draw_checkbox(ui, r, check, true, hit.hovered);
            let tip = if state != Inclusion::Off { l("Leave out of the commit (Space)", "不提交 (空格)") } else { l("Include in the commit (Space)", "提交 (空格)") };
            ui.tooltip(&hit, &tip);
            if hit.pressed {
                clicks.tick = Some(index);
            }
            *x += t.s(12.0) + gap;
        }
    };
    match &row.kind {
        RowKind::Group { title, count } => {
            chevron(ui, &mut x, clicks);
            tick(ui, &mut x, clicks);
            let font = t.font_w(12.0, Weight::Semibold);
            let w = ui.text_width(title, font).ceil().min((right - x).max(0.0));
            ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), title, font, t.text, Align::Left, Overflow::Ellipsis);
            x += w + gap;
            let files = if *count == 1 { l("1 file", "1 个文件") } else { lf!("{count} files", "{count} 个文件") };
            ui.label(Rect::new(x, rect.y, (right - x).max(0.0), rect.h), &files, t.font(11.0), t.text3);
        }
        RowKind::Directory { name, count } => {
            chevron(ui, &mut x, clicks);
            tick(ui, &mut x, clicks);
            let s = t.s(13.0);
            ui.icon(Icon::Folder, Rect::new(x, cy - s / 2.0, s, s), Color::hex(0x7C9CCB));
            x += s + gap;
            let count_text = count.to_string();
            let cw = ui.text_width(&count_text, t.font(10.5)).ceil();
            let font = t.font(12.5);
            let w = ui.text_width(name, font).ceil().min((right - x - cw - gap).max(0.0));
            ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), name, font, t.text2, Align::Left, Overflow::EllipsisMiddle);
            x += w + gap;
            ui.label_with(Rect::new(x, rect.y, cw + 1.0, rect.h), &count_text, t.font(10.5), t.text3, Align::Left, Overflow::Clip);
        }
        RowKind::File(change) => {
            x += slot + gap;
            tick(ui, &mut x, clicks);
            components::file_icon(ui, Rect::new(x, rect.y, t.s(17.0), rect.h), &change.path, 12.0);
            x += t.s(17.0) + gap;
            // The end of the row: the counts and the status letter.
            let letter_w = t.s(14.0);
            ui.label_with(Rect::new(right - letter_w, rect.y, letter_w, rect.h), change.kind.letter(), t.font_w(10.5, Weight::Bold), t.status(Some(change.kind)), Align::Center, Overflow::Clip);
            let stats = components::stat_badge(ui, right - letter_w - t.s(4.0), cy, change.additions, change.deletions, 10.5);
            let end = stats - gap;
            let font = t.font(12.5);
            let name = change.name();
            let w = ui.text_width(name, font).ceil().min((end - x).max(0.0));
            let layout = ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), name, font, t.status(Some(change.kind)), Align::Left, Overflow::EllipsisMiddle);
            if change.kind == ChangeKind::Deleted {
                ui.hline(x, cy, layout.size().w, t.status(Some(ChangeKind::Deleted)).alpha(0.7));
            }
            x += w + gap;
            let directory = change.directory();
            if show_directory && !directory.is_empty() && end - x > t.s(16.0) {
                ui.label_with(Rect::new(x, rect.y, end - x, rect.h), directory, t.font(11.0), t.text3, Align::Left, Overflow::EllipsisMiddle);
            }
        }
    }
}

fn row_tip(row: &ChangeRow) -> String {
    match row.file() {
        Some(c) => match &c.old_path {
            Some(old) => format!("{old} → {}", c.path),
            None => c.path.clone(),
        },
        None => String::new(),
    }
}

/// ← collapses (or moves to the parent row), → expands, Enter opens; shared by the lists of changed files.
fn tree_key(key: Key, row: Option<&ChangeRow>, rows: &[ChangeRow], collapsed: &mut HashSet<String>) -> Option<TreeKey> {
    match key {
        Key::Left => {
            let row = row?;
            if row.file().is_none() && row.expanded {
                collapsed.insert(row.id.clone());
            } else if let Some(index) = rows.iter().position(|r| r.id == row.id) {
                if let Some(parent) = rows[..index].iter().rev().find(|r| r.depth < row.depth) {
                    return Some(TreeKey::Select(parent.id.clone()));
                }
            }
            Some(TreeKey::Handled)
        }
        Key::Right => {
            if let Some(row) = row.filter(|r| r.file().is_none() && !r.expanded) {
                collapsed.remove(&row.id);
            }
            Some(TreeKey::Handled)
        }
        Key::Enter => match row {
            Some(r) if r.file().is_none() => {
                if !collapsed.remove(&r.id) {
                    collapsed.insert(r.id.clone());
                }
                Some(TreeKey::Handled)
            }
            _ => Some(TreeKey::Open),
        },
        _ => None,
    }
}

enum TreeKey {
    Handled,
    Select(String),
    Open,
}

fn toggle_collapsed(collapsed: &mut HashSet<String>, row: &ChangeRow) {
    if row.file().is_some() {
        return;
    }
    if !collapsed.remove(&row.id) {
        collapsed.insert(row.id.clone());
    }
}

// MARK: - Local changes

fn changes_list(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, id: ProjectId, rect: Rect) {
    let t = ui.theme;
    let mut area = rect;
    let Some(p) = app.project(id) else { return };
    let has_changes = !p.changes().is_empty();
    let show_commit = has_changes || p.amend;
    let column_h = rect.h;

    // The commit box goes at the bottom; the list takes what is left.
    if show_commit {
        let h = commit_panel::height(app, ui, id, column_h);
        let bottom = area.take_bottom(h.min(area.h));
        commit_panel::commit_panel(app, vs, ui, platform, id, bottom, column_h);
    }
    let Some(p) = app.project(id) else { return };
    if let Some(operation) = p.status.operation {
        let h = t.s(52.0);
        let banner = area.take_top(h);
        operation_banner(app, ui, id, operation, banner.inset_by(t.s(8.0), t.s(8.0), t.s(8.0), 0.0));
    }
    let Some(p) = app.project(id) else { return };
    if p.changes().len() > 8 || !p.filter.text.is_empty() {
        let row = area.take_top(t.s(36.0));
        let field = Rect::new(row.x + t.s(8.0), row.y + t.s(8.0), row.w - t.s(16.0), t.s(26.0));
        if let Some(p) = app.project_mut(id) {
            let outcome = components::filter_field(ui, platform, Id::new("changes-filter").with(id), field, &mut p.filter, &l("Filter files", "筛选文件"));
            if matches!(outcome, FieldOutcome::Submit | FieldOutcome::Cancel) {
                if outcome == FieldOutcome::Cancel {
                    p.filter.set_text("");
                }
                app.focus_request = Some(ids::list());
            }
        }
    }
    let Some(p) = app.project(id) else { return };
    if let (Some(message), false) = (&p.error_message, has_changes) {
        components::empty_state(ui, area, Icon::Warning, &l("Could not read repository", "无法读取仓库"), message);
        return;
    }
    if !has_changes {
        if p.has_loaded {
            components::empty_state(
                ui,
                area,
                Icon::CheckmarkCircle,
                &l("Working tree clean", "工作区很干净"),
                &l("Nothing to compare. Press Alt+9 to browse history.", "没有改动。按 Alt+9 查看提交历史。"),
            );
        }
        return;
    }
    let rows = change_tree::rows(p.changes(), app.settings.flat_list, &p.collapsed, &p.filter.text, true);
    if rows.is_empty() {
        components::empty_state(ui, area, Icon::Filter, &l("No files match", "没有匹配的文件"), "");
        return;
    }
    // Each row's tick: a file's own, or for a folder or group whether all, some or none of the files under it go in.
    let visible = app.visible_changes(id);
    let ticks: Vec<Inclusion> = rows
        .iter()
        .map(|row| match row.file() {
            Some(file) => p.inclusion(file),
            None => {
                let files = change_tree::files_for(&HashSet::from([row.id.clone()]), &visible);
                let states: Vec<Inclusion> = files.iter().map(|f| p.inclusion(f)).collect();
                if states.iter().all(|s| *s == Inclusion::Off) {
                    Inclusion::Off
                } else if states.iter().all(|s| *s == Inclusion::On) {
                    Inclusion::On
                } else {
                    Inclusion::Mixed
                }
            }
        })
        .collect();
    let row_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let selected = p.selected_change_id.clone();
    let multi = p.selected_change_ids.clone();
    let flat = app.settings.flat_list;
    let mut clicks = RowClicks::default();
    let row_h = t.s(24.0);
    let list_id = ids::list();
    let view = ListView::new(list_id, area, row_h, &row_ids, selected.as_deref()).multi(&multi).draggable();
    let result = components::list(ui, vs.list(list_id.with(("changes", id))), view, |ui, i, r, _, _| {
        draw_change_row(ui, list_id, &rows[i], i, r, flat, Some(ticks[i]), &mut clicks);
    });
    hover_tip(ui, vs, area, row_h, &rows, list_id.with(("changes", id)));

    let mut selection_changed = false;
    if let Some(p) = app.project_mut(id) {
        if let Some(set) = result.multi.clone() {
            p.selected_change_ids = set;
        }
        if let Some(sel) = result.select.clone() {
            selection_changed = p.selected_change_id != sel;
            p.selected_change_id = sel;
        }
        if let Some(i) = clicks.chevron {
            toggle_collapsed(&mut p.collapsed, &rows[i]);
        }
    }
    if let Some(i) = clicks.tick {
        // A partly ticked file or folder becomes wholly ticked, as a mixed checkbox does.
        let files = change_tree::files_for(&HashSet::from([rows[i].id.clone()]), &app.visible_changes(id));
        if let Some(p) = app.project_mut(id) {
            if !files.is_empty() {
                let all_on = files.iter().all(|f| p.inclusion(f) == Inclusion::On);
                p.set_included(!all_on, &files);
            }
        }
    }
    if selection_changed {
        app.change_selection_changed(id);
    }
    if let Some(i) = result.drag {
        app.drag_changes(&rows[i].id, id);
    }
    if let Some(i) = result.double_clicked {
        if rows[i].file().is_some() {
            app.focus_viewer();
        } else if let Some(p) = app.project_mut(id) {
            toggle_collapsed(&mut p.collapsed, &rows[i]);
        }
    }
    if let Some((i, at)) = result.menu {
        let items = change_menu(app, id, &rows[i]);
        app.open_menu(at, items);
    }
    if let Some((key, mods, index)) = result.key {
        if !mods.none() {
            return;
        }
        let row = index.map(|i| &rows[i]);
        let handled = match key {
            Key::Escape => {
                let had = app.project(id).is_some_and(|p| !p.filter.text.is_empty());
                if had {
                    if let Some(p) = app.project_mut(id) {
                        p.filter.set_text("");
                    }
                }
                had
            }
            // Backspace and Delete: new files go to the Recycle Bin.
            Key::Backspace => {
                app.delete_in_context();
                true
            }
            // Space ticks or unticks the selection for the commit, as in the IDE.
            Key::Space => {
                app.toggle_included_selection(id);
                true
            }
            _ => {
                let Some(p) = app.project_mut(id) else { return };
                match tree_key(key, row, &rows, &mut p.collapsed) {
                    Some(TreeKey::Select(s)) => {
                        p.selected_change_id = Some(s);
                        app.change_selection_changed(id);
                        true
                    }
                    Some(TreeKey::Open) => {
                        app.focus_viewer();
                        true
                    }
                    Some(TreeKey::Handled) => true,
                    None => false,
                }
            }
        };
        if handled {
            ui.consume();
        }
    }
}

/// A tooltip for the row under the pointer (the file's path, or where it was renamed from).
fn hover_tip(ui: &mut Ui, vs: &mut ViewState, area: Rect, row_h: f32, rows: &[ChangeRow], memory: Id) {
    let m = ui.mouse();
    if !area.contains(m) {
        return;
    }
    let t = ui.theme;
    let offset = vs.list(memory).scroll.offset;
    let i = ((m.y - area.y + offset - t.s(components::LIST_PAD)) / row_h).floor();
    if i < 0.0 {
        return;
    }
    let Some(row) = rows.get(i as usize) else { return };
    let tip = row_tip(row);
    if tip.is_empty() {
        return;
    }
    let y = area.y + t.s(components::LIST_PAD) + i * row_h - offset;
    let r = crate::ui::Response { id: ids::list().with(("row", &row.id)), rect: Rect::new(area.x, y, area.w, row_h), hovered: ui.is_hovered(ids::list().with(("row", &row.id))), ..Default::default() };
    ui.tooltip(&r, &tip);
}

/// "Rollback…" and "Delete…" for whatever the menu applies to: the selection if the row is in it, else the row.
fn add_edit_items(app: &App, id: ProjectId, row: &ChangeRow, items: &mut Vec<MenuItem>) {
    let targets = app.edit_targets(&row.id, id);
    let rollback: Vec<FileChange> = targets.iter().filter(|c| can_rollback(c)).cloned().collect();
    let delete: Vec<FileChange> = targets.iter().filter(|c| can_delete(c)).cloned().collect();
    if !rollback.is_empty() {
        let n = rollback.len();
        let title = if n == 1 { l("Rollback…", "回滚…") } else { lf!("Rollback {n} Files…", "回滚 {n} 个文件…") };
        items.push(MenuItem::new(title, move |app| app.rollback(rollback, id, true)).shortcut("Ctrl+Alt+Z"));
    }
    if !delete.is_empty() {
        let n = delete.len();
        let title = if n == 1 { l("Delete…", "删除…") } else { lf!("Delete {n} Files…", "删除 {n} 个文件…") };
        items.push(MenuItem::new(title, move |app| app.delete(delete, id, true)).shortcut("Delete"));
    }
    if !items.is_empty() {
        items.push(MenuItem::separator());
    }
}

fn reveal_items(app: &App, id: ProjectId, path: &str, items: &mut Vec<MenuItem>) {
    let Some(p) = app.project(id) else { return };
    let full = p.url.join(path.replace('/', "\\"));
    items.push(MenuItem::new(l("Reveal in File Explorer", "在资源管理器中显示"), move |app| app.requests.push(PlatformRequest::Reveal(full))));
    let path = path.to_string();
    items.push(MenuItem::new(l("Copy Path", "复制路径"), move |app| app.copy_text(path)));
}

fn change_menu(app: &mut App, id: ProjectId, row: &ChangeRow) -> Vec<MenuItem> {
    let mut items = Vec::new();
    let row_id = row.id.clone();
    if let Some(file) = row.file().filter(|f| f.kind == ChangeKind::Conflicted) {
        let r = row_id.clone();
        items.push(MenuItem::new(l("Resolve…", "解决冲突…"), move |app| {
            if let Some(p) = app.project_mut(id) {
                p.selected_change_id = Some(r);
            }
            app.change_selection_changed(id);
            app.focus_viewer();
        }));
        let path = file.path.clone();
        items.push(MenuItem::new(l("Mark as Resolved", "标记为已解决"), move |app| app.mark_resolved(path, id)));
        items.push(MenuItem::separator());
        let path = file.path.clone();
        items.push(MenuItem::new(l("Keep HEAD’s Version", "保留 HEAD 的版本"), move |app| app.take_side(crate::core::remote::ConflictSide::Ours, path, id)));
        let path = file.path.clone();
        items.push(MenuItem::new(l("Take Incoming Version", "采用传入的版本"), move |app| app.take_side(crate::core::remote::ConflictSide::Theirs, path, id)));
        items.push(MenuItem::separator());
        reveal_items(app, id, &file.path, &mut items);
        return items;
    }
    if let Some(file) = row.file() {
        let r = row_id.clone();
        items.push(
            MenuItem::new(l("Show Diff", "查看差异"), move |app| {
                if let Some(p) = app.project_mut(id) {
                    p.selected_change_id = Some(r);
                }
                app.change_selection_changed(id);
                app.focus_viewer();
            })
            .shortcut("Ctrl+D"),
        );
        if !matches!(file.kind, ChangeKind::Untracked | ChangeKind::Added) {
            let path = file.path.clone();
            items.push(MenuItem::new(l("Show History", "查看历史"), move |app| app.show_history_of_path(&path, id)));
        }
        items.push(MenuItem::separator());
        let mut edits = Vec::new();
        add_edit_items(app, id, row, &mut edits);
        items.extend(edits);
        reveal_items(app, id, &file.path, &mut items);
    } else {
        add_edit_items(app, id, row, &mut items);
        let r = row.clone();
        items.push(MenuItem::new(if row.expanded { l("Collapse", "折叠") } else { l("Expand", "展开") }, move |app| {
            if let Some(p) = app.project_mut(id) {
                toggle_collapsed(&mut p.collapsed, &r);
            }
        }));
        items.push(MenuItem::new(l("Expand All", "全部展开"), move |app| {
            if let Some(p) = app.project_mut(id) {
                p.collapsed.clear();
            }
        }));
    }
    items
}

// MARK: - Banners

/// A small button inside a banner.
#[allow(clippy::too_many_arguments)]
fn banner_button(ui: &mut Ui, id: Id, x: f32, cy: f32, title: &str, prominent: bool, enabled: bool, tip: &str) -> (bool, f32) {
    let t = ui.theme;
    let font = t.font_w(11.0, Weight::Semibold);
    let w = ui.text_width(title, font).ceil() + t.s(18.0);
    let h = t.s(22.0);
    let rect = Rect::new(x, cy - h / 2.0, w, h);
    let r = ui.interact(id, rect);
    let fill = if prominent {
        t.accent.alpha(if r.hovered && enabled { 0.85 } else { 1.0 })
    } else {
        t.text.alpha(if r.hovered && enabled { 0.16 } else { 0.09 })
    };
    let alpha = if enabled { 1.0 } else { 0.45 };
    ui.round(rect, t.s(6.0), fill.alpha(alpha));
    ui.label_with(rect, title, font, (if prominent { Color::WHITE } else { t.text }).alpha(alpha), Align::Center, Overflow::Clip);
    ui.tooltip(&r, tip);
    (r.clicked && enabled, w)
}

fn banner_width(ui: &mut Ui, title: &str) -> f32 {
    let t = ui.theme;
    ui.text_width(title, t.font_w(11.0, Weight::Semibold)).ceil() + t.s(18.0)
}

/// Over the Changes list while git is in the middle of a merge or rebase: how many files are still in conflict, and
/// the way out either side (Continue, or Abort back to the start).
fn operation_banner(app: &mut App, ui: &mut Ui, id: ProjectId, operation: RepoOperation, rect: Rect) {
    let t = ui.theme;
    let Some(p) = app.project(id) else { return };
    let conflicts = p.status.conflict_count();
    let busy = p.activity.is_some();
    let tint = if conflicts > 0 { Color::hex(0xF0A030) } else { t.added };
    ui.round(rect, t.s(8.0), tint.alpha(0.13));
    let icon = t.s(13.0);
    let x = rect.x + t.s(10.0);
    ui.icon(if conflicts > 0 { Icon::Warning } else { Icon::CheckmarkCircle }, Rect::new(x, rect.center().y - icon / 2.0, icon, icon), tint);
    let detail = if conflicts > 0 {
        if conflicts == 1 {
            l("1 file in conflict · open it to pick a side", "1 个文件有冲突 · 打开它选择保留哪一边")
        } else {
            lf!("{conflicts} files in conflict · open each to pick a side", "{conflicts} 个文件有冲突 · 逐个打开选择保留哪一边")
        }
    } else if operation == RepoOperation::Merge {
        l("All resolved · commit to finish the merge", "冲突已全部解决 · 提交即可完成合并")
    } else {
        l("All resolved · continue", "冲突已全部解决 · 可以继续")
    };
    // Buttons from the right.
    let cy = rect.center().y;
    let mut right = rect.right() - t.s(10.0);
    let abort = l("Abort", "中止");
    let w = banner_width(ui, &abort);
    right -= w;
    if banner_button(ui, Id::new("op-abort"), right, cy, &abort, false, !busy, "").0 {
        app.abort_operation_in(id);
        return;
    }
    if operation != RepoOperation::Merge {
        let skip = l("Skip", "跳过");
        let w = banner_width(ui, &skip);
        right -= w + t.s(6.0);
        if banner_button(ui, Id::new("op-skip"), right, cy, &skip, false, !busy, &l("Leave out the commit it stopped at, and go on", "略过停下的这个提交，继续后面的")).0 {
            app.skip_operation_in(id, false);
            return;
        }
        let cont = l("Continue", "继续");
        let w = banner_width(ui, &cont);
        right -= w + t.s(6.0);
        if banner_button(ui, Id::new("op-continue"), right, cy, &cont, true, conflicts == 0 && !busy, "").0 {
            app.continue_operation_in(id);
            return;
        }
    }
    let tx = x + icon + t.s(8.0);
    let tw = (right - t.s(6.0) - tx).max(0.0);
    ui.label(Rect::new(tx, rect.y + t.s(6.0), tw, t.s(16.0)), &operation.title(), t.font_w(11.5, Weight::Semibold), t.text);
    ui.label(Rect::new(tx, rect.y + t.s(23.0), tw, t.s(15.0)), &detail, t.font(10.5), t.text2);
}

/// A filter the list is under, with ✕ to drop it: the branch the Log lists, or the file whose history it lists.
/// Returns true when ✕ was clicked.
fn filter_chip(ui: &mut Ui, id: Id, rect: Rect, icon: Icon, text: &str, tip: &str, clear_tip: &str) -> bool {
    let t = ui.theme;
    let font = t.font_w(11.5, Weight::Medium);
    let h = t.s(22.0);
    let x = rect.x + t.s(10.0);
    let max_w = rect.w - t.s(20.0);
    let text_w = ui.text_width(text, font).ceil().min(max_w - t.s(48.0)).max(t.s(20.0));
    let w = (t.s(8.0) + t.s(11.0) + t.s(5.0) + text_w + t.s(5.0) + t.s(14.0) + t.s(6.0)).min(max_w);
    let chip = Rect::new(x, rect.center().y - h / 2.0, w, h);
    let r = ui.interact(id, chip);
    ui.round(chip, h / 2.0, t.selection);
    let mut cx = chip.x + t.s(8.0);
    let s = t.s(11.0);
    ui.icon(icon, Rect::new(cx, chip.center().y - s / 2.0, s, s), t.accent);
    cx += s + t.s(5.0);
    ui.label_with(Rect::new(cx, chip.y, text_w, h), text, font, t.accent, Align::Left, Overflow::EllipsisMiddle);
    cx += text_w + t.s(3.0);
    let close = Rect::new(cx, chip.center().y - t.s(8.0), t.s(16.0), t.s(16.0));
    let c = ui.interact(id.with("clear"), close);
    if c.hovered {
        ui.round(close, t.s(8.0), t.accent.alpha(0.15));
    }
    ui.icon(Icon::Dismiss, close.inset(t.s(3.5)), t.accent);
    ui.tooltip(&c, clear_tip);
    if !c.hovered {
        ui.tooltip(&r, tip);
    }
    c.clicked
}

fn short(hash: &str) -> String {
    hash.chars().take(7).collect()
}

/// What the Changes tab compares with, over its list.
fn comparison_banner(app: &mut App, ui: &mut Ui, comparison: &Comparison, rect: Rect) {
    let t = ui.theme;
    ui.round(rect, t.s(8.0), t.selection);
    let r = ui.interact(Id::new("compare-banner"), rect);
    let icon = t.s(12.0);
    let x = rect.x + t.s(10.0);
    ui.icon(Icon::ArrowSwap, Rect::new(x, rect.y + t.s(8.0), icon, icon), t.accent);
    let title = &comparison.title;
    let headline = match &comparison.other {
        Some(other) => {
            let o = &other.title;
            lf!("Comparing {title} with {o}", "比较 {title} 与 {o}")
        }
        None if comparison.fork_point => lf!("Since forking from {title}", "自从与 {title} 分叉以来"),
        None => lf!("Compared with {title}", "与 {title} 比较"),
    };
    let base = short(&comparison.base);
    let detail = match &comparison.other {
        Some(other) => {
            let o = short(&other.hash);
            lf!("{base} → {o}, what the later commits changed", "{base} → {o}，其间提交的全部改动")
        }
        None => lf!("{base} → working tree, commits included", "{base} → 工作区，含其间的提交"),
    };
    let close = Rect::new(rect.right() - t.s(26.0), rect.y + t.s(5.0), t.s(18.0), t.s(18.0));
    let tx = x + icon + t.s(7.0);
    let tw = (close.x - t.s(4.0) - tx).max(0.0);
    ui.label_with(Rect::new(tx, rect.y + t.s(5.0), tw, t.s(17.0)), &headline, t.font_w(11.5, Weight::Semibold), t.accent, Align::Left, Overflow::EllipsisMiddle);
    ui.label(Rect::new(tx, rect.y + t.s(22.0), tw, t.s(15.0)), &detail, t.font(10.5), t.text3);
    let c = ui.interact(Id::new("compare-close"), close);
    if c.hovered {
        ui.round(close, t.s(4.0), t.hover);
    }
    ui.icon(Icon::Dismiss, close.inset(t.s(4.0)), t.text2);
    ui.tooltip(&c, &l("Stop comparing · back to the local changes", "退出比较 · 回到本地变更"));
    if !c.hovered {
        let tip = match &comparison.other {
            Some(o) => format!("{}\n{}", comparison.base, o.hash),
            None => comparison.base.clone(),
        };
        ui.tooltip(&r, &tip);
    }
    if c.clicked {
        app.end_comparison();
    }
}

/// Over the list while it shows what a stash holds: the stash, and what to do with it.
fn stash_banner(app: &mut App, ui: &mut Ui, id: ProjectId, entry: &StashEntry, rect: Rect) {
    let t = ui.theme;
    let busy = app.project(id).is_some_and(|p| p.activity.is_some());
    ui.round(rect, t.s(8.0), t.selection);
    let r = ui.interact(Id::new("stash-banner"), rect);
    let icon = t.s(12.0);
    let x = rect.x + t.s(10.0);
    ui.icon(Icon::Archive, Rect::new(x, rect.y + t.s(8.0), icon, icon), t.accent);
    let close = Rect::new(rect.right() - t.s(26.0), rect.y + t.s(5.0), t.s(18.0), t.s(18.0));
    let tx = x + icon + t.s(7.0);
    let tw = (close.x - t.s(4.0) - tx).max(0.0);
    ui.label(Rect::new(tx, rect.y + t.s(5.0), tw, t.s(17.0)), entry.title(), t.font_w(11.5, Weight::Semibold), t.accent);
    let mut parts = vec![entry.name()];
    if let Some(b) = entry.branch() {
        parts.push(lf!("on {b}", "在 {b} 上"));
    }
    parts.push(relative_text(entry.date));
    ui.label(Rect::new(tx, rect.y + t.s(22.0), tw, t.s(15.0)), &parts.join(" · "), t.font(10.5), t.text3);
    let c = ui.interact(Id::new("stash-close"), close);
    if c.hovered {
        ui.round(close, t.s(4.0), t.hover);
    }
    ui.icon(Icon::Dismiss, close.inset(t.s(4.0)), t.text2);
    ui.tooltip(&c, &l("Close · back to the local changes", "关闭 · 回到本地变更"));
    if !c.hovered {
        ui.tooltip(&r, &format!("{} {}", entry.name(), entry.hash));
    }
    if c.clicked {
        app.end_comparison_in(id);
        return;
    }
    let cy = rect.bottom() - t.s(18.0);
    let mut bx = tx;
    let (apply, w) = banner_button(ui, Id::new("stash-apply"), bx, cy, &l("Apply", "应用"), true, !busy, &l("Bring its changes back; it stays in the list", "恢复它的改动；它仍留在列表里"));
    bx += w + t.s(6.0);
    let (pop, _) = banner_button(ui, Id::new("stash-pop"), bx, cy, &l("Pop", "弹出"), false, !busy, &l("Bring its changes back, then drop it", "恢复它的改动，然后从列表删除"));
    let drop_title = l("Drop…", "删除…");
    let dw = banner_width(ui, &drop_title);
    let (drop, _) = banner_button(ui, Id::new("stash-drop"), rect.right() - t.s(10.0) - dw, cy, &drop_title, false, !busy, &l("Delete it from the list without applying it", "不应用，直接从列表删除"));
    if apply {
        app.apply_stash(entry.clone(), false, id);
    } else if pop {
        app.apply_stash(entry.clone(), true, id);
    } else if drop {
        app.drop_stash(entry.clone(), id);
    }
}

// MARK: - Comparison

/// The Changes tab while it compares the working tree with another commit or branch: every file that differs from
/// it, committed since or not. Nothing here rolls back or commits; ✕ goes back to the local changes.
fn comparison_list(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, id: ProjectId, rect: Rect) {
    let t = ui.theme;
    let mut area = rect;
    let Some(p) = app.project(id) else { return };
    let Some(comparison) = p.comparison.clone() else { return };
    if let Some(stash) = &comparison.stash {
        let banner = area.take_top(t.s(84.0));
        stash_banner(app, ui, id, stash, banner.inset_by(t.s(8.0), t.s(8.0), t.s(8.0), 0.0));
    } else {
        let banner = area.take_top(t.s(52.0));
        comparison_banner(app, ui, &comparison, banner.inset_by(t.s(8.0), t.s(8.0), t.s(8.0), 0.0));
    }
    let Some(p) = app.project(id) else { return };
    if p.compare_changes.len() > 8 || !p.filter.text.is_empty() {
        let row = area.take_top(t.s(36.0));
        let field = Rect::new(row.x + t.s(8.0), row.y + t.s(8.0), row.w - t.s(16.0), t.s(26.0));
        if let Some(p) = app.project_mut(id) {
            let outcome = components::filter_field(ui, platform, Id::new("compare-filter").with(id), field, &mut p.filter, &l("Filter files", "筛选文件"));
            if matches!(outcome, FieldOutcome::Submit | FieldOutcome::Cancel) {
                if outcome == FieldOutcome::Cancel {
                    p.filter.set_text("");
                }
                app.focus_request = Some(ids::list());
            }
        }
    }
    let Some(p) = app.project(id) else { return };
    if p.compare_changes.is_empty() {
        if p.is_loading_compare {
            components::spinner(ui, area.center(), t.s(8.0), t.text3);
        } else if let Some(error) = &p.compare_error {
            components::empty_state(ui, area, Icon::Warning, error, "");
        } else {
            let title = &comparison.title;
            let detail = match &comparison.other {
                Some(other) => {
                    let o = &other.title;
                    lf!("{title} and {o} have the same files.", "{title} 与 {o} 的文件完全相同。")
                }
                None => lf!("The working tree is the same as {title}.", "工作区与 {title} 完全相同。"),
            };
            components::empty_state(ui, area, Icon::CheckmarkCircle, &l("No differences", "没有差异"), &detail);
        }
        return;
    }
    let rows = change_tree::rows(&p.compare_changes, app.settings.flat_list, &p.compare_collapsed, &p.filter.text, true);
    if rows.is_empty() {
        components::empty_state(ui, area, Icon::Filter, &l("No files match", "没有匹配的文件"), "");
        return;
    }
    let row_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let selected = p.compare_selected_id.clone();
    let flat = app.settings.flat_list;
    let mut clicks = RowClicks::default();
    let row_h = t.s(24.0);
    let list_id = ids::list();
    let memory = list_id.with(("compare", id));
    // Only a comparison with the working tree has its files on disk to drag out.
    let on_disk = p.comparison.as_ref().is_none_or(|c| c.head().is_none());
    let mut view = ListView::new(list_id, area, row_h, &row_ids, selected.as_deref());
    if on_disk {
        view = view.draggable();
    }
    let result = components::list(ui, vs.list(memory), view, |ui, i, r, _, _| {
        draw_change_row(ui, list_id, &rows[i], i, r, flat, None, &mut clicks);
    });
    hover_tip(ui, vs, area, row_h, &rows, memory);
    let mut changed = false;
    if let Some(p) = app.project_mut(id) {
        if let Some(sel) = result.select.clone() {
            changed = p.compare_selected_id != sel;
            p.compare_selected_id = sel;
        }
        if let Some(i) = clicks.chevron {
            toggle_collapsed(&mut p.compare_collapsed, &rows[i]);
        }
    }
    if changed {
        app.compare_selection_changed(id);
    }
    if let Some(file) = result.drag.and_then(|i| rows[i].file()) {
        if file.kind != crate::core::git::ChangeKind::Deleted {
            app.drag_out(id, vec![file.path.clone()]);
        }
    }
    if let Some(i) = result.double_clicked {
        if rows[i].file().is_some() {
            app.focus_viewer();
        } else if let Some(p) = app.project_mut(id) {
            toggle_collapsed(&mut p.compare_collapsed, &rows[i]);
        }
    }
    if let Some((i, at)) = result.menu {
        let items = compare_menu(app, id, &rows[i]);
        app.open_menu(at, items);
    }
    if let Some((key, mods, index)) = result.key {
        if !mods.none() {
            return;
        }
        let handled = if key == Key::Escape {
            let had = app.project(id).is_some_and(|p| !p.filter.text.is_empty());
            if let Some(p) = app.project_mut(id).filter(|_| had) {
                p.filter.set_text("");
            }
            had
        } else {
            let Some(p) = app.project_mut(id) else { return };
            match tree_key(key, index.map(|i| &rows[i]), &rows, &mut p.compare_collapsed) {
                Some(TreeKey::Select(s)) => {
                    p.compare_selected_id = Some(s);
                    app.compare_selection_changed(id);
                    true
                }
                Some(TreeKey::Open) => {
                    app.focus_viewer();
                    true
                }
                Some(TreeKey::Handled) => true,
                None => false,
            }
        };
        if handled {
            ui.consume();
        }
    }
}

fn compare_menu(app: &mut App, id: ProjectId, row: &ChangeRow) -> Vec<MenuItem> {
    let mut items = Vec::new();
    let Some(file) = row.file() else {
        let r = row.clone();
        items.push(MenuItem::new(if row.expanded { l("Collapse", "折叠") } else { l("Expand", "展开") }, move |app| {
            if let Some(p) = app.project_mut(id) {
                toggle_collapsed(&mut p.compare_collapsed, &r);
            }
        }));
        items.push(MenuItem::new(l("Expand All", "全部展开"), move |app| {
            if let Some(p) = app.project_mut(id) {
                p.compare_collapsed.clear();
            }
        }));
        return items;
    };
    let r = row.id.clone();
    items.push(MenuItem::new(l("Show Diff", "查看差异"), move |app| {
        if let Some(p) = app.project_mut(id) {
            p.compare_selected_id = Some(r);
        }
        app.compare_selection_changed(id);
        app.focus_viewer();
    }));
    if file.kind != ChangeKind::Untracked {
        let path = file.path.clone();
        items.push(MenuItem::new(l("Show History", "查看历史"), move |app| app.show_history_of_path(&path, id)));
    }
    items.push(MenuItem::separator());
    reveal_items(app, id, &file.path, &mut items);
    items
}

// MARK: - Log

fn log_panel(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, id: ProjectId, rect: Rect) {
    let t = ui.theme;
    let mut area = rect;
    if vs.appeared(Id::new("log").with(id)) {
        // A load already running (one asked for by the Git graph) is not replaced by a plain one.
        let empty_files = app.project(id).is_some_and(|p| !p.commits.is_empty() && p.commit_files.is_empty());
        if let Some(p) = app.project_mut(id) {
            if p.commits.is_empty() && !p.is_loading_log {
                p.load_log(true, 0, None);
            }
        }
        if empty_files {
            app.commit_selection_changed(id);
        }
    }

    // The search field and the all-branches toggle.
    let top = area.take_top(t.s(36.0));
    let b = Rect::new(top.right() - t.s(6.0) - t.s(26.0), top.y + t.s(9.0), t.s(26.0), t.s(24.0));
    let field = Rect::new(top.x + t.s(8.0), top.y + t.s(8.0), b.x - t.s(4.0) - top.x - t.s(8.0), t.s(26.0));
    let all = app.project(id).is_some_and(|p| p.log_all_branches);
    if icon_button(ui, Id::new("log-all"), b, Icon::Branch, &l("Show all branches", "显示所有分支"), true, all).clicked {
        if let Some(p) = app.project_mut(id) {
            p.log_all_branches = !p.log_all_branches;
            if p.log_all_branches {
                p.log_ref = None;
            }
            p.load_log(true, 0, None);
        }
    }
    if let Some(p) = app.project_mut(id) {
        let outcome = components::filter_field(ui, platform, Id::new("log-query").with(id), field, &mut p.log_query, &l("Search message, author or hash", "搜索提交信息、作者或哈希"));
        match outcome {
            FieldOutcome::Changed => app.log_search_at = Some((id, Instant::now() + Duration::from_millis(250))),
            FieldOutcome::Submit => {
                app.log_search_at = Some((id, Instant::now()));
                app.focus_request = Some(ids::list());
            }
            FieldOutcome::Cancel => app.focus_request = Some(ids::list()),
            _ => {}
        }
    }

    let Some(p) = app.project(id) else { return };
    if let Some(history) = p.history.clone() {
        // The Log lists one file's commits (or its lines'); ✕ goes back to the branch.
        let chip = area.take_top(t.s(28.0));
        let label = history.label();
        let tip = if history.rev == "HEAD" { history.path.clone() } else { format!("{} @ {}", history.path, short(&history.rev)) };
        let icon = if history.lines.is_none() { Icon::History } else { Icon::TextExpand };
        if filter_chip(ui, Id::new("history-chip"), chip, icon, &lf!("History of {label}", "{label} 的历史"), &tip, &l("Back to the whole Log", "回到完整历史")) {
            app.end_history();
            return;
        }
    }
    let Some(p) = app.project(id) else { return };
    if let Some(label) = p.log_ref_label() {
        // The Log lists a branch picked in the Git graph; ✕ goes back to the current branch.
        let chip = area.take_top(t.s(28.0));
        if filter_chip(ui, Id::new("ref-chip"), chip, Icon::Branch, &label, &label, &l("Back to the current branch", "回到当前分支")) {
            if let Some(p) = app.project_mut(id) {
                p.log_ref = None;
                p.load_log(true, 0, None);
            }
            return;
        }
    }

    let Some(p) = app.project(id) else { return };
    if p.commits.is_empty() {
        if p.is_loading_log {
            components::spinner(ui, area.center(), t.s(8.0), t.text3);
        } else if let Some(history) = &p.history {
            let title = if history.lines.is_none() { l("No commits changed this file", "没有提交改动过这个文件") } else { l("No commits changed these lines", "没有提交改动过这些行") };
            let detail = if p.log_query.text.is_empty() { String::new() } else { l("Nothing matches the search.", "没有符合搜索条件的提交。") };
            components::empty_state(ui, area, Icon::History, &title, &detail);
        } else {
            components::empty_state(ui, area, Icon::History, &l("No commits", "没有提交"), "");
        }
        return;
    }

    // The commit list over the selected commit's details; the line between them is dragged.
    let key = Id::new("log-files-height");
    let files_h = vs.heights.get(&key).copied().unwrap_or(260.0).clamp(120.0, 520.0f32.min((area.h - 120.0).max(120.0)));
    let detail = area.take_bottom(t.s(files_h).min(area.h - t.s(60.0)).max(0.0));
    let line_y = detail.y;
    let commits = p.commits.clone();
    let row_ids: Vec<String> = commits.iter().map(|c| c.hash.clone()).collect();
    let selected = p.selected_commit_id.clone();
    let multi = p.selected_commit_ids.clone();
    let row_h = t.s(44.0);
    let list_id = ids::list();
    let view = ListView::new(list_id, area, row_h, &row_ids, selected.as_deref()).multi(&multi);
    let result = components::list(ui, vs.list(list_id.with(("log", id))), view, |ui, i, r, _, _| {
        commit_row(ui, &commits[i], r);
    });
    let mut changed = false;
    if let Some(p) = app.project_mut(id) {
        if let Some(set) = result.multi.clone() {
            p.selected_commit_ids = set;
        }
        if let Some(sel) = result.select.clone() {
            changed = p.selected_commit_id != sel;
            p.selected_commit_id = sel;
        }
        if result.at_end && p.has_more_commits && !p.is_loading_log {
            p.load_log(false, 0, None);
        }
    }
    if changed {
        app.picked_in_log(id, true);
    }
    if result.double_clicked.is_some() {
        app.focus_request = Some(ids::log_files());
    }
    if let Some((i, at)) = result.menu {
        let items = app.log_commit_menu(&commits[i], id);
        app.open_menu(at, items);
    }
    if let Some((key, mods, _)) = result.key {
        if mods.none() && matches!(key, Key::Right | Key::Enter) {
            ui.consume();
            app.focus_request = Some(ids::log_files());
        }
    }

    ui.hline(detail.x, line_y, detail.w, t.hairline);
    commit_detail(app, vs, ui, id, detail.inset_by(0.0, 1.0, 0.0, 0.0));
    let mut h = files_h;
    let grip = Rect::new(detail.x, line_y - 3.0, detail.w, 7.0);
    splitter(ui, Id::new("split-log-files"), grip, false, &mut h, 120.0, 520.0, -1.0 / t.scale.max(0.5));
    vs.heights.insert(key, h);
}

fn commit_row(ui: &mut Ui, commit: &Commit, rect: Rect) {
    let t = ui.theme;
    let x0 = rect.x + t.s(14.0);
    let right = rect.right() - t.s(14.0);
    let line1 = Rect::new(x0, rect.y + t.s(5.0), right - x0, t.s(18.0));
    let mut x = x0;
    for r in commit.refs.iter().take(3) {
        let w = ref_chip(ui, x, line1.center().y, r, right - x - t.s(40.0));
        if w <= 0.0 {
            break;
        }
        x += w + t.s(5.0);
    }
    ui.label(Rect::new(x, line1.y, (right - x).max(0.0), line1.h), &commit.subject, t.font(12.5), t.text);
    let line2 = Rect::new(x0, rect.y + t.s(23.0), right - x0, t.s(16.0));
    let hash = commit.short_hash();
    let hash_font = t.mono(10.5);
    let hw = ui.text_width(&hash, hash_font).ceil();
    ui.label_with(Rect::new(right - hw, line2.y, hw + 1.0, line2.h), &hash, hash_font, t.text3, Align::Left, Overflow::Clip);
    let mut end = right - hw - t.s(6.0);
    if commit.is_merge() {
        let s = t.s(11.0);
        ui.icon(Icon::Merge, Rect::new(end - s, line2.center().y - s / 2.0, s, s), t.text3);
        end -= s + t.s(6.0);
    }
    let text = format!("{}  ·  {}", commit.author, relative_text(commit.date));
    ui.label(Rect::new(x0, line2.y, (end - x0).max(0.0), line2.h), &text, t.font(11.0), t.text3);
}

/// A branch or tag on a commit, as `git log --decorate=full --format=%D` names them. Returns its width (0 when
/// there is no room).
pub fn ref_chip(ui: &mut Ui, x: f32, cy: f32, r: &str, room: f32) -> f32 {
    let t = ui.theme;
    let is_head = r.starts_with("HEAD");
    let is_tag = r.starts_with("tag: ");
    let is_remote = r.starts_with("refs/remotes/");
    let label = ref_label(r);
    // Remote branches in a quieter colour than local ones; the checked-out one in green.
    let tint = if is_tag {
        Color::hex(0xD9A520)
    } else if is_head {
        t.added
    } else if is_remote {
        Color::hex(0x9A7FE0)
    } else {
        t.accent
    };
    let font = t.font_w(10.0, Weight::Semibold);
    let w = (ui.text_width(&label, font).ceil() + t.s(10.0)).min(t.s(140.0));
    if room < t.s(30.0) {
        return 0.0;
    }
    let w = w.min(room);
    let h = t.s(16.0);
    let rect = Rect::new(x, cy - h / 2.0, w, h);
    ui.round(rect, t.s(4.0), tint.alpha(0.16));
    ui.label_with(rect.inset_xy(t.s(5.0), 0.0), &label, font, tint, Align::Left, Overflow::Ellipsis);
    w
}

pub fn ref_label(r: &str) -> String {
    let name = r.replace("HEAD -> ", "").replace("tag: ", "");
    for prefix in ["refs/heads/", "refs/remotes/", "refs/tags/"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    name
}

/// The selected commit's message and changed files.
/// A wrapped message cut to four lines, ending in "…" when there was more.
fn four_lines(ui: &mut Ui, message: &str, style: crate::paint::TextStyle) -> std::rc::Rc<dyn crate::paint::TextLayout> {
    const LINES: usize = 4;
    let full = ui.text_layout(message, style.clone());
    let lines = full.line_count().max(1);
    if lines <= LINES {
        return full;
    }
    let line_h = full.size().h / lines as f32;
    let width = style.max_width.unwrap_or(full.size().w);
    let mut end = full.hit_test(Point::new(width, line_h * (LINES as f32 - 0.5))).min(message.len());
    loop {
        let text = format!("{}…", message[..end].trim_end());
        let cut = ui.text_layout(&text, style.clone());
        if cut.line_count() <= LINES || end == 0 {
            return cut;
        }
        end = message[..end].char_indices().next_back().map_or(0, |(i, _)| i);
    }
}

fn commit_detail(app: &mut App, vs: &mut ViewState, ui: &mut Ui, id: ProjectId, rect: Rect) {
    let t = ui.theme;
    let mut area = rect;
    let Some(p) = app.project(id) else { return };
    if let Some(commit) = p.selected_commit().cloned() {
        let message = if p.commit_body.is_empty() { commit.subject.clone() } else { p.commit_body.clone() };
        let mut style = crate::paint::TextStyle::new(t.font(12.0));
        let max_w = (area.w - t.s(28.0)).max(10.0);
        style.max_width = Some(max_w);
        style.overflow = Overflow::Wrap;
        let layout = four_lines(ui, message.trim_end(), style);
        let text_h = layout.size().h;
        let h = t.s(8.0) + text_h + t.s(4.0) + t.s(16.0) + t.s(8.0);
        let head = area.take_top(h);
        ui.clipped(Rect::new(head.x, head.y, head.w, t.s(8.0) + text_h), |ui| {
            ui.draw_layout(&layout, Rect::new(head.x + t.s(14.0), head.y + t.s(8.0), layout.size().w, layout.size().h), t.text, Align::Left);
        });
        let info = Rect::new(head.x + t.s(14.0), head.y + t.s(8.0) + text_h + t.s(4.0), head.w - t.s(28.0), t.s(16.0));
        // The hash at the right copies the full hash.
        let hash = commit.short_hash();
        let hash_font = t.mono(10.5);
        let hw = ui.text_width(&hash, hash_font).ceil();
        let icon = t.s(10.0);
        let b = Rect::new(info.right() - hw - icon - t.s(3.0), info.y, hw + icon + t.s(3.0), info.h);
        let r = ui.interact(Id::new("copy-hash"), b.inset(-t.s(2.0)));
        let color = if r.hovered { t.text2 } else { t.text3 };
        ui.label_with(Rect::new(b.x, b.y, hw + 1.0, b.h), &hash, hash_font, color, Align::Left, Overflow::Clip);
        ui.icon(Icon::Copy, Rect::new(b.right() - icon, b.center().y - icon / 2.0, icon, icon), color);
        ui.tooltip(&r, &l("Copy full hash", "复制完整哈希"));
        if r.clicked {
            app.requests.push(PlatformRequest::Copy(commit.hash.clone()));
            app.show_toast(lf!("Copied {hash}", "已复制 {hash}"));
        }
        let text = format!("{}   {}", commit.author, date_time_text(commit.date));
        ui.label(Rect::new(info.x, info.y, (b.x - t.s(6.0) - info.x).max(0.0), info.h), &text, t.font(11.0), t.text3);
        ui.hline(area.x, area.y, area.w, t.hairline);
        area.take_top(1.0);
    }

    let Some(p) = app.project(id) else { return };
    let rows = change_tree::rows(&p.commit_files, app.settings.flat_list, &p.commit_collapsed, "", false);
    let row_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let selected = p.selected_commit_file_id.clone();
    let flat = app.settings.flat_list;
    let mut clicks = RowClicks::default();
    let row_h = t.s(24.0);
    let list_id = ids::log_files();
    let memory = list_id.with(("files", id));
    let view = ListView::new(list_id, area, row_h, &row_ids, selected.as_deref());
    let result = components::list(ui, vs.list(memory), view, |ui, i, r, _, _| {
        draw_change_row(ui, list_id, &rows[i], i, r, flat, None, &mut clicks);
    });
    let mut changed = false;
    if let Some(p) = app.project_mut(id) {
        if let Some(sel) = result.select.clone() {
            changed = p.selected_commit_file_id != sel;
            p.selected_commit_file_id = sel;
        }
        if let Some(i) = clicks.chevron {
            toggle_collapsed(&mut p.commit_collapsed, &rows[i]);
        }
    }
    if changed {
        app.picked_in_log(id, false);
    }
    if let Some(i) = result.double_clicked {
        if rows[i].file().is_some() {
            app.focus_viewer();
        } else if let Some(p) = app.project_mut(id) {
            toggle_collapsed(&mut p.commit_collapsed, &rows[i]);
        }
    }
    if let Some((i, at)) = result.menu {
        let commit = app.project(id).and_then(|p| p.selected_commit().cloned());
        if let (Some(file), Some(commit)) = (rows[i].file().cloned(), commit) {
            let items = app.commit_file_menu(&file, &commit, id);
            app.open_menu(at, items);
        }
    }
    if let Some((key, mods, index)) = result.key {
        if !mods.none() {
            return;
        }
        let row = index.map(|i| &rows[i]);
        let handled = match key {
            Key::Left => {
                match row {
                    Some(r) if r.file().is_none() && r.expanded => {
                        if let Some(p) = app.project_mut(id) {
                            p.commit_collapsed.insert(r.id.clone());
                        }
                    }
                    _ => app.focus_list(),
                }
                true
            }
            Key::Right => {
                if let Some(r) = row.filter(|r| r.file().is_none()) {
                    if let Some(p) = app.project_mut(id) {
                        p.commit_collapsed.remove(&r.id);
                    }
                }
                true
            }
            Key::Enter => {
                match row {
                    Some(r) if r.file().is_none() => {
                        if let Some(p) = app.project_mut(id) {
                            toggle_collapsed(&mut p.commit_collapsed, r);
                        }
                    }
                    _ => app.focus_viewer(),
                }
                true
            }
            Key::Escape => {
                app.focus_list();
                true
            }
            _ => false,
        };
        if handled {
            ui.consume();
        }
    }
}
