//! The left column: the projects, and under them the file tree of the one that is selected. Also the screens shown
//! where the panels would be: the welcome screen, and the one for a machine without Git.

use super::components::{self, ViewState};
use super::file_tree;
use crate::app::project::ProjectId;
use crate::app::{App, MenuItem, PlatformRequest, Popup};
use crate::core::lang::l;
use crate::core::lf;
use crate::geom::{Color, Point, Rect, Size};
use crate::icons::Icon;
use crate::paint::{Overflow, Weight};
use crate::platform::Platform;
use crate::ui::{Align, Id, Ui};
use crate::widgets::{badge, button, button_width, icon_button, scroll_area, ButtonKind};

const ROW_SPACING: f32 = 2.0;
const BOTTOM_PADDING: f32 = 6.0;

pub fn sidebar(app: &mut App, vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, rect: Rect) {
    let t = ui.theme;
    ui.fill(rect, t.sidebar);
    let mut area = rect;

    // The header: a caption and the add button.
    let header = area.take_top(t.s(34.0));
    components::section_label(ui, header.x + t.s(16.0), header.center().y + t.s(1.0), header.w - t.s(60.0), &l("Projects", "项目"));
    let b = Rect::new(header.right() - t.s(8.0) - t.s(26.0), header.center().y - t.s(12.0) + t.s(1.0), t.s(26.0), t.s(24.0));
    if icon_button(ui, Id::new("add-project"), b, Icon::Add, &l("Add Project (Ctrl+N)", "添加项目 (Ctrl+N)"), true, false).clicked {
        app.present_add_project();
    }

    let footer = area.take_bottom(t.s(34.0));
    ui.hline(footer.x, footer.y, footer.w, t.hairline.alpha(0.5));
    sidebar_footer(app, ui, footer);

    let shows_tree = app.current_project().is_some() && !app.git_missing;
    if shows_tree {
        // The projects take the room they need, up to about a third; the tree gets everything else.
        let h = projects_height(t.s(40.0), app.projects.len(), rect.h);
        let list = area.take_top(h.min(area.h));
        projects(app, vs, ui, list);
        ui.hline(area.x, area.y, area.w, t.hairline);
        area.take_top(1.0);
        file_tree::file_tree(app, vs, ui, platform, area);
    } else {
        projects(app, vs, ui, area);
    }
}

/// Every project when they fit; otherwise a cut that leaves half a row showing, so the list reads as scrollable.
fn projects_height(row: f32, count: usize, available: f32) -> f32 {
    let step = row + ROW_SPACING;
    let all = count.max(1) as f32 * step - ROW_SPACING + BOTTOM_PADDING;
    let limit = (step * 2.5).max((step * 6.5).min(available * 0.34));
    if all <= limit {
        all
    } else {
        ((limit / step - 0.5).floor() + 0.5) * step
    }
}

fn projects(app: &mut App, vs: &mut ViewState, ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    let row_h = t.s(40.0);
    let step = row_h + ROW_SPACING;
    let content = app.projects.len() as f32 * step - ROW_SPACING + BOTTOM_PADDING;
    let scroll = vs.scroll(Id::new("projects"));
    ui.block(Id::new("projects-back"), rect);
    scroll_area(ui, Id::new("projects-scroll"), rect, content, scroll);
    let offset = scroll.offset;
    let ids: Vec<ProjectId> = app.projects.iter().map(|p| p.id).collect();
    ui.clipped(rect, |ui| {
        for (i, id) in ids.into_iter().enumerate() {
            let y = rect.y + i as f32 * step - offset;
            let row = Rect::new(rect.x + t.s(8.0), y, rect.w - t.s(16.0), row_h);
            if row.bottom() < rect.y || row.y > rect.bottom() {
                continue;
            }
            project_row(app, ui, id, row);
        }
    });
}

