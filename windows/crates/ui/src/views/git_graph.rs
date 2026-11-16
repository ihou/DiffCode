//! The Git graph dropdown: the repository's branches and tags beside the commit graph of whichever is picked.
//! It hangs under the branch that opened it, over everything else; a click outside closes it.

use std::collections::HashMap;

use super::components::{self, ListView, ViewState};
use super::project_panel::ref_chip;
use crate::app::git_graph::{BranchRow, BranchRowKind};
use crate::app::{ids, App};
use crate::core::history::{GraphRow, RefKind, Span, GitRef};
use crate::core::lang::l;
use crate::core::lf;
use crate::core::util::{date_time_text, relative_text};
use crate::geom::{Color, Point, Rect};
use crate::icons::Icon;
use crate::input::Key;
use crate::paint::{Overflow, Placement, VectorPath, Weight};
use crate::platform::Platform;
use crate::text_field::FieldOutcome;
use crate::ui::{layer, Align, Id, Response, Ui};

const PALETTE: [u32; 8] = [0x5C92F7, 0x57B46F, 0xE5A04B, 0xB57BEE, 0xEF6F8E, 0x3DBFC6, 0xD9B43B, 0xE5736B];

fn lane_color(index: u32) -> Color {
    Color::hex(PALETTE[index as usize % PALETTE.len()])
}

fn filter_id() -> Id {
    Id::new("graph-branch-filter")
}

pub fn git_graph(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform) {
    let t = ui.theme;
    ui.with_layer(layer::POPUP, |ui| {
        let window = ui.bounds();
        ui.fill(window, Color::BLACK.alpha(0.06));
        if ui.block(Id::new("graph-back"), window).pressed {
            app.dismiss_popup(true);
            return;
        }
        let width = t.s(940.0).min(window.w - 16.0);
        let anchor = app.git_graph.anchor.unwrap_or(Point::new(12.0, 4.0));
        let top = anchor.y.min(window.h * 0.4);
        let height = t.s(580.0).min(window.h - top - 12.0);
        let left = anchor.x.max(8.0).min(window.w - width - 8.0);
        let rect = Rect::new(left, top, width, height).snapped(ui.painter.scale());
        components::popup_panel(ui, rect, t.s(12.0));
        ui.block(Id::new("graph-panel"), rect);

        // Escape closes from anywhere inside; the filter clears itself first.
        if ui.key().is_some_and(|(k, m)| k == Key::Escape && m.none()) && !(ui.has_focus(filter_id()) && !app.git_graph.branch_filter.text.is_empty()) {
            ui.consume();
            app.dismiss_popup(true);
            return;
        }

        let footer_h = t.s(34.0);
        let body = Rect::new(rect.x, rect.y, rect.w, rect.h - footer_h - 1.0);
        let branches_w = t.s(230.0);
        let branches = Rect::new(body.x, body.y, branches_w, body.h);
        let graph = Rect::new(branches.right() + 1.0, body.y, body.right() - branches.right() - 1.0, body.h);
        branch_list(app, vs, ui, platform, branches);
        ui.vline(branches.right(), body.y, body.h, t.hairline);
        if app.popup.is_none() {
            return;
        }
        commit_graph(app, vs, ui, graph);
        if app.popup.is_none() {
            return;
        }
        ui.hline(rect.x, body.bottom(), rect.w, t.hairline);
        footer(app, ui, Rect::new(rect.x, body.bottom() + 1.0, rect.w, footer_h));
    });
}

// MARK: - Branches

