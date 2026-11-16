//! The bottom of the Changes list: what will be committed, then the message box with the Commit button in it. The
//! line above it is a divider: drag it to give the message more room or less.

use super::components::{self, ViewState};
use crate::app::project::ProjectId;
use crate::app::settings::COMMIT_BOX_RANGE;
use crate::app::{ids, App};
use crate::core::git::FileChange;
use crate::core::lang::l;
use crate::core::lf;
use crate::core::remote::RepoOperation;
use crate::geom::{Color, Point, Rect};
use crate::icons::Icon;
use crate::paint::{Overflow, Weight};
use crate::platform::Platform;
use crate::text_field::FieldOutcome;
use crate::ui::{Align, Id, Ui};
use crate::widgets::{draw_checkbox, splitter, Check};

struct Summary {
    included: Vec<FileChange>,
    partial: usize,
    added: usize,
    deleted: usize,
    operation: Option<RepoOperation>,
    amend: bool,
    can_commit: bool,
    warning: Option<String>,
}

fn summary(app: &App, id: ProjectId) -> Option<Summary> {
    let p = app.project(id)?;
    let included = p.included_changes();
    // Lines left out of a file do not count.
    let out: Vec<_> = included.iter().filter_map(|c| p.excluded_lines.get(&c.path)).collect();
    let partial = out.iter().filter(|s| !s.is_empty()).count();
    let out_added: usize = out.iter().map(|s| s.iter().filter(|l| !l.removed).count()).sum();
    let out_removed: usize = out.iter().map(|s| s.iter().filter(|l| l.removed).count()).sum();
    let added = included.iter().map(|c| c.additions.unwrap_or(0)).sum::<usize>().saturating_sub(out_added);
    let deleted = included.iter().map(|c| c.deletions.unwrap_or(0)).sum::<usize>().saturating_sub(out_removed);
    let operation = p.status.operation;
    let merging = operation == Some(RepoOperation::Merge);
    let amend = p.amend && operation.is_none();
    let has_message = !p.commit_message.text.trim().is_empty();
    let can_commit = (if merging { p.status.conflict_count() == 0 } else { (!included.is_empty() || amend) && operation.is_none() })
        && !p.is_committing
        && has_message;
    let warning = if amend {
        p.amend_pushed_to.first().map(|remote| {
            let head = p.status.short_head();
            lf!("{head} is already on {remote} · it can then only be pushed by force", "{head} 已推送到 {remote} · 修改后只能强制推送")
        })
    } else {
        None
    };
    Some(Summary { included, partial, added, deleted, operation, amend, can_commit, warning })
}

fn ceiling(column_h: f32) -> f32 {
    COMMIT_BOX_RANGE.0.max(COMMIT_BOX_RANGE.1.min(column_h - 220.0))
}

/// The whole panel's height, for the list above to leave it room.
pub fn height(app: &App, ui: &mut Ui, id: ProjectId, column_h: f32) -> f32 {
    let t = ui.theme;
    let warning = summary(app, id).is_some_and(|s| s.warning.is_some());
    let box_h = app.settings.commit_box_height.min(ceiling(column_h));
    t.s(7.0) + t.s(16.0) + if warning { t.s(7.0) + t.s(30.0) } else { 0.0 } + t.s(7.0) + box_h + t.s(10.0)
}

