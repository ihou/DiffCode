//! User preferences, kept in a small `key=value` file (on Windows, `%APPDATA%\DiffCode\settings.ini`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::lang::l;

pub const UI_FONT_RANGE: (f32, f32) = (11.0, 17.0);
pub const CODE_FONT_RANGE: (f32, f32) = (9.0, 24.0);
pub const TERMINAL_FONT_RANGE: (f32, f32) = (9.0, 24.0);
/// The sidebar holds the project list and the file tree, so it may grow wider than a plain list.
pub const SIDEBAR_RANGE: (f32, f32) = (200.0, 480.0);
/// The commit message box with its button row; dragged by the line above it.
pub const COMMIT_BOX_RANGE: (f32, f32) = (96.0, 640.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn title(self) -> String {
        match self {
            Appearance::System => l("System", "跟随系统"),
            Appearance::Light => l("Light", "浅色"),
            Appearance::Dark => l("Dark", "深色"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }
}

/// The interface language; read once at launch, so a change shows after a restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    System,
    English,
    Chinese,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::System, Language::English, Language::Chinese];

    /// Language names in their own language, so people find theirs whatever the interface shows.
    pub fn title(self) -> String {
        match self {
            Language::System => l("System", "跟随系统"),
            Language::English => "English".into(),
            Language::Chinese => "简体中文".into(),
        }
    }

    pub fn is_chinese(self, system_chinese: bool) -> bool {
        match self {
            Language::System => system_chinese,
            Language::English => false,
            Language::Chinese => true,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Language::System => "system",
            Language::English => "en",
            Language::Chinese => "zh-Hans",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub appearance: Appearance,
    pub language: Language,
    /// Code size in the viewer.
    pub font_size: f32,
    /// Interface text (lists, headers, popups); everything in the chrome scales from it.
    pub ui_font_size: f32,
    pub terminal_font_size: f32,
    pub unified: bool,
    pub ignore_whitespace: bool,
    pub flat_list: bool,
    pub sidebar_width: f32,
    pub list_width: f32,
    pub terminal_height: f32,
    pub commit_box_height: f32,
    /// Keep the file tree's selection on whatever file the viewer shows.
    pub follow_file: bool,
    /// Projects in the sidebar, in order, and the one selected.
    pub projects: Vec<PathBuf>,
    pub selected_project: Option<PathBuf>,
    /// The window's place: x, y, width, height in pixels, and whether maximised.
    pub window: Option<(i32, i32, i32, i32, bool)>,
    pub recent_actions: Vec<String>,
    file: Option<PathBuf>,
}

fn clamp(v: f32, (lo, hi): (f32, f32)) -> f32 {
    v.clamp(lo, hi)
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            // First launch is dark; a choice made in Settings is kept.
            appearance: Appearance::Dark,
            language: Language::System,
            font_size: 12.0,
            ui_font_size: 13.0,
            terminal_font_size: 12.0,
            unified: false,
            ignore_whitespace: false,
            flat_list: false,
            sidebar_width: 240.0,
            list_width: 300.0,
            terminal_height: 280.0,
            commit_box_height: 140.0,
            follow_file: false,
            projects: Vec::new(),
            selected_project: None,
            window: None,
            recent_actions: Vec::new(),
            file: None,
        }
    }
}

