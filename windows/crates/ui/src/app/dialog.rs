//! A question or a notice shown inside the window, in DiffCode's own style, instead of a system message box.

use super::{App, AppCallback, Popup};
use crate::core::git::{ChangeKind, FileChange};
use crate::core::lang::l;
use crate::icons::Icon;
use crate::text_field::TextField;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogStyle {
    /// Throws work away (roll back, delete): the confirm button is red.
    Destructive,
    /// Stops something running, or reports a failure.
    Caution,
    Info,
}

/// A line in the dialog's list: a file, or a running program.
#[derive(Clone, Debug, Default)]
pub struct DialogItem {
    pub title: String,
    pub detail: String,
    pub kind: Option<ChangeKind>,
    pub icon: Option<Icon>,
}

pub struct DialogInput {
    pub placeholder: String,
    pub field: TextField,
    /// Why the text cannot be used yet, shown under the field; None when it can.
    pub problem: Box<dyn Fn(&str) -> Option<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct DialogChoice {
    pub title: String,
    pub detail: String,
    /// Throws work away: the confirm button turns red while it is picked.
    pub destructive: bool,
}

#[derive(Clone, Debug)]
pub struct DialogOption {
    pub title: String,
    pub is_on: bool,
}

#[derive(Clone, Debug)]
pub struct DialogAnswer {
    pub text: String,
    pub choice: usize,
    pub option: bool,
}

pub type AnswerCallback = Box<dyn FnOnce(&mut App, DialogAnswer)>;

pub struct DialogRequest {
    pub style: DialogStyle,
    pub icon: Icon,
    pub title: String,
    pub message: String,
    pub items: Vec<DialogItem>,
    /// Items left out of the list: "…and 12 more".
    pub hidden_count: usize,
    /// A program's own words (git's error, a hook's output), shown as they are, in monospace.
    pub output: Option<String>,
    pub confirm_title: String,
    /// None for a notice with a single button.
    pub cancel_title: Option<String>,
    pub on_confirm: Option<AppCallback>,
    pub on_cancel: Option<AppCallback>,
    /// A third answer, between Cancel and the confirm button ("Rebase" beside "Merge").
    pub alternate_title: Option<String>,
    pub on_alternate: Option<AppCallback>,
    /// A line to type (a branch name). The confirm button hands it to `on_answer`.
    pub input: Option<DialogInput>,
    /// Ways to do it, one of which is picked (the modes of a reset); Up and Down move the pick.
    pub choices: Vec<DialogChoice>,
    pub choice: usize,
    /// A box to tick ("Include untracked files").
    pub option: Option<DialogOption>,
    /// Called on confirm with everything that was answered, in place of `on_confirm`.
    pub on_answer: Option<AnswerCallback>,
}

impl DialogRequest {
    pub fn new(style: DialogStyle, icon: Icon, title: impl Into<String>, confirm_title: impl Into<String>) -> Self {
        DialogRequest {
            style,
            icon,
            title: title.into(),
            message: String::new(),
            items: Vec::new(),
            hidden_count: 0,
            output: None,
            confirm_title: confirm_title.into(),
            cancel_title: Some(l("Cancel", "取消")),
            on_confirm: None,
            on_cancel: None,
            alternate_title: None,
            on_alternate: None,
            input: None,
            choices: Vec::new(),
            choice: 0,
            option: None,
            on_answer: None,
        }
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn confirm(mut self, f: impl FnOnce(&mut App) + 'static) -> Self {
        self.on_confirm = Some(Box::new(f));
        self
    }

    pub fn cancel(mut self, f: impl FnOnce(&mut App) + 'static) -> Self {
        self.on_cancel = Some(Box::new(f));
        self
    }

    pub fn answer(mut self, f: impl FnOnce(&mut App, DialogAnswer) + 'static) -> Self {
        self.on_answer = Some(Box::new(f));
        self
    }

    pub fn notice(mut self) -> Self {
        self.cancel_title = None;
        self
    }

    pub fn with_files(mut self, changes: &[FileChange], limit: usize) -> Self {
        let (items, hidden) = files(changes, limit);
        self.items = items;
        self.hidden_count = hidden;
        self
    }

    pub fn with_input(mut self, placeholder: impl Into<String>, text: &str, problem: impl Fn(&str) -> Option<String> + 'static) -> Self {
        let mut field = TextField::with_text(text);
        field.select_all();
        self.input = Some(DialogInput { placeholder: placeholder.into(), field, problem: Box::new(problem) });
        self
    }

    /// Whether the confirm button turns red: the dialog is destructive, or the picked choice is.
    pub fn is_destructive(&self) -> bool {
        self.style == DialogStyle::Destructive || self.choices.get(self.choice).is_some_and(|c| c.destructive)
    }
}

/// The first few files as list items, and how many did not fit.
pub fn files(changes: &[FileChange], limit: usize) -> (Vec<DialogItem>, usize) {
    let items = changes
        .iter()
        .take(limit)
        .map(|c| DialogItem { title: c.name().to_string(), detail: c.directory().to_string(), kind: Some(c.kind), icon: None })
        .collect();
    (items, changes.len().saturating_sub(limit))
}

impl App {
    /// Shows `request`. A newer question replaces an older one, which counts as cancelled.
    pub fn ask(&mut self, request: DialogRequest) {
        if let Some(current) = self.dialog.take() {
            if let Some(cancel) = current.on_cancel {
                cancel(self);
            }
        }
        self.dialog = Some(request);
        self.present(Popup::Dialog);
        self.dialog_focus = true;
    }