pub fn commit_panel(app: &mut App, _vs: &mut ViewState, ui: &mut Ui, platform: &mut dyn Platform, id: ProjectId, rect: Rect, column_h: f32) {
    let t = ui.theme;
    let Some(s) = summary(app, id) else { return };
    let Some(p) = app.project(id) else { return };
    let changes = p.changes().len();
    let head = p.status.short_head();
    let is_initial = p.status.is_initial;
    let raw_amend = p.amend;
    let committing = p.is_committing;
    let conflicts = p.status.conflict_count();
    let has_message = !p.commit_message.text.trim().is_empty();
    let last = p.last_commit.as_ref().map(|c| (c.hash.chars().take(7).collect::<String>(), c.amended.is_some()));

    ui.hline(rect.x, rect.y, rect.w, t.hairline);
    let inner = rect.inset_by(t.s(10.0), t.s(7.0), t.s(10.0), t.s(10.0));
    let mut y = inner.y;

    // What goes in.
    let n = s.included.len();
    let partial = s.partial;
    let in_part = if partial == 0 { String::new() } else { lf!(" · {partial} in part", " · {partial} 个部分提交") };
    let text = if s.operation == Some(RepoOperation::Merge) {
        l("Finishing the merge: what it brought, plus the ticked files", "完成合并：合并带来的改动，加上勾选的文件")
    } else if s.amend {
        if n == 0 {
            lf!("Amending {head}: new message only", "修改 {head}：只改提交说明")
        } else {
            lf!("Amending {head}, adding {n} of {changes} files", "修改 {head}，加入 {n} / {changes} 个文件") + &in_part
        }
    } else {
        lf!("{n} of {changes} files to commit", "将提交 {n} / {changes} 个文件") + &in_part
    };
    let line = Rect::new(inner.x, y, inner.w, t.s(16.0));
    let stats = components::stat_badge(ui, line.right(), line.center().y, Some(s.added), Some(s.deleted), 11.0);
    ui.label(Rect::new(line.x, line.y, (stats - t.s(8.0) - line.x).max(0.0), line.h), &text, t.font(11.0), t.text3);
    y += line.h + t.s(7.0);
    if let Some(warning) = &s.warning {
        let icon = t.s(12.0);
        let orange = Color::hex(0xF0A030);
        ui.icon(Icon::Warning, Rect::new(inner.x, y + t.s(2.0), icon, icon), orange);
        let mut style = crate::paint::TextStyle::new(t.font(11.0));
        style.max_width = Some((inner.w - icon - t.s(6.0)).max(10.0));
        style.overflow = Overflow::Wrap;
        let layout = ui.text_layout(warning, style);
        ui.clipped(Rect::new(inner.x, y, inner.w, t.s(30.0)), |ui| {
            ui.draw_layout(&layout, Rect::new(inner.x + icon + t.s(6.0), y, layout.size().w, layout.size().h), orange, Align::Left);
        });
        y += t.s(30.0) + t.s(7.0);
    }

    // The message box, with the buttons inside it under the text.
    let box_h = app.settings.commit_box_height.min(ceiling(column_h));
    let b = Rect::new(inner.x, y, inner.w, box_h.min(inner.bottom() - y).max(t.s(40.0)));
    let editing = ui.has_focus(ids::commit_message());
    let back = ui.interact(Id::new("commit-box"), b);
    ui.round(b, t.s(8.0), t.chip);
    if editing {
        ui.painter.stroke_round_rect(b.inset(0.75), t.s(8.0), 1.5, t.accent.alpha(0.7));
    }
    // A click on the box around the text, by the buttons, still means "write here".
    if back.pressed {
        ui.focus(ids::commit_message());
    }
    let buttons = Rect::new(b.x + t.s(6.0), b.bottom() - t.s(6.0) - t.s(24.0), b.w - t.s(12.0), t.s(24.0));
    let text_rect = Rect::new(b.x + t.s(4.0) + t.s(6.0), b.y + t.s(5.0) + t.s(2.0), b.w - t.s(20.0), buttons.y - t.s(4.0) - b.y - t.s(7.0));
    if let Some(p) = app.project_mut(id) {
        if !p.commit_message.multiline {
            p.commit_message.multiline = true;
        }
        let outcome = components::field_input(ui, platform, ids::commit_message(), &mut p.commit_message);
        let r = ui.interact(ids::commit_message(), text_rect);
        if r.pressed {
            ui.focus(ids::commit_message());
        }
        if r.hovered {
            ui.set_cursor(crate::paint::CursorIcon::Text);
        }
        p.commit_message.draw_text(ui, ids::commit_message(), text_rect, &l("Commit message", "提交说明"), t.font(12.5), editing, &r);
        match outcome {
            FieldOutcome::Submit => app.commit_current(false),
            FieldOutcome::Cancel => app.focus_list(),
            _ => {}
        }
    }

    // Amend and taking back the last commit on the left, Commit on the right.
    let mut x = buttons.x;
    let cy = buttons.center().y;
    if s.operation.is_none() && (!is_initial || raw_amend) {
        let label = l("Amend", "修改上次提交");
        let font = t.font(11.5);
        let w = t.s(4.0) + t.s(16.0) + t.s(4.0) + ui.text_width(&label, font).ceil();
        let area = Rect::new(x, buttons.y, w, buttons.h);
        let r = ui.interact(Id::new("amend"), area);
        draw_checkbox(ui, Rect::new(x + t.s(4.0), buttons.y, t.s(16.0), buttons.h), if raw_amend { Check::On } else { Check::Off }, true, r.hovered);
        ui.label_with(Rect::new(x + t.s(24.0), buttons.y, w - t.s(24.0) + 1.0, buttons.h), &label, font, t.text2, Align::Left, Overflow::Clip);
        ui.tooltip(
            &r,
            &l(
                "Replace the last commit with this one: its changes plus the ticked files, with this message",
                "用这次提交替换上一次提交：保留它的改动，加上勾选的文件，并使用这里的提交说明",
            ),
        );
        if r.clicked {
            app.set_amend(!raw_amend, id);
        }
        x += w + t.s(8.0);
    }
    if let Some((hash, amended)) = &last {
        let label = lf!("Undo {hash}", "撤销 {hash}");
        let font = t.font(11.5);
        let icon = t.s(11.0);
        let w = t.s(6.0) + icon + t.s(4.0) + ui.text_width(&label, font).ceil() + t.s(6.0);
        let area = Rect::new(x, buttons.y, w, buttons.h);
        let r = ui.interact(Id::new("undo-commit"), area);
        if r.hovered {
            ui.round(area, t.s(5.0), t.hover);
        }
        ui.icon(Icon::ArrowUndo, Rect::new(x + t.s(6.0), cy - icon / 2.0, icon, icon), t.text2);
        ui.label_with(Rect::new(x + t.s(6.0) + icon + t.s(4.0), buttons.y, w, buttons.h), &label, font, t.text2, Align::Left, Overflow::Clip);
        let tip = if *amended {
            l("Undo the amend; the replaced commit comes back", "撤销这次修改，被替换的提交会恢复")
        } else {
            l("Undo the commit just made; its changes come back to the list", "撤销刚才的提交，改动会回到列表里")
        };
        ui.tooltip(&r, &tip);
        if r.clicked {
            app.undo_last_commit();
            return;
        }
    }

    let title = if s.operation == Some(RepoOperation::Merge) {
        l("Commit Merge", "提交合并")
    } else if s.amend {
        l("Amend Commit", "修改提交")
    } else {
        l("Commit", "提交")
    };
    let hint = "Ctrl+Enter";
    let font = t.font_w(12.0, Weight::Semibold);
    let hint_font = t.font_w(10.0, Weight::Medium);
    let spin = if committing { t.s(12.0) + t.s(6.0) } else { 0.0 };
    let tw = ui.text_width(&title, font).ceil();
    let hw = ui.text_width(hint, hint_font).ceil();
    let w = t.s(11.0) + spin + tw + t.s(6.0) + hw + t.s(11.0);
    let cb = Rect::new(buttons.right() - w, buttons.y, w, buttons.h);
    let r = ui.interact(Id::new("commit-button"), cb);
    let fill = t.accent.alpha(if s.can_commit { if r.hovered { 0.88 } else { 1.0 } } else { 0.38 });
    ui.round(cb, t.s(6.0), fill);
    let mut bx = cb.x + t.s(11.0);
    if committing {
        components::spinner(ui, Point::new(bx + t.s(6.0), cy), t.s(6.0), Color::WHITE);
        bx += spin;
    }
    ui.label_with(Rect::new(bx, cb.y, tw + 1.0, cb.h), &title, font, Color::WHITE, Align::Left, Overflow::Clip);
    bx += tw + t.s(6.0);
    ui.label_with(Rect::new(bx, cb.y, hw + 1.0, cb.h), hint, hint_font, Color::WHITE.alpha(0.65), Align::Left, Overflow::Clip);
    let tip = if let Some(operation) = s.operation {
        if operation != RepoOperation::Merge {
            let command = operation.command();
            lf!("A {command} is in progress · Continue it above the list", "正在进行 {command} · 请用列表上方的“继续”")
        } else if conflicts > 0 {
            l("Resolve the conflicts first", "请先解决冲突")
        } else if !has_message {
            l("Write a commit message first", "先写提交说明")
        } else {
            l("Commit the merge (Ctrl+Enter)", "提交合并 (Ctrl+Enter)")
        }
    } else if n == 0 && !s.amend {
        l("Tick the files to commit", "先勾选要提交的文件")
    } else if !has_message {
        l("Write a commit message first", "先写提交说明")
    } else if s.amend {
        l("Replace the last commit (Ctrl+Enter)", "替换上一次提交 (Ctrl+Enter)")
    } else {
        l("Commit the ticked files (Ctrl+Enter)", "提交勾选的文件 (Ctrl+Enter)")
    };
    ui.tooltip(&r, &tip);
    if r.clicked && s.can_commit {
        app.commit_current(false);
    }

    // The line above the panel is the divider: dragging it up gives the message more room.
    let mut h = app.settings.commit_box_height;
    let grip = Rect::new(rect.x, rect.y - 3.0, rect.w, 7.0);
    if splitter(ui, Id::new("split-commit"), grip, false, &mut h, COMMIT_BOX_RANGE.0, ceiling(column_h), -1.0) {
        app.settings.save();
    }
    app.settings.commit_box_height = h;
}