fn branch_list(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect) {
    let t = ui.theme;
    let filter_h = t.s(38.0);
    let field = Rect::new(rect.x + t.s(8.0), rect.y + t.s(8.0), rect.w - t.s(16.0), t.s(24.0));
    // Down from the filter goes into the list.
    if ui.has_focus(filter_id()) && ui.take_key(Key::Down, crate::input::Modifiers::NONE) {
        ui.focus(ids::graph_branches());
    }
    match components::filter_field(ui, platform, filter_id(), field, &mut app.git_graph.branch_filter, &l("Filter branches", "筛选分支")) {
        FieldOutcome::Submit => ui.focus(ids::graph_branches()),
        FieldOutcome::Cancel => app.git_graph.branch_filter.set_text(""),
        _ => {}
    }

    let area = Rect::new(rect.x, rect.y + filter_h, rect.w, rect.h - filter_h);
    let rows = app.git_graph.branch_rows();
    let row_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let list_id = ids::graph_branches();
    let focused = ui.has_focus(list_id);
    if components::speed_search(ui, focused, &mut app.git_graph.branch_filter) {
        ui.request_repaint();
    }
    let selectable = |i: usize| rows[i].is_selectable();
    let selected = app.git_graph.selected_branch_id.clone();
    let mem = vs.list(Id::new("graph-branches-memory"));
    mem.scroll = app.git_graph.branch_scroll;
    let view = ListView::new(list_id, area, t.s(26.0), &row_ids, selected.as_deref()).selectable(&selectable);
    let result = components::list(ui, mem, view, |ui, i, r, _, _| branch_row(ui, &rows[i], r));
    app.git_graph.branch_scroll = vs.list(Id::new("graph-branches-memory")).scroll;
    branch_tip(ui, vs, area, &rows);

    if let Some(Some(id)) = &result.select {
        if let Some(row) = rows.iter().find(|r| &r.id == id) {
            app.pick_branch_row(&row.clone());
        }
    }
    if let Some(i) = result.clicked {
        if !rows[i].is_selectable() {
            app.pick_branch_row(&rows[i].clone());
        }
    }
    if let Some((i, at)) = result.menu {
        let items = app.graph_branch_menu(&rows[i].clone());
        if !items.is_empty() {
            app.open_menu(at, items);
        }
    }
    if let Some((key, mods, _)) = result.key {
        if (key == Key::Tab || key == Key::Right) && mods.none() {
            ui.consume();
            ui.focus(ids::graph());
        }
    }
}

fn branch_row(ui: &mut Ui, row: &BranchRow, rect: Rect) {
    let t = ui.theme;
    let r = rect.inset_xy(t.s(12.0), 0.0);
    let cy = r.center().y;
    match &row.kind {
        BranchRowKind::Section { title, count, expanded, .. } => {
            let s = t.s(10.0);
            let icon = if *expanded { Icon::ChevronDown } else { Icon::ChevronRight };
            ui.icon(icon, Rect::new(r.x + t.s(1.0), cy - s / 2.0, s, s), t.text3);
            let font = t.font_w(10.5, Weight::Semibold);
            let upper = title.to_uppercase();
            let x = r.x + t.s(18.0);
            let w = ui.text_width(&upper, font).ceil();
            ui.label_with(Rect::new(x, r.y, w + 1.0, r.h), &upper, font, t.text3, Align::Left, Overflow::Clip);
            ui.label(Rect::new(x + w + t.s(6.0), r.y, t.s(40.0), r.h), &count.to_string(), t.font(10.5), t.text3);
        }
        BranchRowKind::Scope { title, icon, .. } => {
            let s = t.s(13.0);
            ui.icon(*icon, Rect::new(r.x + t.s(1.5), cy - s / 2.0, s, s), t.text2);
            let x = r.x + t.s(22.0);
            ui.label(Rect::new(x, r.y, r.right() - x, r.h), title, t.font_w(12.5, Weight::Medium), t.text);
        }
        BranchRowKind::Ref(gr) => {
            let s = t.s(12.0);
            let x0 = r.x + t.s(10.0);
            let (icon, color) = ref_icon(ui, gr);
            ui.icon(icon, Rect::new(x0 + t.s(2.0), cy - s / 2.0, s, s), color);
            let mut right = r.right();
            if gr.upstream_gone {
                let gs = t.s(11.0);
                // A cloud struck through: the upstream is gone.
                let g = Rect::new(right - gs, cy - gs / 2.0, gs, gs);
                ui.icon(Icon::Cloud, g, t.text3);
                ui.painter.line(Point::new(g.x + 1.0, g.y + 1.0), Point::new(g.right() - 1.0, g.bottom() - 1.0), 1.2, t.text3);
                right -= gs + t.s(6.0);
            }
            let small = t.font_w(10.5, Weight::Medium);
            for text in [(gr.behind > 0).then(|| format!("↓{}", gr.behind)), (gr.ahead > 0).then(|| format!("↑{}", gr.ahead))].into_iter().flatten() {
                let w = ui.text_width(&text, small).ceil();
                ui.label_with(Rect::new(right - w, r.y, w + 1.0, r.h), &text, small, t.text2, Align::Left, Overflow::Clip);
                right -= w + t.s(6.0);
            }
            let x = x0 + t.s(22.0);
            let font = t.font_w(12.5, if gr.is_head { Weight::Semibold } else { Weight::Regular });
            ui.label_with(Rect::new(x, r.y, (right - x - t.s(4.0)).max(0.0), r.h), &gr.short_name, font, t.text, Align::Left, Overflow::EllipsisMiddle);
        }
    }
}

