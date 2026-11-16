//! Looking for a newer DiffCode on Windows, where nothing updates it: GitHub's latest release is checked at launch
//! and once a day while DiffCode stays open, and offered in the sidebar, a toast and a dialog whose Download opens
//! the release page. (The Mac app updates itself with Sparkle.) Only a release that already carries the Windows
//! installer counts, so nobody is sent to a page without it while the release is still being built.

use std::time::{Duration, Instant};

use super::dialog::{DialogRequest, DialogStyle};
use super::{App, PlatformRequest};
use crate::core::lang::l;
use crate::core::lf;
use crate::icons::Icon;
use crate::platform::HttpGet;

pub const LATEST: &str = "https://api.github.com/repos/ihou/DiffCode/releases/latest";
const RELEASE_PAGE: &str = "https://github.com/ihou/DiffCode/releases/tag/";
const INSTALLER: &str = "DiffCode-Setup-x64.exe";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The version running.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Without the tag's leading "v".
    pub version: String,
    pub page: String,
}

#[derive(Default)]
pub struct Updates {
    http: Option<HttpGet>,
    /// A release newer than this one, from the last check.
    pub available: Option<Release>,
    pub checking: bool,
    /// How the last check ended: Ok, or why it could not be done.
    pub last: Option<Result<(), String>>,
    /// The check under way was asked for (Settings, Find Action), so it answers even when there is nothing new.
    manual: bool,
    next: Option<Instant>,
}

/// The latest release from GitHub's JSON for it: None when it does not carry the Windows installer yet.
pub fn parse_release(json: &str) -> Result<Option<Release>, String> {
    let tag = string_field(json, "tag_name").ok_or_else(|| l("GitHub sent an unexpected reply", "GitHub 返回了意外的内容"))?;
    if !json.contains(&format!("\"{INSTALLER}\"")) {
        return Ok(None);
    }
    Ok(Some(Release { version: tag.trim_start_matches('v').to_string(), page: format!("{RELEASE_PAGE}{tag}") }))
}

/// The value of the first `"name": "value"` in `json` (no escapes are expected in the fields read).
fn string_field(json: &str, name: &str) -> Option<String> {
    let rest = &json[json.find(&format!("\"{name}\""))? + name.len() + 2..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// Whether version `a` comes after `b`, comparing the numbers of "major.minor.patch" (a suffix such as "-beta" is
/// ignored).
pub fn is_newer(a: &str, b: &str) -> bool {
    let numbers = |v: &str| -> Vec<u64> {
        let core = v.split(['-', '+']).next().unwrap_or("");
        core.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
    };
    let (mut x, mut y) = (numbers(a), numbers(b));
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    x > y
}

impl App {
    /// Where the platform can fetch from the web: Windows. The harness and the Mac never check.
    pub fn updates_supported(&self) -> bool {
        self.update.http.is_some()
    }

    pub(super) fn start_updates(&mut self, http: Option<HttpGet>) {
        self.update.http = http;
        if self.settings.check_updates {
            self.check_for_updates(false);
        }
    }

    /// The daily check while DiffCode stays open; run every frame, cheap until it is due.
    pub(super) fn tick_updates(&mut self) {
        if self.settings.check_updates && self.update.next.is_some_and(|t| Instant::now() >= t) {
            self.check_for_updates(false);
        }
    }

    /// Looks for a newer release. `manual`: asked for, so the answer shows whatever it is.
    pub fn check_for_updates(&mut self, manual: bool) {
        let Some(http) = self.update.http.clone() else {
            if manual {
                self.show_toast(l("This copy of DiffCode does not check for updates", "这个 DiffCode 不检查更新"));
            }
            return;
        };
        self.update.manual |= manual;
        if self.update.checking {
            return;
        }
        self.update.checking = true;
        self.update.next = None;
        self.jobs.spawn(move || http(LATEST).and_then(|json| parse_release(&json)), |app, result| app.finished_update_check(result));
    }

    fn finished_update_check(&mut self, result: Result<Option<Release>, String>) {
        self.update.checking = false;
        self.update.next = Some(Instant::now() + DAY);
        let manual = std::mem::take(&mut self.update.manual);
        match result {
            Ok(Some(release)) if is_newer(&release.version, CURRENT) => {
                let news = self.update.available.as_ref() != Some(&release);
                let skipped = self.settings.skipped_update.as_deref() == Some(release.version.as_str());
                self.update.available = Some(release.clone());
                self.update.last = Some(Ok(()));
                if manual {
                    self.show_update_dialog();
                } else if news && !skipped {
                    let v = release.version;
                    self.show_toast_action(lf!("DiffCode {v} is available", "DiffCode {v} 已发布"), l("Details", "查看"), |app| app.show_update_dialog());
                }
            }
            Ok(_) => {
                self.update.available = None;
                self.update.last = Some(Ok(()));
                if manual {
                    self.show_toast(lf!("DiffCode {CURRENT} is up to date", "DiffCode {CURRENT} 已是最新版本"));
                }
            }
            Err(e) => {
                if manual {
                    self.show_toast(lf!("Couldn't check for updates: {e}", "检查更新失败：{e}"));
                }
                self.update.last = Some(Err(e));
            }
        }
    }

    /// The newer release, unless it is the one the user chose to skip.
    pub fn update_offered(&self) -> Option<&Release> {
        self.update.available.as_ref().filter(|r| self.settings.skipped_update.as_deref() != Some(r.version.as_str()))
    }

    /// Download (opens the release page), Later, or Skip This Version.
    pub fn show_update_dialog(&mut self) {
        let Some(release) = self.update.available.clone() else { return };
        let (v, page) = (release.version.clone(), release.page.clone());
        let mut request = DialogRequest::new(DialogStyle::Info, Icon::ArrowDownload, lf!("DiffCode {v} is available", "DiffCode {v} 已发布"), l("Download", "下载"))
            .message(lf!(
                "You have {CURRENT}. The download page has the installer and what is new; installing it keeps your projects and settings.",
                "当前版本是 {CURRENT}。下载页上有安装包和更新说明；安装新版本会保留你的项目和设置。"
            ))
            .confirm(move |app| app.requests.push(PlatformRequest::OpenUrl(page)));
        request.cancel_title = Some(l("Later", "稍后"));
        request.alternate_title = Some(l("Skip This Version", "跳过此版本"));
        request.on_alternate = Some(Box::new(move |app: &mut App| {
            app.settings.skipped_update = Some(v);
            app.settings.save();
        }));
        self.ask(request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.3.1", "0.3.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.3.1"));
        assert!(!is_newer("0.3.1-beta", "0.3.1"));
    }

    #[test]
    fn reads_the_release() {
        let json = r#"{"url":"x","html_url":"https://github.com/ihou/DiffCode/releases/tag/v0.3.1","tag_name" : "v0.3.1",
            "author":{"html_url":"https://github.com/someone"},
            "assets":[{"name":"DiffCode.pkg"},{"name": "DiffCode-Setup-x64.exe","browser_download_url":"u"}]}"#;
        let r = parse_release(json).unwrap().unwrap();
        assert_eq!(r.version, "0.3.1");
        assert_eq!(r.page, "https://github.com/ihou/DiffCode/releases/tag/v0.3.1");
        // Published, but the Windows build is not attached yet.
        assert_eq!(parse_release(r#"{"tag_name":"v0.3.2","assets":[{"name":"DiffCode.pkg"}]}"#), Ok(None));
        assert!(parse_release(r#"{"message":"API rate limit exceeded"}"#).is_err());
    }
}