    /// The user's answer. Escape, a click outside and Cancel all come here as `false`.
    pub fn answer_dialog(&mut self, confirmed: bool) {
        let Some(request) = self.dialog.as_ref() else { return };
        let mut typed = String::new();
        if confirmed {
            if let Some(input) = &request.input {
                typed = input.field.text.trim().to_string();
                if (input.problem)(&typed).is_some() {
                    return;
                }
            }
        }
        let request = self.dialog.take().expect("dialog");
        self.dismiss_popup(true);
        if !confirmed {
            if let Some(cancel) = request.on_cancel {
                cancel(self);
            }
            return;
        }
        if let Some(answer) = request.on_answer {
            let option = request.option.as_ref().is_some_and(|o| o.is_on);
            answer(self, DialogAnswer { text: typed, choice: request.choice, option });
        } else if let Some(confirm) = request.on_confirm {
            confirm(self);
        }
    }

    pub fn answer_dialog_alternate(&mut self) {
        if self.dialog.as_ref().is_none_or(|d| d.alternate_title.is_none()) {
            return;
        }
        let request = self.dialog.take().expect("dialog");
        self.dismiss_popup(true);
        if let Some(alternate) = request.on_alternate {
            alternate(self);
        }
    }

    /// Picks one of the dialog's choices.
    pub fn pick_dialog_choice(&mut self, index: isize) {
        if let Some(d) = self.dialog.as_mut() {
            if !d.choices.is_empty() {
                d.choice = index.clamp(0, d.choices.len() as isize - 1) as usize;
            }
        }
    }

    pub fn toggle_dialog_option(&mut self) {
        if let Some(o) = self.dialog.as_mut().and_then(|d| d.option.as_mut()) {
            o.is_on = !o.is_on;
        }
    }

    /// Reports a failure. Git's own message is shown as it is; DiffCode's are plain sentences.
    pub fn show_failure(&mut self, title: impl Into<String>, error: &dyn std::fmt::Display, note: Option<String>) {
        let mut request = DialogRequest::new(DialogStyle::Caution, Icon::Warning, title, l("OK", "好")).notice();
        request.output = Some(error.to_string());
        request.message = note.unwrap_or_default();
        self.ask(request);
    }

    /// Reports a failure in DiffCode's own words.
    pub fn show_failure_text(&mut self, title: impl Into<String>, message: impl Into<String>) {
        let request = DialogRequest::new(DialogStyle::Caution, Icon::Warning, title, l("OK", "好")).notice().message(message);
        self.ask(request);
    }
}