fn ref_icon(ui: &Ui, r: &GitRef) -> (Icon, Color) {
    let t = ui.theme;
    if r.is_head {
        return (Icon::Checkmark, t.added);
    }
    let icon = match r.kind {
        RefKind::Local => Icon::Branch,
        RefKind::Remote => Icon::Cloud,
        RefKind::Tag => Icon::Tag,
    };
    (icon, t.text3)
}

fn branch_tip(ui: &mut Ui, vs: &mut ViewState, area: Rect, rows: &[BranchRow]) {
    let Some((i, rect)) = hovered_row(ui, vs, area, Id::new("graph-branches-memory"), rows.len(), ui.theme.s(26.0)) else { return };
    let BranchRowKind::Ref(r) = &rows[i].kind else { return };
    let mut lines = vec![r.short_name.clone()];
    if let Some(up) = &r.upstream {
        lines.push(if r.upstream_gone { lf!("Upstream {up} is gone", "上游 {up} 已不存在") } else { lf!("Tracking {up}", "跟踪 {up}") });
    }
    let id = ids::graph_branches().with(("row", &rows[i].id));
    let response = Response { id, rect, hovered: ui.is_hovered(id), ..Default::default() };
    ui.tooltip(&response, &lines.join("\n"));
}

/// The row under the pointer in a list drawn by `components::list`, and where it is.
fn hovered_row(ui: &mut Ui, vs: &mut ViewState, area: Rect, memory: Id, count: usize, row_h: f32) -> Option<(usize, Rect)> {
    let m = ui.mouse();
    if !area.contains(m) {
        return None;
    }
    let pad = ui.theme.s(components::LIST_PAD);
    let offset = vs.list(memory).scroll.offset;
    let i = ((m.y - area.y + offset - pad) / row_h).floor();
    if i < 0.0 || i as usize >= count {
        return None;
    }
    let y = area.y + pad + i * row_h - offset;
    Some((i as usize, Rect::new(area.x, y, area.w, row_h)))
}

// MARK: - Commit graph

