//! A page of every widget, for checking a backend by eye (`DiffCode --gallery out.png` on the Mac harness).

use diffcode_ui::geom::{Color, Rect};
use diffcode_ui::icons::Icon;
use diffcode_ui::paint::{ColorRun, Overflow, Weight};
use diffcode_ui::text_field::TextField;
use diffcode_ui::ui::{Align, Id, Ui};
use diffcode_ui::widgets::{self, ButtonKind, Check};

pub struct Gallery {
    pub field: TextField,
    pub message: TextField,
}

impl Default for Gallery {
    fn default() -> Self {
        Gallery {
            field: TextField::with_text("src/main/java/com/example/提交.kt"),
            message: {
                let mut f = TextField::multiline();
                f.set_text("Fix the line numbers in the gutter\n\n修复折叠后行号错位的问题，并且让长行在窄窗口里自动换行显示。");
                f
            },
        }
    }
}

pub fn draw(ui: &mut Ui, g: &mut Gallery) {
    let t = ui.theme;
    let b = ui.bounds();
    ui.fill(b, t.window);
    let (side, main) = b.split_left(t.s(240.0));
    ui.fill(side, t.sidebar);
    ui.vline(side.right(), 0.0, b.h, t.border);

    // Sidebar: a heading, projects with badges, a filter field.
    let mut y = t.s(14.0);
    ui.label(Rect::new(t.s(14.0), y, side.w, t.s(16.0)), "PROJECTS", t.font_w(11.0, Weight::Semibold), t.text3);
    y += t.s(24.0);
    for (i, (name, branch, count)) in [("DiffCode", "main ↑2", "6"), ("android-app", "feature/登录", "128"), ("website", "gh-pages", "")]
        .iter()
        .enumerate()
    {
        let row = Rect::new(t.s(6.0), y, side.w - t.s(12.0), t.s(40.0));
        let r = ui.interact(Id::new("project").with(i), row);
        if i == 0 {
            ui.round(row, t.s(6.0), t.selection);
        } else if r.hovered {
            ui.round(row, t.s(6.0), t.hover);
        }
        let mark = Rect::new(row.x + t.s(8.0), row.y + t.s(8.0), t.s(24.0), t.s(24.0));
        ui.round(mark, t.s(6.0), t.lane(i as u32));
        ui.label_with(mark, &name[..1].to_uppercase(), t.font_w(12.0, Weight::Bold), Color::WHITE, Align::Center, Overflow::Clip);
        let text_x = mark.right() + t.s(8.0);
        let text_w = row.right() - text_x - t.s(40.0);
        ui.label_with(Rect::new(text_x, row.y + t.s(4.0), text_w, t.s(17.0)), name, t.font_w(13.0, Weight::Medium), t.text, Align::Left, Overflow::Ellipsis);
        ui.label_with(Rect::new(text_x, row.y + t.s(21.0), text_w, t.s(15.0)), branch, t.font(11.5), t.text2, Align::Left, Overflow::Ellipsis);
        if !count.is_empty() {
            widgets::badge(ui, row.right() - t.s(8.0), row.center().y, count, t.chip, t.text2);
        }
        y += t.s(42.0);
    }
    y += t.s(8.0);
    let field = Rect::new(t.s(10.0), y, side.w - t.s(20.0), t.s(28.0));
    g.field.show(ui, Id::new("filter"), field, "Filter files", t.font(12.5));
    y += t.s(40.0);
    // Icons.
    let mut x = t.s(10.0);
    for (i, icon) in Icon::ALL.iter().enumerate() {
        if x + t.s(22.0) > side.right() {
            x = t.s(10.0);
            y += t.s(24.0);
        }
        ui.icon(*icon, Rect::new(x, y, t.s(18.0), t.s(18.0)), if i % 7 == 0 { t.accent } else { t.text2 });
        x += t.s(24.0);
    }

    // Main: toolbar, buttons, checkboxes, text styles, a message field.
    let pad = t.s(20.0);
    let mut y = t.s(12.0);
    let mut x = main.x + pad;
    for (i, (icon, on)) in [(Icon::ArrowUp, false), (Icon::ArrowDown, false), (Icon::SplitVertical, true), (Icon::TextWrap, false), (Icon::Search, false), (Icon::WindowConsole, false)]
        .iter()
        .enumerate()
    {
        widgets::icon_button(ui, Id::new("tool").with(i), Rect::new(x, y, t.s(28.0), t.s(28.0)), *icon, "Tool", i != 1, *on);
        x += t.s(32.0);
    }
    y += t.s(44.0);
    ui.label(Rect::new(main.x + pad, y, main.w, t.s(22.0)), "提交.kt  — app/src/main/java", t.font_w(15.0, Weight::Semibold), t.text);
    y += t.s(28.0);
    let runs = vec![
        ColorRun { range: 0..3, color: t.added },
        ColorRun { range: 4..8, color: t.removed },
    ];
    ui.label_runs(Rect::new(main.x + pad, y, main.w, t.s(18.0)), "+12 −4  ·  1 of 6 differences  ·  12 ms", t.font(12.0), t.text2, runs);
    y += t.s(32.0);
    let mut x = main.x + pad;
    for (i, (label, kind, enabled)) in [
        ("Commit", ButtonKind::Primary, true),
        ("Cancel", ButtonKind::Secondary, true),
        ("Discard", ButtonKind::Danger, true),
        ("More…", ButtonKind::Plain, true),
        ("Disabled", ButtonKind::Primary, false),
    ]
    .iter()
    .enumerate()
    {
        let w = widgets::button_width(ui, label);
        widgets::button(ui, Id::new("button").with(i), Rect::new(x, y, w, t.s(28.0)), label, *kind, *enabled);
        x += w + t.s(8.0);
    }
    y += t.s(44.0);
    let mut x = main.x + pad;
    for (i, state) in [Check::Off, Check::On, Check::Mixed].iter().enumerate() {
        widgets::checkbox(ui, Id::new("check").with(i), Rect::new(x, y, t.s(20.0), t.s(20.0)), *state, true);
        x += t.s(28.0);
    }
    widgets::checkbox(ui, Id::new("check-off"), Rect::new(x, y, t.s(20.0), t.s(20.0)), Check::On, false);
    ui.label(Rect::new(x + t.s(28.0), y + t.s(2.0), t.s(200.0), t.s(16.0)), "Amend", t.font(12.5), t.text);
    let mut cx = x + t.s(100.0);
    for (i, name) in ["main", "origin/main", "v0.3.0"].iter().enumerate() {
        let fill = if i == 2 { t.warning.alpha(0.18) } else { t.accent.alpha(0.14) };
        let r = widgets::chip(ui, cx, y + t.s(10.0), name, t.font_w(11.0, Weight::Medium), fill, if i == 2 { t.warning } else { t.accent });
        cx = r.right() + t.s(6.0);
    }
    y += t.s(40.0);
    // Code text in the mono face with syntax colours and a tab.
    let code = "fun main() {\n\tval name = \"DiffCode\" // 注释\n\treturn 42\n}";
    let c = &t.canvas;
    let code_box = Rect::new(main.x + pad, y, main.w - pad * 2.0, t.s(84.0));
    ui.round(code_box, t.s(6.0), c.background);
    ui.painter.stroke_round_rect(code_box.inset(0.5), t.s(6.0), 1.0, t.hairline);
    let mut ly = code_box.y + t.s(8.0);
    for line in code.lines() {
        let mut runs = Vec::new();
        for (word, kind) in [("fun", diffcode_ui::core::syntax::TokenKind::Keyword), ("val", diffcode_ui::core::syntax::TokenKind::Keyword), ("return", diffcode_ui::core::syntax::TokenKind::Keyword), ("\"DiffCode\"", diffcode_ui::core::syntax::TokenKind::String), ("// 注释", diffcode_ui::core::syntax::TokenKind::Comment), ("42", diffcode_ui::core::syntax::TokenKind::Number), ("main", diffcode_ui::core::syntax::TokenKind::Function)] {
            if let Some(at) = line.find(word) {
                runs.push(ColorRun { range: at as u32..(at + word.len()) as u32, color: c.token(kind) });
            }
        }
        ui.label_runs(Rect::new(code_box.x + t.s(12.0), ly, code_box.w, t.s(17.0)), line, t.code_font(), c.text, runs);
        ly += t.s(17.0);
    }
    y += t.s(100.0);
    // Ellipsis at the end and in the middle.
    let path = "app/src/main/java/com/nanshanbei/diffcode/very/long/package/name/ChangeTreeViewModel.kt";
    ui.label_with(Rect::new(main.x + pad, y, t.s(300.0), t.s(16.0)), path, t.font(12.0), t.text2, Align::Left, Overflow::Ellipsis);
    y += t.s(20.0);
    ui.label_with(Rect::new(main.x + pad, y, t.s(300.0), t.s(16.0)), path, t.font(12.0), t.text2, Align::Left, Overflow::EllipsisMiddle);
    y += t.s(28.0);
    let msg = Rect::new(main.x + pad, y, t.s(360.0), t.s(90.0));
    g.message.show(ui, Id::new("message"), msg, "Commit message", t.font(13.0));
    if ui.st.focus.is_none() {
        ui.focus(Id::new("message"));
    }

    // A popup with a shadow on top.
    let pop = Rect::new(main.right() - t.s(260.0), t.s(60.0), t.s(230.0), t.s(150.0));
    ui.with_layer(diffcode_ui::ui::layer::POPUP, |ui| {
        let t = ui.theme;
        ui.painter.shadow(pop, t.s(8.0), t.s(16.0), t.shadow);
        ui.round(pop, t.s(8.0), t.elevated);
        ui.painter.stroke_round_rect(pop.inset(0.5), t.s(8.0), 1.0, t.hairline);
        let mut y = pop.y + t.s(6.0);
        for (i, (icon, label)) in [(Icon::Branch, "Checkout"), (Icon::Merge, "Merge into main"), (Icon::Edit, "Rename…"), (Icon::Delete, "Delete")].iter().enumerate() {
            let row = Rect::new(pop.x + t.s(6.0), y, pop.w - t.s(12.0), t.s(30.0));
            if i == 1 {
                ui.round(row, t.s(5.0), t.accent);
            }
            let color = if i == 1 { Color::WHITE } else if i == 3 { t.removed } else { t.text };
            ui.icon(*icon, Rect::new(row.x + t.s(8.0), row.y + t.s(7.0), t.s(16.0), t.s(16.0)), color);
            ui.label(Rect::new(row.x + t.s(32.0), row.y + t.s(7.0), row.w, t.s(16.0)), label, t.font(13.0), color);
            y += t.s(32.0);
        }
        ui.block(Id::new("popup"), pop);
    });
}

/// The gallery as a whole program, for a first look at the Windows backend (`DiffCode --gallery`).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
pub struct GalleryProgram {
    gallery: Gallery,
}

impl diffcode_ui::platform::Program for GalleryProgram {
    fn theme(&mut self, system_dark: bool) -> diffcode_ui::theme::Theme {
        diffcode_ui::theme::Theme::new(system_dark, 13.0, 13.0)
    }

    fn frame(&mut self, ui: &mut Ui, platform: &mut dyn diffcode_ui::platform::Platform) {
        // Typing goes to whichever field has the focus.
        if let Some(event) = ui.event.clone() {
            for (id, field) in [(Id::new("filter"), &mut self.gallery.field), (Id::new("message"), &mut self.gallery.message)] {
                if ui.has_focus(id) {
                    let outcome = field.handle(&event, platform.clipboard());
                    if outcome != diffcode_ui::text_field::FieldOutcome::Ignored {
                        ui.consume();
                        ui.request_repaint();
                    }
                }
            }
        }
        draw(ui, &mut self.gallery);
    }
}