fn project_row(app: &mut App, ui: &mut Ui, id: ProjectId, row: Rect) {
    let t = ui.theme;
    let selected = app.selected == Some(id);
    let Some(p) = app.project(id) else { return };
    let name = p.name.clone();
    let path = p.url.to_string_lossy().to_string();
    let error = p.error_message.clone();
    let count = p.changes().len();
    let r = ui.interact(Id::new("project-row").with(id), row);
    let fill = if selected {
        t.selection
    } else if r.hovered {
        t.hover
    } else {
        Color::TRANSPARENT
    };
    if fill.a > 0.0 {
        ui.round(row, t.s(8.0), fill);
    }
    let avatar = t.s(24.0);
    components::avatar(ui, Rect::new(row.x + t.s(8.0), row.center().y - avatar / 2.0, avatar, avatar), &name);
    let text_x = row.x + t.s(8.0) + avatar + t.s(9.0);
    let mut right = row.right() - t.s(8.0);

    // What sits at the end of the row: a warning, or how many files changed.
    if let Some(message) = &error {
        let s = t.s(13.0);
        let w = Rect::new(right - s, row.center().y - s / 2.0, s, s);
        ui.icon(Icon::Warning, w, Color::hex(0xF0A030));
        let hover = ui.interact(Id::new("project-error").with(id), w.inset(-t.s(3.0)));
        ui.tooltip(&hover, message);
        right = w.x - t.s(4.0);
    } else if count > 0 {
        let (fill, color) = if selected { (t.accent, Color::WHITE) } else { (t.chip, t.text2) };
        let b = badge(ui, right, row.center().y, &count.to_string(), fill, color);
        right = b.x - t.s(4.0);
    }
    // A terminal is open here; orange once it rings for attention (an agent waiting on you).
    if app.terminals.has_sessions(id) {
        let waiting = app.terminals.needs_attention(id);
        let s = t.s(14.0);
        let icon = Rect::new(right - s, row.center().y - s / 2.0, s, s);
        let color = if waiting { Color::hex(if t.dark { 0xFF9F0A } else { 0xFF9500 }) } else { t.text3 };
        ui.icon(Icon::WindowConsole, icon, color);
        let hover = ui.interact(Id::new("project-terminal").with(id), icon.inset(-t.s(3.0)));
        let tip = if waiting { l("A terminal here wants attention", "这里有终端在等你") } else { l("Has open terminals", "有打开的终端") };
        ui.tooltip(&hover, &tip);
        if hover.clicked {
            app.select(id);
        }
        right = icon.x - t.s(4.0);
    }
    let name_rect = Rect::new(text_x, row.y + t.s(4.0), (right - text_x).max(0.0), t.s(17.0));
    ui.label(name_rect, &name, t.font_w(13.0, Weight::Medium), t.text);
    let branch_rect = Rect::new(text_x, row.y + t.s(21.0), (right - text_x).max(0.0), t.s(16.0));
    let label_hit = branch_label(app, ui, id, branch_rect);

    ui.tooltip(&r, &path);
    if r.clicked && !label_hit {
        app.select(id);
    }
    if r.secondary {
        project_menu(app, id, ui.mouse());
    }
}