fn commit_graph(app: &mut App, vs: &mut ViewState, ui: &mut Ui, area: Rect) {
    let t = ui.theme;
    let g = &app.git_graph;
    if g.rows.is_empty() {
        if g.is_loading {
            components::spinner(ui, area.center(), t.s(7.0), t.text3);
        } else {
            let title = if g.error.is_none() { l("No commits yet", "还没有提交") } else { l("Could not read the history", "无法读取提交历史") };
            components::empty_state(ui, area, Icon::Branch, &title, g.error.as_deref().unwrap_or(""));
        }
        return;
    }
    let list_id = ids::graph();
    let memory = Id::new("graph-commits-memory");
    if std::mem::take(&mut app.git_graph.reveal) {
        vs.reset_list(memory);
    }
    let g = &app.git_graph;
    let row_ids: Vec<String> = g.rows.iter().map(|r| r.commit.hash.clone()).collect();
    let selected = g.selected_commit_id.clone();
    let multi = g.selected_commit_ids.clone();
    let lanes = g.lane_count;
    let mem = vs.list(memory);
    if mem.scroll.offset == 0.0 {
        mem.scroll = app.git_graph.graph_scroll;
    }
    let row_h = t.s(26.0);
    let view = ListView::new(list_id, area, row_h, &row_ids, selected.as_deref()).multi(&multi);
    let rows = &app.git_graph.rows;
    let mut dates = HashMap::new();
    let result = components::list(ui, mem, view, |ui, i, r, _, _| {
        let date = dates.entry(rows[i].commit.date).or_insert_with(|| relative_text(rows[i].commit.date)).clone();
        graph_row(ui, &rows[i], lanes, r, &date);
    });
    let mem = vs.list(memory);
    app.git_graph.graph_scroll = mem.scroll;
    if let Some((i, rect)) = hovered_row(ui, vs, area, memory, rows.len(), row_h) {
        let c = &rows[i].commit;
        let id = list_id.with(("row", &c.hash));
        let response = Response { id, rect, hovered: ui.is_hovered(id), ..Default::default() };
        let tip = format!("{}\n{} · {} · {}", c.subject, c.author, date_time_text(c.date), c.short_hash());
        ui.tooltip(&response, &tip);
    }

    let g = &mut app.git_graph;
    if let Some(set) = result.multi.clone() {
        g.selected_commit_ids = set;
    }
    if let Some(sel) = result.select.clone() {
        if result.multi.is_none() {
            g.selected_commit_ids = sel.iter().cloned().collect();
        }
        g.selected_commit_id = sel;
    }
    if result.at_end && g.has_more && !g.is_loading {
        app.load_git_graph(false);
    }
    if let Some(i) = result.double_clicked {
        let hash = app.git_graph.rows[i].commit.hash.clone();
        app.show_commit_in_log(hash);
        return;
    }
    if let Some((i, at)) = result.menu {
        let items = app.graph_commit_menu(i);
        if !items.is_empty() {
            app.open_menu(at, items);
        }
    }
    if let Some((key, mods, row)) = result.key {
        if key == Key::Enter && mods.none() {
            ui.consume();
            if let Some(hash) = row.and_then(|i| app.git_graph.rows.get(i)).map(|r| r.commit.hash.clone()) {
                app.show_commit_in_log(hash);
            }
        } else if (key == Key::Tab || key == Key::Left) && mods.none() {
            ui.consume();
            ui.focus(ids::graph_branches());
        }
    }
}

fn lanes_width(ui: &Ui, lanes: usize) -> f32 {
    lanes as f32 * ui.theme.s(14.0) + 6.0
}

fn graph_row(ui: &mut Ui, row: &GraphRow, lanes: usize, rect: Rect, date: &str) {
    let t = ui.theme;
    let commit = &row.commit;
    let lanes_rect = Rect::new(rect.x + t.s(8.0), rect.y, lanes_width(ui, lanes), rect.h);
    let cy = rect.center().y;
    let mut right = rect.right() - t.s(14.0);

    // From the right: hash, date, author.
    let hash = commit.short_hash();
    let hw = t.s(58.0);
    ui.label_with(Rect::new(right - hw, rect.y, hw, rect.h), &hash, t.code_font().size(11.0 * t.scale), t.text3, Align::Right, Overflow::Clip);
    right -= hw + t.s(8.0);
    let dw = t.s(70.0);
    ui.label_with(Rect::new(right - dw, rect.y, dw, rect.h), date, t.font(11.5), t.text3, Align::Right, Overflow::Ellipsis);
    right -= dw + t.s(8.0);
    let aw = t.s(104.0);
    ui.label_with(Rect::new(right - aw, rect.y, aw, rect.h), &commit.author, t.font(11.5), t.text3, Align::Left, Overflow::Ellipsis);
    right -= aw + t.s(8.0);

    // From the left: the refs and the subject.
    let mut x = lanes_rect.right() + t.s(8.0);
    for r in commit.refs.iter().take(3) {
        if right - x < t.s(40.0) {
            break;
        }
        let w = ref_chip(ui, x, cy, r, right - x);
        x += w + t.s(8.0);
    }
    if commit.refs.len() > 3 {
        let more = format!("+{}", commit.refs.len() - 3);
        let f = t.font_w(10.0, Weight::Semibold);
        let w = ui.text_width(&more, f).ceil();
        ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), &more, f, t.text3, Align::Left, Overflow::Clip);
        x += w + t.s(8.0);
    }
    if right - x > t.s(8.0) {
        ui.label(Rect::new(x, rect.y, right - x, rect.h), &commit.subject, t.font(12.5), t.text);
    }

    ui.clipped(lanes_rect, |ui| draw_lanes(ui, row, lanes_rect.x, rect));
}

