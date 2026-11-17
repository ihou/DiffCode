//! Small helpers shared across the core: Finder-like name ordering, path pieces, sizes.

use std::cmp::Ordering;

/// Orders names the way a file manager does: case-insensitively, with runs of digits compared as numbers
/// ("file2" before "file10"). The counterpart of Foundation's `localizedStandardCompare`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(&c) = ai.peek() {
                        if !c.is_ascii_digit() {
                            break;
                        }
                        na.push(c);
                        ai.next();
                    }
                    let mut nb = String::new();
                    while let Some(&c) = bi.peek() {
                        if !c.is_ascii_digit() {
                            break;
                        }
                        nb.push(c);
                        bi.next();
                    }
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    continue;
                }
                let la = fold(ca);
                let lb = fold(cb);
                if la != lb {
                    return la.cmp(&lb);
                }
                ai.next();
                bi.next();
            }
        }
    }
    // Equal ignoring case and leading zeros: fall back to the exact text so the order is total.
    a.cmp(b)
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The last component of a slash-separated repository path. A slash at the end is not part of the name: git
/// gives a folder it does not look into (a repository inside this one) as "docs/handbook/".
pub fn file_name(path: &str) -> &str {
    let path = path.strip_suffix('/').unwrap_or(path);
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Everything before the last slash of a repository path ("" for a top-level file), a slash at the end apart.
pub fn parent_dir(path: &str) -> &str {
    let path = path.strip_suffix('/').unwrap_or(path);
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// The extension of a file name, without the dot, lower-cased ("" when there is none).
pub fn extension(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// "1.2 MB"-style size, in the decimal units file managers use.
pub fn byte_size_string(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return if bytes == 1 { "1 byte".to_string() } else { format!("{bytes} bytes") };
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 || unit == 1 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// Splits on NUL, dropping empty pieces, as UTF-8 (lossily).
pub fn split_nul(data: &[u8]) -> Vec<String> {
    data.split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Splits on NUL, keeping empty pieces.
pub fn split_nul_keep_empty(data: &[u8]) -> Vec<String> {
    data.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// `str::splitn`, but collecting owned pieces. Mirrors Swift's `split(separator:maxSplits:omittingEmptySubsequences: false)`.
pub fn split_max(s: &str, sep: char, max_splits: usize) -> Vec<String> {
    s.splitn(max_splits + 1, sep).map(str::to_string).collect()
}

/// A moment in local time, broken down: (year, month 1–12, day 1–31, hour, minute).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// `secs` since 1970 in the user's time zone.
#[cfg(windows)]
pub fn local_time(secs: i64) -> LocalTime {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // 100-nanosecond intervals since 1601.
    let ticks = (secs + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            local = utc;
        }
    }
    LocalTime {
        year: local.wYear as i32,
        month: local.wMonth as u32,
        day: local.wDay as u32,
        hour: local.wHour as u32,
        minute: local.wMinute as u32,
    }
}

/// `secs` since 1970 in the user's time zone.
#[cfg(unix)]
pub fn local_time(secs: i64) -> LocalTime {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
    }
}

/// "2026-11-13", in local time.
pub fn date_text(secs: i64) -> String {
    let t = local_time(secs);
    format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
}

/// "2026-11-13 14:05", in local time.
pub fn date_time_text(secs: i64) -> String {
    let t = local_time(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
}

/// How long ago, abbreviated: "5 min. ago", "3 hr. ago", "2 days ago" (as the Mac app's relative dates).
pub fn relative_text(secs: i64) -> String {
    let d = (now_secs() - secs).max(0);
    let (n, en, zh) = match d {
        0..=59 => return crate::lang::l("now", "刚刚"),
        60..=3599 => (d / 60, "min.", "分钟"),
        3600..=86_399 => (d / 3600, "hr.", "小时"),
        86_400..=604_799 => (d / 86_400, if d / 86_400 == 1 { "day" } else { "days" }, "天"),
        604_800..=2_629_799 => (d / 604_800, "wk.", "周"),
        2_629_800..=31_557_599 => (d / 2_629_800, "mo.", "个月"),
        _ => (d / 31_557_600, "yr.", "年"),
    };
    if crate::lang::is_chinese() {
        format!("{n}{zh}前")
    } else {
        format!("{n} {en} ago")
    }
}

/// Seconds since 1970, now.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut names = vec!["file10.txt", "File2.txt", "file1.txt", "alpha", "Beta"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["alpha", "Beta", "file1.txt", "File2.txt", "file10.txt"]);
    }

    #[test]
    fn path_pieces() {
        assert_eq!(file_name("a/b/c.kt"), "c.kt");
        // A repository inside this one, as git lists it.
        assert_eq!(file_name("docs/handbook/"), "handbook");
        assert_eq!(parent_dir("docs/handbook/"), "docs");
        assert_eq!(file_name("handbook/"), "handbook");
        assert_eq!(parent_dir("handbook/"), "");
        assert_eq!(parent_dir("a/b/c.kt"), "a/b");
        assert_eq!(parent_dir("c.kt"), "");
        assert_eq!(extension("a/B.KT"), "kt");
        assert_eq!(extension(".gitignore"), "");
    }

    #[test]
    fn sizes() {
        assert_eq!(byte_size_string(512), "512 bytes");
        assert_eq!(byte_size_string(4_163_009), "4.2 MB");
        assert_eq!(byte_size_string(12_000), "12 KB");
    }
}