/// The branch under a project's name. A click opens that project's branches and commit graph just below it.
/// Returns whether the pointer is on it (so the row's own click leaves it alone).
fn branch_label(app: &mut App, ui: &mut Ui, id: ProjectId, rect: Rect) -> bool {
    let t = ui.theme;
    let Some(p) = app.project(id) else { return false };
    let label = p.branch_label();
    let activity = p.activity.clone();
    let (ahead, behind) = (p.status.ahead, p.status.behind);
    let operation = p.status.operation.map(|o| o.title());
    let upstream = p.status.upstream.clone();
    let open = app.popup == Some(Popup::GitGraph) && app.git_graph.project == Some(id);

    // Measure first: the highlight hugs the content.
    let font = t.font(11.0);
    let small = t.font_w(10.5, Weight::Medium);
    let icon = t.s(11.0);
    let gap = t.s(3.0);
    let mut extras: Vec<(String, Color)> = Vec::new();
    if activity.is_none() {
        if ahead > 0 {
            extras.push((format!("↑{ahead}"), Color::TRANSPARENT));
        }
        if behind > 0 {
            extras.push((format!("↓{behind}"), Color::TRANSPARENT));
        }
    }
    if let Some(op) = &operation {
        extras.push((op.clone(), Color::hex(0xF0A030)));
    }
    let extras_w: f32 = extras.iter().map(|(s, _)| ui.text_width(s, small).ceil() + gap).sum();
    let spinner_w = if activity.is_some() { t.s(10.0) + gap } else { 0.0 };
    let chevron = t.s(9.0);
    let fixed = icon + gap + spinner_w + extras_w + chevron;
    let name_w = ui.text_width(&label, font).ceil().min((rect.w - fixed - t.s(8.0)).max(t.s(20.0)));
    let content_w = fixed + name_w;
    let hit = Rect::new(rect.x - t.s(4.0), rect.y, content_w + t.s(8.0), rect.h);
    let r = ui.interact(Id::new("branch-label").with(id), hit);
    let lit = r.hovered || open;
    if lit {
        ui.round(hit.inset_xy(0.0, t.s(0.5)), t.s(4.0), t.text.alpha(0.1));
    }
    let color = if lit { t.text } else { t.text2 };
    let cy = rect.center().y;
    let mut x = rect.x;
    ui.icon(Icon::Branch, Rect::new(x, cy - icon / 2.0, icon, icon), color);
    x += icon + gap;
    ui.label_with(Rect::new(x, rect.y, name_w + 1.0, rect.h), &label, font, color, Align::Left, Overflow::EllipsisMiddle);
    x += name_w + gap;
    if activity.is_some() {
        components::spinner(ui, Point::new(x + t.s(5.0), cy), t.s(5.0), color);
        x += spinner_w;
    }
    for (text, c) in &extras {
        let w = ui.text_width(text, small).ceil();
        let c = if c.a > 0.0 { *c } else { color };
        ui.label_with(Rect::new(x, rect.y, w + 1.0, rect.h), text, small, c, Align::Left, Overflow::Clip);
        x += w + gap;
    }
    if lit {
        ui.icon(Icon::ChevronDown, Rect::new(x, cy - chevron / 2.0, chevron, chevron), color);
    }
    app.branch_label_anchors.insert(id, Point::new(hit.x, hit.bottom() + t.s(4.0)));

    let mut tip: Vec<String> = Vec::new();
    if let Some(a) = &activity {
        tip.push(a.clone());
    }
    if let Some(u) = upstream {
        tip.push(lf!("Tracking {u}", "跟踪 {u}"));
    }
    tip.push(l("Branches and commit graph (Ctrl+Shift+`)", "分支与提交树 (Ctrl+Shift+`)"));
    ui.tooltip(&r, &tip.join("\n"));
    if r.clicked {
        app.toggle_git_graph_for(id, Point::new(hit.x, hit.bottom() + t.s(4.0)));
    }
    r.hovered
}

fn project_menu(app: &mut App, id: ProjectId, at: Point) {
    let Some(p) = app.project(id) else { return };
    let url = p.url.clone();
    let index = app.projects.iter().position(|p| p.id == id).unwrap_or(0);
    let last = app.projects.len().saturating_sub(1);
    let path = url.to_string_lossy().to_string();
    let items = vec![
        MenuItem::new(l("Reveal in File Explorer", "在资源管理器中显示"), move |app| app.requests.push(PlatformRequest::Reveal(url))),
        MenuItem::new(l("Copy Path", "复制路径"), move |app| app.copy_text(path)),
        MenuItem::new(l("Refresh", "刷新"), move |app| {
            if let Some(p) = app.project_mut(id) {
                p.refresh();
            }
        }),
        MenuItem::separator(),
        MenuItem::new(l("Move Up", "上移"), move |app| app.move_project(id, -1)).disabled(index == 0),
        MenuItem::new(l("Move Down", "下移"), move |app| app.move_project(id, 1)).disabled(index == last),
        MenuItem::separator(),
        MenuItem::new(l("Remove from DiffCode", "从 DiffCode 中移除"), move |app| app.remove_project(id)),
    ];
    app.open_menu(at, items);
}