/// One row's slice of the graph. Rows are drawn independently; their lines meet at the row edges.
fn draw_lanes(ui: &mut Ui, row: &GraphRow, inset: f32, rect: Rect) {
    let t = ui.theme;
    let lane = t.s(14.0);
    let x = |i: u16| inset + 3.0 + lane * (i as f32 + 0.5);
    let top = rect.y;
    let bottom = rect.bottom();
    let mid = (rect.y + rect.h / 2.0).round();
    let mut paths: Vec<(u32, VectorPath)> = Vec::new();
    for s in &row.segments {
        let (start, end) = match s.span {
            Span::Through => (Point::new(x(s.from), top), Point::new(x(s.from), bottom)),
            Span::IntoNode => (Point::new(x(s.from), top), Point::new(x(s.to), mid)),
            Span::OutOfNode => (Point::new(x(s.from), mid), Point::new(x(s.to), bottom)),
        };
        let index = match paths.iter().position(|(c, _)| *c == s.color) {
            Some(i) => i,
            None => {
                paths.push((s.color, VectorPath::new()));
                paths.len() - 1
            }
        };
        let path = &mut paths[index].1;
        path.move_to(start.x, start.y);
        if start.x == end.x {
            path.line_to(end.x, end.y);
        } else {
            // An S-bend that leaves and arrives vertically, so lanes flow into each other.
            let bend = (end.y - start.y) * 0.6;
            path.cubic_to(Point::new(start.x, start.y + bend), Point::new(end.x, end.y - bend), end);
        }
    }
    paths.sort_by_key(|(c, _)| *c);
    let at = Placement { offset: Point::new(0.0, 0.0), scale: 1.0 };
    for (color, path) in &paths {
        ui.painter.stroke_path(path, at, 1.6, lane_color(*color));
    }

    let center = Point::new(x(row.column), mid);
    let color = lane_color(row.color);
    let background = t.elevated;
    let is_head = row.commit.refs.iter().any(|r| r == "HEAD" || r.starts_with("HEAD -> "));
    if is_head {
        // The checked-out commit: a ring around the dot.
        ui.painter.fill_ellipse(center, 5.5, 5.5, background);
        ui.painter.stroke_ellipse(center, 4.5, 4.5, 2.0, color);
        ui.painter.fill_ellipse(center, 2.5, 2.5, color);
    } else if row.commit.parents.len() > 1 {
        ui.painter.fill_ellipse(center, 3.5, 3.5, background);
        ui.painter.stroke_ellipse(center, 2.7, 2.7, 1.6, color);
    } else {
        ui.painter.fill_ellipse(center, 4.0, 4.0, color);
    }
}

// MARK: - Footer

