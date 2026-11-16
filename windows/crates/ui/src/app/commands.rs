//! Every user-invocable action in one table, which feeds the keyboard, Find Action and the keymap reference, so a
//! shortcut can never be documented one way and bound another. Shortcuts follow the IDE's default Windows keymap
//! wherever an equivalent action exists.

use std::time::{Duration, Instant};

use super::project::ProjectTab;
use super::search::QuickOpenMode;
use super::{ids, App, Popup};
use crate::core::lang::l;
use crate::icons::Icon;
use crate::input::{Event, Key, Modifiers};
use crate::ui::Ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    File,
    View,
    Navigate,
    Git,
    Terminal,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::File, Group::View, Group::Navigate, Group::Git, Group::Terminal];

    pub fn title(self) -> String {
        match self {
            Group::File => l("File", "文件"),
            Group::View => l("View", "视图"),
            Group::Navigate => l("Navigate", "导航"),
            Group::Git => "Git".into(),
            Group::Terminal => l("Terminal", "终端"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    pub mods: Modifiers,
}

impl Shortcut {
    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.mods.ctrl {
            out.push_str("Ctrl+");
        }
        if self.mods.alt {
            out.push_str("Alt+");
        }
        if self.mods.shift {
            out.push_str("Shift+");
        }
        out.push_str(&key_name(self.key));
        out
    }
}

pub fn key_name(key: Key) -> String {
    match key {
        Key::Enter => "Enter".into(),
        Key::Escape => "Esc".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "Backspace".into(),
        Key::Delete => "Delete".into(),
        Key::Insert => "Insert".into(),
        Key::Left => "←".into(),
        Key::Right => "→".into(),
        Key::Up => "↑".into(),
        Key::Down => "↓".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "PgUp".into(),
        Key::PageDown => "PgDn".into(),
        Key::Space => "Space".into(),
        Key::Shift => "Shift".into(),
        Key::Char(c) => c.to_ascii_uppercase().to_string(),
        Key::F(n) => format!("F{n}"),
    }
}

pub struct Command {
    pub id: &'static str,
    pub title: String,
    pub icon: Icon,
    pub group: Group,
    pub shortcut: Option<Shortcut>,
    /// A second key for the same thing (F5 beside Ctrl+Alt+Y), bound but not shown.
    pub also: Option<Shortcut>,
    /// Shown instead of the shortcut's own label, for keys handled elsewhere (double Shift, Delete in a list).
    pub label: Option<String>,
    pub separator_before: bool,
    pub action: fn(&mut App),
}

impl Command {
    fn new(id: &'static str, title: String, icon: Icon, group: Group, action: fn(&mut App)) -> Self {
        Command { id, title, icon, group, shortcut: None, also: None, label: None, separator_before: false, action }
    }

    fn key(mut self, key: Key, mods: Modifiers) -> Self {
        self.shortcut = Some(Shortcut { key, mods });
        self
    }

    fn also(mut self, key: Key, mods: Modifiers) -> Self {
        self.also = Some(Shortcut { key, mods });
        self
    }

    fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    fn sep(mut self) -> Self {
        self.separator_before = true;
        self
    }

    pub fn shortcut_label(&self) -> String {
        self.label.clone().or_else(|| self.shortcut.map(|s| s.label())).unwrap_or_default()
    }

    fn matches(&self, key: Key, mods: Modifiers) -> bool {
        let hit = |s: &Option<Shortcut>| s.is_some_and(|s| s.key == key && s.mods == mods);
        hit(&self.shortcut) || hit(&self.also)
    }
}

const fn mods(ctrl: bool, alt: bool, shift: bool) -> Modifiers {
    Modifiers { ctrl, shift, alt }
}

const CTRL: Modifiers = mods(true, false, false);
const ALT: Modifiers = mods(false, true, false);
const SHIFT: Modifiers = mods(false, false, true);
const CTRL_SHIFT: Modifiers = mods(true, false, true);
const CTRL_ALT: Modifiers = mods(true, true, false);
const CTRL_ALT_SHIFT: Modifiers = mods(true, true, true);
const NONE: Modifiers = mods(false, false, false);

/// Every command, in menu order.
pub fn commands(app: &App) -> Vec<Command> {
    use Group::*;
    use Key::{Char, F};
    let mut list = vec![
        // File
        Command::new("project.add", l("Add Project…", "添加项目…"), Icon::FolderOpen, File, |a| a.present_add_project()).key(Char('n'), CTRL),
        Command::new("git.refresh", l("Refresh", "刷新"), Icon::ArrowClockwise, File, |a| a.refresh_current())
            .key(Char('y'), CTRL_ALT)
            .also(F(5), NONE),
        // Edits are written on their own a moment after typing stops; this writes them now.
        Command::new("file.save", l("Save", "保存"), Icon::ArrowDownload, File, |a| a.viewer.save_now()).key(Char('s'), CTRL),
        Command::new("file.reveal", l("Reveal in Explorer", "在资源管理器中显示"), Icon::Folder, File, |a| a.reveal_in_explorer()).sep(),
        Command::new("file.copyPath", l("Copy Path", "复制路径"), Icon::Copy, File, |a| a.copy_path(false)).key(Char('c'), CTRL_SHIFT),
        Command::new("file.copyReference", l("Copy Path with Line Number", "复制路径和行号"), Icon::Copy, File, |a| a.copy_path(true))
            .key(Char('c'), CTRL_ALT_SHIFT),
        Command::new("app.settings", l("Settings…", "设置…"), Icon::Settings, File, |a| a.present(Popup::Settings)).key(Char('s'), CTRL_ALT).sep(),
        // View
        Command::new("view.projects", l("Sidebar", "侧边栏"), Icon::PanelLeft, View, |a| a.toggle_sidebar()),
        Command::new("view.files", l("Project Files", "项目文件"), Icon::Folder, View, |a| a.toggle_file_tree()).key(Char('1'), ALT),
        Command::new("view.changes", l("Local Changes", "本地变更"), Icon::TextBulletListTree, View, |a| a.toggle_tab(ProjectTab::Changes))
            .key(Char('0'), ALT),
        Command::new("view.log", l("Git Log", "提交历史"), Icon::History, View, |a| a.toggle_tab(ProjectTab::Log)).key(Char('9'), ALT),
        Command::new("view.terminal", l("Terminal", "终端"), Icon::WindowConsole, View, |a| a.toggle_terminal()).key(F(12), ALT),
        Command::new("view.maximize", l("Hide All Tool Windows", "隐藏所有工具栏"), Icon::ArrowMaximize, View, |a| a.toggle_maximize_viewer())
            .key(F(12), CTRL_SHIFT),
        Command::new("view.unified", l("Toggle Side-by-Side / Unified", "切换 并排 / 合并 视图"), Icon::SplitHorizontal, View, |a| a.toggle_unified())
            .key(Char('v'), CTRL_ALT)
            .sep(),
        Command::new("view.whitespace", l("Ignore Whitespace", "忽略空白差异"), Icon::TextWholeWord, View, |a| a.toggle_whitespace())
            .key(Char('w'), CTRL_ALT),
        Command::new("view.flat", l("Toggle Tree / Flat List", "切换 目录树 / 平铺列表"), Icon::List, View, |a| {
            a.settings.flat_list = !a.settings.flat_list;
            a.settings.save();
        }),
        Command::new("view.fontUp", l("Increase Font Size", "增大字号"), Icon::TextCaseTitle, View, |a| a.change_code_font_size(1.0))
            .key(Char('.'), CTRL_SHIFT)
            .sep(),
        Command::new("view.fontDown", l("Decrease Font Size", "减小字号"), Icon::TextCaseTitle, View, |a| a.change_code_font_size(-1.0))
            .key(Char(','), CTRL_SHIFT),
        Command::new("view.uiFontUp", l("Increase Interface Font Size", "增大界面字号"), Icon::Add, View, |a| a.change_interface_font_size(1.0)),
        Command::new("view.uiFontDown", l("Decrease Interface Font Size", "减小界面字号"), Icon::Subtract, View, |a| a.change_interface_font_size(-1.0)),
        Command::new("view.theme", l("Switch Light / Dark Theme", "切换 浅色 / 深色 主题"), Icon::Circle, View, |a| {
            a.settings.appearance = if a.is_dark() { super::settings::Appearance::Light } else { super::settings::Appearance::Dark };
            a.settings.save();
        }),
        // Navigate
        Command::new("nav.everywhere", l("Search Everywhere", "随处搜索"), Icon::Search, Navigate, |a| a.show_quick_open(QuickOpenMode::All))
            .label(l("Double Shift", "双击 Shift")),
        Command::new("nav.file", l("Go to File…", "查找文件…"), Icon::Document, Navigate, |a| a.show_quick_open(QuickOpenMode::Files))
            .key(Char('n'), CTRL_SHIFT),
        Command::new("nav.findInPath", l("Find in Files…", "在文件中查找…"), Icon::Search, Navigate, |a| a.show_find_in_path())
            .key(Char('f'), CTRL_SHIFT),
        Command::new("nav.action", l("Find Action…", "查找操作…"), Icon::Keyboard, Navigate, |a| a.show_quick_open(QuickOpenMode::Actions))
            .key(Char('a'), CTRL_SHIFT),
        Command::new("nav.recent", l("Recent Files", "最近的文件"), Icon::History, Navigate, |a| a.show_quick_open(QuickOpenMode::Recent))
            .key(Char('e'), CTRL),
        Command::new("nav.line", l("Go to Line…", "跳转到行…"), Icon::ArrowRight, Navigate, |a| a.show_go_to_line()).key(Char('g'), CTRL),
        Command::new("nav.find", l("Find in File…", "在文件中查找…"), Icon::Search, Navigate, |a| a.show_find()).key(Char('f'), CTRL),
        Command::new("nav.findNext", l("Find Next", "查找下一个"), Icon::ArrowDown, Navigate, |a| a.step_find(1)).key(F(3), NONE),
        Command::new("nav.findPrevious", l("Find Previous", "查找上一个"), Icon::ArrowUp, Navigate, |a| a.step_find(-1)).key(F(3), SHIFT),
        Command::new("nav.nextDiff", l("Next Difference", "下一处差异"), Icon::ArrowDown, Navigate, |a| a.viewer.step_difference(1))
            .key(F(7), NONE)
            .sep(),
        Command::new("nav.prevDiff", l("Previous Difference", "上一处差异"), Icon::ArrowUp, Navigate, |a| a.viewer.step_difference(-1))
            .key(F(7), SHIFT),
        // In the IDE these keys mean "next tab" wherever you are; with a terminal focused they walk the terminals.
        Command::new("nav.nextFile", l("Compare Next File", "下一个文件"), Icon::ArrowRight, Navigate, |a| {
            if !a.cycle_terminal(1) {
                a.step_file_or_toast(1);
            }
        })
        .key(Key::Right, ALT),
        Command::new("nav.prevFile", l("Compare Previous File", "上一个文件"), Icon::ArrowLeft, Navigate, |a| {
            if !a.cycle_terminal(-1) {
                a.step_file_or_toast(-1);
            }
        })
        .key(Key::Left, ALT),
        Command::new("nav.locate", l("Select Opened File in Tree", "在文件树中定位当前文件"), Icon::Target, Navigate, |a| a.locate_in_tree())
            .key(F(1), ALT),
        Command::new("nav.showDiff", l("Show Diff", "查看差异"), Icon::ArrowSwap, Navigate, |a| a.focus_viewer()).key(Char('d'), CTRL).sep(),
        Command::new("nav.nextProject", l("Next Project", "下一个项目"), Icon::ChevronDown, Navigate, |a| a.select_project_offset(1))
            .key(Char(']'), CTRL_ALT),
        Command::new("nav.prevProject", l("Previous Project", "上一个项目"), Icon::ChevronRight, Navigate, |a| a.select_project_offset(-1))
            .key(Char('['), CTRL_ALT),
        Command::new("help.keymap", l("Keymap Reference", "快捷键一览"), Icon::Keyboard, Navigate, |a| a.present(Popup::Keymap))
            .key(Char('/'), CTRL)
            .sep(),
        // Git. DiffCode reads the repository and otherwise leaves it alone; these are the exceptions.
        Command::new("git.commit", l("Commit…", "提交…"), Icon::CheckmarkCircle, Git, |a| a.commit_shortcut()).key(Char('k'), CTRL),
        Command::new("git.commitPush", l("Commit and Push…", "提交并推送…"), Icon::ArrowUpload, Git, |a| a.commit_current(true))
            .key(Char('k'), CTRL_ALT),
        Command::new("git.amend", l("Amend Last Commit", "修改上次提交 (Amend)"), Icon::Edit, Git, |a| a.toggle_amend()),
        Command::new("git.undoCommit", l("Undo Last Commit", "撤销上次提交"), Icon::ArrowUndo, Git, |a| a.undo_last_commit()),
        Command::new("git.push", l("Push…", "推送…"), Icon::ArrowUp, Git, |a| a.push()).key(Char('k'), CTRL_SHIFT).sep(),
        Command::new("git.pull", l("Update Project (Pull)", "更新项目 (拉取)"), Icon::ArrowDown, Git, |a| a.pull()),
        Command::new("git.fetch", l("Fetch", "获取远程更新"), Icon::ArrowSync, Git, |a| a.fetch()),
        Command::new("git.newBranch", l("New Branch…", "新建分支…"), Icon::Branch, Git, |a| a.new_branch()),
        Command::new("git.stash", l("Stash Changes…", "Stash 改动…"), Icon::Archive, Git, |a| a.stash_changes()).sep(),
        Command::new("git.stashes", l("Stashes…", "Stash 列表…"), Icon::Archive, Git, |a| a.show_stashes()),
        Command::new("git.continue", l("Continue Merge / Rebase", "继续合并 / 变基"), Icon::ArrowRight, Git, |a| a.continue_operation()).sep(),
        Command::new("git.abort", l("Abort Merge / Rebase…", "中止合并 / 变基…"), Icon::DismissCircle, Git, |a| a.abort_operation()),
        Command::new("git.graph", l("Branches and Git Graph…", "分支与 Git 树…"), Icon::BranchFork, Git, |a| a.toggle_git_graph())
            .key(Char('`'), CTRL_SHIFT)
            .sep(),
        Command::new("git.annotate", l("Annotate with Git Blame", "逐行追溯 (Blame)"), Icon::Person, Git, |a| a.viewer.toggle_annotate()).sep(),
        Command::new("git.history", l("Show History for Current File", "查看当前文件的历史"), Icon::History, Git, |a| {
            a.show_history_for_current_file()
        }),
        Command::new("git.historySelection", l("Show History for Selection", "查看所选内容的历史"), Icon::History, Git, |a| {
            a.show_history_for_selection()
        }),
        Command::new("git.compare", l("Compare with Branch…", "与分支比较…"), Icon::ArrowSwap, Git, |a| a.show_compare_picker()),
        Command::new("git.endCompare", l("Stop Comparing", "退出比较"), Icon::DismissCircle, Git, |a| a.end_comparison()),
        Command::new("git.rollback", l("Rollback…", "回滚…"), Icon::ArrowUndo, Git, |a| a.rollback_in_context()).key(Char('z'), CTRL_ALT).sep(),
        Command::new("git.rollbackHunk", l("Rollback Current Change", "回滚当前这处变更"), Icon::ArrowUndo, Git, |a| a.rollback_current_hunk()),
        Command::new("git.delete", l("Delete New Files…", "删除新增的文件…"), Icon::Delete, Git, |a| a.delete_in_context()).label("Delete"),
        Command::new("git.undo", l("Undo Rollback / Delete", "撤销回滚 / 删除"), Icon::ArrowCounterclockwise, Git, |a| a.undo_edit())
            .label("Ctrl+Z")
            .sep(),
    ];
    if app.has_terminal() {
        list.extend([
            Command::new("term.new", l("New Terminal", "新建终端"), Icon::Add, Terminal, |a| a.open_terminal_in_current_project())
                .key(Char('t'), CTRL_SHIFT),
            Command::new("term.focus", l("Focus Terminal", "聚焦终端"), Icon::WindowConsole, Terminal, |a| a.focus_terminal()),
            Command::new("term.insertPath", l("Insert File Path into Terminal", "把当前文件路径插入终端"), Icon::TextExpand, Terminal, |a| {
                a.insert_reference_in_terminal()
            })
            .key(Char('t'), CTRL_ALT),
            // No key of its own: in a terminal, Ctrl+K belongs to the shell (delete to the end of the line) and Ctrl+L
            // clears the screen there already, as in Windows Terminal.
            Command::new("term.clear", l("Clear Terminal", "清空终端"), Icon::Dismiss, Terminal, |a| a.clear_terminal()).sep(),
            Command::new("term.close", l("Close Terminal", "关闭终端"), Icon::DismissCircle, Terminal, |a| a.close_terminal()).label("Ctrl+Shift+W"),
        ]);
    } else {
        list.retain(|c| c.id != "view.terminal");
    }
    list
}

/// Whether a key pressed in a terminal goes to the program running there rather than to a DiffCode command.
fn terminal_keeps(key: Key, mods: Modifiers) -> bool {
    match (mods.ctrl, mods.alt, mods.shift) {
        (true, false, true) => matches!(key, Key::Char('c') | Key::Char('v')),
        (true, true, _) => false,
        (false, true, _) => !matches!(key, Key::Char('0'..='9') | Key::Left | Key::Right | Key::F(_)),
        _ => true,
    }
}

/// How long the second Shift may follow the first.
const DOUBLE_SHIFT: Duration = Duration::from_millis(400);

/// Runs the command bound to this frame's key press, before any view sees it.
pub fn handle_shortcuts(app: &mut App, ui: &mut Ui) {
    let Some(Event::Key { key, mods, repeat }) = ui.event.clone() else {
        if matches!(ui.event, Some(Event::MouseDown { .. })) {
            app.last_shift = None;
        }
        return;
    };
    if key == Key::Shift {
        if !repeat && mods == SHIFT && app.popup != Some(Popup::Dialog) {
            let now = Instant::now();
            if app.last_shift.is_some_and(|t| now.duration_since(t) < DOUBLE_SHIFT) {
                app.last_shift = None;
                app.show_quick_open(QuickOpenMode::All);
                ui.consume();
            } else {
                app.last_shift = Some(now);
            }
        }
        return;
    }
    app.last_shift = None;
    // A question waits for its answer; an open menu takes the keys it knows.
    if app.popup == Some(Popup::Dialog) || app.menu.is_some() {
        return;
    }
    if ui.is_consumed() {
        return;
    }
    let in_text = ui.focus_is_text();
    let in_editor = app.focus == Some(ids::canvas()) && app.viewer.canvas.editor.is_some();
    match (key, mods) {
        // In text, Ctrl+Z is the text's own; in the editor, its undo first, then the last rollback or delete.
        (Key::Char('z'), CTRL) => {
            if in_text || (in_editor && app.viewer.canvas.can_undo()) {
                return;
            }
            ui.consume();
            app.undo_in_context();
            return;
        }
        // Delete acts on files only where files are selected.
        (Key::Delete, NONE) => {
            let in_list = app.focus == Some(ids::list()) || app.focus == Some(ids::tree());
            if in_list && !in_text {
                ui.consume();
                app.delete_in_context();
            }
            return;
        }
        (Key::Char('w'), CTRL_SHIFT) if app.terminal_has_focus() => {
            ui.consume();
            app.close_terminal();
            return;
        }
        // A terminal takes typing, Ctrl+letters and F-keys for its program; Alt with a digit, an arrow or an F-key,
        // and Ctrl+Shift or Ctrl+Alt chords still reach DiffCode (Ctrl+Shift+C and V copy and paste in the terminal).
        _ if app.terminal_has_focus() && terminal_keeps(key, mods) => return,
        // Find in the file belongs to the viewer; in another field Ctrl+F is left alone.
        (Key::Char('f'), CTRL) | (Key::F(3), _) if in_text && app.focus != Some(ids::find_field()) => return,
        // A text field takes plain keys; F-keys and modified keys still reach the commands.
        _ if in_text && mods.none() && !matches!(key, Key::F(_)) => return,
        _ => {}
    }
    let list = commands(app);
    let Some(command) = list.iter().find(|c| c.matches(key, mods)) else { return };
    ui.consume();
    app.note_action(command.id);
    (command.action)(app);
}

impl App {
    /// Remembers what was run from the keyboard or Find Action, for the recent list in Find Action.
    pub fn note_action(&mut self, id: &str) {
        let actions = &mut self.settings.recent_actions;
        actions.retain(|a| a != id);
        actions.insert(0, id.to_string());
        actions.truncate(8);
    }

    pub fn run_command(&mut self, id: &str) {
        let list = commands(self);
        if let Some(command) = list.iter().find(|c| c.id == id) {
            let action = command.action;
            self.note_action(id);
            self.settings.save();
            action(self);
        }
    }

    pub fn step_file_or_toast(&mut self, delta: i32) {
        if !self.step_file(delta) {
            self.show_toast(if delta > 0 { l("No more changed files", "没有更多变更文件了") } else { l("Already at the first file", "已经是第一个文件") });
        }
    }

    pub fn show_find(&mut self) {
        if !self.viewer.is_text_document() {
            return;
        }
        self.viewer.show_find();
        self.focus_request = Some(ids::find_field());
    }

    pub fn step_find(&mut self, delta: i32) {
        if self.viewer.find_visible {
            self.viewer.step_find(delta);
        } else {
            self.show_find();
        }
    }
}