fn sidebar_footer(app: &mut App, ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    // The whole point of DiffCode is being light; keep the number honest and visible.
    let icon = t.s(11.0);
    let x = rect.x + t.s(14.0);
    ui.icon(Icon::Flash, Rect::new(x, rect.center().y - icon / 2.0, icon, icon), t.text3);
    let text = format!("{:.0} MB", app.memory_mb);
    let font = t.font_w(11.0, Weight::Medium);
    let w = ui.text_width(&text, font).ceil();
    let label = Rect::new(x + icon + t.s(4.0), rect.y, w + 1.0, rect.h);
    ui.label_with(label, &text, font, t.text3, Align::Left, Overflow::Clip);
    let hover = ui.interact(Id::new("memory"), Rect::new(x, rect.y, label.right() - x, rect.h));
    ui.tooltip(&hover, &l("Memory used by DiffCode right now", "DiffCode 当前占用的内存"));

    let size = Size::new(t.s(26.0), t.s(24.0));
    let settings = Rect::new(rect.right() - t.s(8.0) - size.w, rect.center().y - size.h / 2.0, size.w, size.h);
    let keymap = settings.offset(-(size.w + t.s(2.0)), 0.0);
    if icon_button(ui, Id::new("footer-keymap"), keymap, Icon::Keyboard, &l("Keymap Reference (Ctrl+/)", "快捷键一览 (Ctrl+/)"), true, false).clicked {
        app.present(Popup::Keymap);
    }
    if icon_button(ui, Id::new("footer-settings"), settings, Icon::Settings, &l("Settings (Ctrl+Alt+S)", "设置 (Ctrl+Alt+S)"), true, false).clicked {
        app.present(Popup::Settings);
    }

    // A newer release: a quiet pill until it is installed or skipped (just the arrow when the sidebar is narrow).
    if let Some(release) = app.update_offered() {
        let v = release.version.clone();
        let text = l("Update", "更新");
        let font = t.font_w(11.0, Weight::Semibold);
        let glyph = t.s(12.0);
        let full = t.s(8.0) + glyph + t.s(4.0) + ui.text_width(&text, font).ceil() + t.s(9.0);
        let right = keymap.x - t.s(6.0);
        let w = if right - full >= label.right() + t.s(8.0) { full } else { t.s(26.0) };
        let r = Rect::new(right - w, rect.center().y - t.s(11.0), w, t.s(22.0));
        let hit = ui.interact(Id::new("footer-update"), r);
        ui.round(r, t.s(11.0), t.accent.alpha(if hit.hovered { 0.24 } else { 0.14 }));
        let gx = if w == full { r.x + t.s(8.0) } else { r.center().x - glyph / 2.0 };
        ui.icon(Icon::ArrowDownload, Rect::new(gx, r.center().y - glyph / 2.0, glyph, glyph), t.accent);
        if w == full {
            ui.label_with(Rect::new(gx + glyph + t.s(4.0), r.y, r.right() - gx - glyph - t.s(4.0), r.h), &text, font, t.accent, Align::Left, Overflow::Clip);
        }
        ui.tooltip(&hit, &lf!("DiffCode {v} is available", "DiffCode {v} 已发布"));
        if hit.clicked {
            app.show_update_dialog();
        }
    }
}