fn footer(app: &mut App, ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    let cy = rect.center().y;
    let mut x = rect.x + t.s(14.0);
    for (keys, text) in [
        ("↑↓", l("Select", "选择")),
        ("Enter", l("Show in Log", "在历史中查看")),
        ("Tab", l("Branches ⇄ commits", "分支 ⇄ 提交")),
        ("Esc", l("Close", "关闭")),
    ] {
        let caps = components::key_caps(ui, x, cy, keys, 10.0);
        let f = t.font(11.0);
        let w = ui.text_width(&text, f).ceil();
        ui.label_with(Rect::new(caps.right() + t.s(4.0), rect.y, w + 1.0, rect.h), &text, f, t.text3, Align::Left, Overflow::Clip);
        x = caps.right() + t.s(4.0) + w + t.s(14.0);
    }

    // From the right: Fetch, Pull and Push (or what is running), the summary, the spinner.
    let mut right = rect.right() - t.s(14.0);
    let current = app.selected.filter(|id| Some(*id) == app.git_graph.project);
    if let Some(id) = current {
        let (activity, cancellable, ahead, behind) = match app.project(id) {
            Some(p) => (p.activity.clone(), p.activity_cancel.is_some(), p.status.ahead, p.status.behind),
            None => (None, false, 0, 0),
        };
        if let Some(activity) = activity {
            if cancellable && footer_button(ui, Id::new("graph-stop"), &mut right, cy, Icon::Dismiss, &l("Stop", "停止"), &l("Stop it", "停止")) {
                app.cancel_activity(id);
            }
            let f = t.font(11.5);
            let w = ui.text_width(&activity, f).ceil().min(t.s(220.0));
            ui.label_with(Rect::new(right - w, rect.y, w + 1.0, rect.h), &activity, f, t.text2, Align::Left, Overflow::Ellipsis);
            right -= w + t.s(6.0);
            components::spinner(ui, Point::new(right - t.s(6.0), cy), t.s(5.5), t.text3);
            right -= t.s(16.0);
        } else {
            let push = if ahead > 0 { lf!("Push {ahead}", "推送 {ahead}") } else { l("Push", "推送") };
            if footer_button(ui, Id::new("graph-push"), &mut right, cy, Icon::ArrowUp, &push, &l("Push the branch (Ctrl+Shift+K)", "推送当前分支 (Ctrl+Shift+K)")) {
                app.dismiss_popup(false);
                app.push();
                return;
            }
            let pull = if behind > 0 { lf!("Pull {behind}", "拉取 {behind}") } else { l("Pull", "拉取") };
            if footer_button(ui, Id::new("graph-pull"), &mut right, cy, Icon::ArrowDown, &pull, &l("Update the branch from its upstream", "从上游更新当前分支")) {
                app.pull();
            }
            if footer_button(ui, Id::new("graph-fetch"), &mut right, cy, Icon::ArrowClockwise, &l("Fetch", "获取"), &l("Fetch every remote’s branches", "获取所有远程的分支")) {
                app.fetch();
            }
        }
        right -= t.s(4.0);
        ui.vline(right, cy - t.s(8.0), t.s(16.0), t.hairline);
        right -= t.s(8.0);
    }
    let g = &app.git_graph;
    let scope = g.scope_title();
    let n = g.rows.len();
    let summary = if g.is_capped {
        lf!("{scope} · latest {n} commits · older ones in the Log", "{scope} · 最近 {n} 个提交 · 更早的在「历史」里")
    } else {
        let count = if g.has_more { format!("{n}+") } else { n.to_string() };
        lf!("{scope} · {count} commits", "{scope} · {count} 个提交")
    };
    let f = t.font(11.0);
    let left_edge = x;
    let w = ui.text_width(&summary, f).ceil().min((right - left_edge).max(0.0));
    ui.label_with(Rect::new(right - w, rect.y, w + 1.0, rect.h), &summary, f, t.text3, Align::Left, Overflow::Ellipsis);
    right -= w + t.s(8.0);
    if g.is_loading && right - left_edge > t.s(16.0) {
        components::spinner(ui, Point::new(right - t.s(6.0), cy), t.s(5.5), t.text3);
    }
}

/// A small text button in the footer, laid out right to left. Returns whether it was clicked.
fn footer_button(ui: &mut Ui, id: Id, right: &mut f32, cy: f32, icon: Icon, title: &str, tip: &str) -> bool {
    let t = ui.theme;
    let f = t.font_w(11.5, Weight::Medium);
    let tw = ui.text_width(title, f).ceil();
    let is = t.s(10.0);
    let w = t.s(7.0) + is + t.s(4.0) + tw + t.s(7.0);
    let r = Rect::new(*right - w, cy - t.s(11.0), w, t.s(22.0));
    *right -= w + t.s(4.0);
    let hit = ui.interact(id, r);
    if hit.hovered {
        ui.round(r, t.s(5.0), t.hover);
    }
    let color = if hit.hovered { t.text } else { t.text2 };
    ui.icon(icon, Rect::new(r.x + t.s(7.0), cy - is / 2.0, is, is), color);
    ui.label_with(Rect::new(r.x + t.s(7.0) + is + t.s(4.0), r.y, tw + 1.0, r.h), title, f, color, Align::Left, Overflow::Clip);
    ui.tooltip(&hit, tip);
    hit.clicked
}