impl Settings {
    /// Reads the settings in `file`; missing or unreadable values keep their defaults.
    pub fn load(file: &Path) -> Settings {
        let mut s = Settings { file: Some(file.to_path_buf()), ..Settings::default() };
        let Ok(text) = std::fs::read_to_string(file) else { return s };
        let mut map: HashMap<&str, &str> = HashMap::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "project" => s.projects.push(PathBuf::from(v)),
                "recentAction" => s.recent_actions.push(v.to_string()),
                _ => {
                    map.insert(k, v);
                }
            }
        }
        let f = |k: &str| map.get(k).and_then(|v| v.parse::<f32>().ok());
        let b = |k: &str| map.get(k).map(|v| *v == "true");
        if let Some(v) = map.get("appearance") {
            s.appearance = Appearance::ALL.into_iter().find(|a| a.key() == *v).unwrap_or(Appearance::Dark);
        }
        if let Some(v) = map.get("language") {
            s.language = Language::ALL.into_iter().find(|a| a.key() == *v).unwrap_or(Language::System);
        }
        if let Some(v) = f("fontSize") {
            s.font_size = clamp(v, CODE_FONT_RANGE);
        }
        if let Some(v) = f("uiFontSize") {
            s.ui_font_size = clamp(v, UI_FONT_RANGE);
        }
        if let Some(v) = f("terminalFontSize") {
            s.terminal_font_size = clamp(v, TERMINAL_FONT_RANGE);
        }
        s.unified = b("unified").unwrap_or(false);
        s.ignore_whitespace = b("ignoreWhitespace").unwrap_or(false);
        s.flat_list = b("flatList").unwrap_or(false);
        if let Some(v) = f("sidebarWidth") {
            s.sidebar_width = clamp(v, SIDEBAR_RANGE);
        }
        if let Some(v) = f("listWidth") {
            s.list_width = v.max(160.0);
        }
        if let Some(v) = f("terminalHeight") {
            s.terminal_height = v.max(80.0);
        }
        if let Some(v) = f("commitBoxHeight") {
            s.commit_box_height = clamp(v, COMMIT_BOX_RANGE);
        }
        s.follow_file = b("followFile").unwrap_or(false);
        s.selected_project = map.get("selectedProject").map(PathBuf::from);
        if let Some(v) = map.get("window") {
            let parts: Vec<&str> = v.split(',').collect();
            if parts.len() == 5 {
                let n = |i: usize| parts[i].trim().parse::<i32>().ok();
                if let (Some(x), Some(y), Some(w), Some(h)) = (n(0), n(1), n(2), n(3)) {
                    s.window = Some((x, y, w, h, parts[4].trim() == "1"));
                }
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(file) = &self.file else { return };
        let mut out = String::new();
        let mut put = |k: &str, v: String| {
            out.push_str(k);
            out.push('=');
            out.push_str(&v);
            out.push('\n');
        };
        put("appearance", self.appearance.key().into());
        put("language", self.language.key().into());
        put("fontSize", self.font_size.to_string());
        put("uiFontSize", self.ui_font_size.to_string());
        put("terminalFontSize", self.terminal_font_size.to_string());
        put("unified", self.unified.to_string());
        put("ignoreWhitespace", self.ignore_whitespace.to_string());
        put("flatList", self.flat_list.to_string());
        put("sidebarWidth", self.sidebar_width.to_string());
        put("listWidth", self.list_width.to_string());
        put("terminalHeight", self.terminal_height.to_string());
        put("commitBoxHeight", self.commit_box_height.to_string());
        put("followFile", self.follow_file.to_string());
        if let Some(p) = &self.selected_project {
            put("selectedProject", p.display().to_string());
        }
        if let Some((x, y, w, h, max)) = self.window {
            put("window", format!("{x},{y},{w},{h},{}", if max { 1 } else { 0 }));
        }
        for p in &self.projects {
            put("project", p.display().to_string());
        }
        for a in &self.recent_actions {
            put("recentAction", a.clone());
        }
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Written beside and moved over, so a crash mid-write never leaves half a file.
        let temp = file.with_extension("ini.tmp");
        if std::fs::write(&temp, out).is_ok() {
            let _ = std::fs::rename(&temp, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.ini");
        let mut s = Settings::load(&file);
        assert_eq!(s.appearance, Appearance::Dark);
        s.unified = true;
        s.ui_font_size = 15.0;
        s.projects = vec![PathBuf::from("C:\\work\\a"), PathBuf::from("D:\\b c")];
        s.selected_project = Some(PathBuf::from("D:\\b c"));
        s.window = Some((10, 20, 1200, 800, true));
        s.save();
        let t = Settings::load(&file);
        assert_eq!(t, s);
    }
}
