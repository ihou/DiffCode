//! The interface language: English or Simplified Chinese, chosen once at launch.

use std::sync::atomic::{AtomicBool, Ordering};

static CHINESE: AtomicBool = AtomicBool::new(false);

/// Set first thing at launch (from the user's setting or the system language), before any text is made.
pub fn set_chinese(chinese: bool) {
    CHINESE.store(chinese, Ordering::Relaxed);
}

pub fn is_chinese() -> bool {
    CHINESE.load(Ordering::Relaxed)
}

/// The English or the Chinese text, whichever the interface shows.
pub fn l(en: &str, zh: &str) -> String {
    if is_chinese() { zh.to_string() } else { en.to_string() }
}

/// `l` with `format!` arguments captured from the call site: `lf!("{n} files", "{n} 个文件")`.
#[macro_export]
macro_rules! lf {
    ($en:literal, $zh:literal) => {
        if $crate::lang::is_chinese() { format!($zh) } else { format!($en) }
    };
}