/// Shown when there are no projects yet.
pub fn welcome(app: &mut App, ui: &mut Ui, platform: &mut dyn Platform, area: Rect) {
    let t = ui.theme;
    let button_h = t.s(36.0);
    let (glyph, y) = super::notice_layout(
        ui,
        area,
        &l("Welcome to DiffCode", "欢迎使用 DiffCode"),
        &l("A featherweight viewer for Git changes.", "一个极轻量的 Git 变更查看器。"),
        button_h + t.s(18.0) + t.s(18.0),
    );
    // The app's own icon, at the size in the icon nearest the pixels it covers.
    let pixels = (glyph.w * ui.painter.scale()).round() as u32;
    if app.app_icon.as_ref().is_none_or(|(p, _)| *p != pixels) {
        app.app_icon = platform.app_icon(pixels).map(|png| (pixels, crate::paint::ImageSource::new(png)));
    }
    match &app.app_icon {
        Some((_, icon)) => ui.painter.image(icon, glyph),
        None => {
            ui.round(glyph, t.s(12.0), Color::hex(0x2B2D35));
            ui.icon(Icon::ArrowSwap, glyph.inset(t.s(11.0)), Color::hex(0x4C8DFF));
        }
    }

    let label = l("Add a Git Repository", "添加 Git 仓库");
    let font = t.font_w(13.0, Weight::Medium);
    let caps = "Ctrl+N";
    let caps_w = components::key_caps_width(ui, caps, 11.0);
    let icon = t.s(14.0);
    let w = t.s(16.0) + icon + t.s(8.0) + ui.text_width(&label, font).ceil() + t.s(8.0) + caps_w + t.s(16.0);
    let b = Rect::new(area.center().x - w / 2.0, y, w, button_h).snapped(ui.painter.scale());
    let r = ui.interact(Id::new("welcome-add"), b);
    ui.round(b, t.s(9.0), if r.hovered { t.selection.alpha(1.5f32.min(1.0 / t.selection.a.max(0.01))) } else { t.selection });
    let mut x = b.x + t.s(16.0);
    ui.icon(Icon::Add, Rect::new(x, b.center().y - icon / 2.0, icon, icon), t.text);
    x += icon + t.s(8.0);
    let lw = ui.text_width(&label, font).ceil();
    ui.label_with(Rect::new(x, b.y, lw + 1.0, b.h), &label, font, t.text, Align::Left, Overflow::Clip);
    x += lw + t.s(8.0);
    components::key_caps(ui, x, b.center().y, caps, 11.0);
    if r.clicked {
        app.present_add_project();
    }
    ui.label_with(
        Rect::new(area.x, b.bottom() + t.s(14.0), area.w, t.s(18.0)),
        &l("…or drop folders anywhere in this window", "…或者把文件夹拖进这个窗口"),
        t.font(12.0),
        t.text3,
        Align::Center,
        Overflow::Ellipsis,
    );
}

/// Shown instead of the panels when no Git is found.
pub fn git_missing(app: &mut App, ui: &mut Ui, area: Rect) {
    let t = ui.theme;
    let button_h = t.s(32.0);
    let (glyph, y) = super::notice_layout(
        ui,
        area,
        &l("DiffCode needs Git", "DiffCode 需要 Git"),
        &l(
            "It uses Git for Windows. Install it from git-scm.com (the default options are fine), then check again.",
            "DiffCode 使用 Git for Windows。请从 git-scm.com 安装（默认选项即可），装好后再检查一次。",
        ),
        button_h,
    );
    ui.icon(Icon::Cube, glyph.inset(t.s(4.0)), t.text3);
    let download = l("Download Git for Windows", "下载 Git for Windows");
    let again = l("Check Again", "重新检查");
    let w1 = button_width(ui, &download) + t.s(8.0);
    let w2 = button_width(ui, &again) + t.s(8.0);
    let gap = t.s(10.0);
    let x = area.center().x - (w1 + gap + w2) / 2.0;
    if button(ui, Id::new("git-download"), Rect::new(x, y, w1, button_h), &download, ButtonKind::Primary, true).clicked {
        app.requests.push(PlatformRequest::OpenUrl("https://git-scm.com/download/win".into()));
    }
    if button(ui, Id::new("git-again"), Rect::new(x + w1 + gap, y, w2, button_h), &again, ButtonKind::Secondary, true).clicked {
        app.check_git_again();
    }
}
