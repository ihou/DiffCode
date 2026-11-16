//! Decoding and line-splitting for file contents, and lossless write-back of edited text.

use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf16LittleEndian,
    Utf16BigEndian,
    Gb18030,
}

/// How a file's text sits on disk, so that an edited file can be written back byte for byte the
/// same everywhere it was not edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextFormat {
    pub encoding: Encoding,
    pub bom: bool,
    /// Ends the lines that editing adds: whichever the file uses most.
    pub newline: &'static str,
    /// The last line ends with a line break.
    pub final_newline: bool,
}

impl Default for TextFormat {
    fn default() -> Self {
        TextFormat { encoding: Encoding::Utf8, bom: false, newline: "\n", final_newline: false }
    }
}

/// A file's text split into lines exactly as `split_lines` splits it, plus what it takes to write it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditableText {
    pub lines: Vec<String>,
    pub format: TextFormat,
    /// Per line, whether it ends in "\r\n". Kept only for files that mix line endings; None when all end alike.
    pub crlf: Option<Vec<bool>>,
}

/// Git's own heuristic: a NUL byte in the first 8000 bytes means binary.
pub fn looks_binary(data: &[u8]) -> bool {
    if data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]) {
        return false; // UTF-16 BOM
    }
    data[..data.len().min(8000)].contains(&0)
}

fn utf16_decode(data: &[u8], little: bool) -> Option<String> {
    if data.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
        .collect();
    String::from_utf16(&units).ok()
}

fn gb18030_decode(data: &[u8]) -> Option<String> {
    encoding_rs::GB18030
        .decode_without_bom_handling_and_without_replacement(data)
        .map(|c| c.into_owned())
}

/// Best-effort text for display: BOMs honoured, UTF-8 first, then GB18030, then lossy UTF-8.
pub fn decode(data: &[u8]) -> String {
    if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&data[3..]).into_owned();
    }
    if data.starts_with(&[0xFF, 0xFE]) {
        if let Some(s) = utf16_decode(&data[2..], true) {
            return s;
        }
    }
    if data.starts_with(&[0xFE, 0xFF]) {
        if let Some(s) = utf16_decode(&data[2..], false) {
            return s;
        }
    }
    if let Ok(s) = std::str::from_utf8(data) {
        return s.to_string();
    }
    if let Some(s) = gb18030_decode(data) {
        return s;
    }
    String::from_utf8_lossy(data).into_owned()
}

/// Decodes a file for editing. None unless writing the result back with `encode` gives exactly `data`:
/// bytes no encoding accepts, a carriage return that does not end a line and the like keep a file read-only.
pub fn decode_for_editing(data: &[u8]) -> Option<EditableText> {
    let mut format = TextFormat::default();
    let text: Option<String> = if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        format.bom = true;
        std::str::from_utf8(&data[3..]).ok().map(str::to_string)
    } else if data.starts_with(&[0xFF, 0xFE]) {
        format.bom = true;
        format.encoding = Encoding::Utf16LittleEndian;
        utf16_decode(&data[2..], true)
    } else if data.starts_with(&[0xFE, 0xFF]) {
        format.bom = true;
        format.encoding = Encoding::Utf16BigEndian;
        utf16_decode(&data[2..], false)
    } else if let Ok(s) = std::str::from_utf8(data) {
        Some(s.to_string())
    } else {
        format.encoding = Encoding::Gb18030;
        gb18030_decode(data)
    };
    let text = text?;
    let buf = text.as_bytes();

    let mut lines: Vec<String> = Vec::with_capacity(buf.len() / 32 + 1);
    let mut ends_in_crlf: Vec<bool> = Vec::with_capacity(buf.len() / 32 + 1);
    let (mut crlf_count, mut lf_count) = (0usize, 0usize);
    let mut stray = false;
    let mut start = 0usize;
    for i in 0..buf.len() {
        let byte = buf[i];
        if byte == 0x0D {
            // Only as half of "\r\n": a lone one is a line break git does not see, or a character no editor shows.
            if i + 1 >= buf.len() || buf[i + 1] != 0x0A {
                stray = true;
            }
            continue;
        }
        if byte != 0x0A {
            continue;
        }
        let crlf = i > start && buf[i - 1] == 0x0D;
        if crlf {
            crlf_count += 1;
        } else {
            lf_count += 1;
        }
        let end = if crlf { i - 1 } else { i };
        lines.push(text[start..end].to_string());
        ends_in_crlf.push(crlf);
        start = i + 1;
    }
    if start < buf.len() {
        lines.push(text[start..].to_string());
        ends_in_crlf.push(false);
    }
    format.final_newline = !buf.is_empty() && start == buf.len();
    if stray {
        return None;
    }
    // An empty file still has a line to type on.
    if lines.is_empty() {
        lines.push(String::new());
        ends_in_crlf.push(false);
    }
    format.newline = if crlf_count > lf_count { "\r\n" } else { "\n" };
    let crlf = if crlf_count > 0 && lf_count > 0 { Some(ends_in_crlf) } else { None };
    let result = EditableText { lines, format, crlf };
    if encode(&result.lines, &result.format, result.crlf.as_deref()).as_deref() != Some(data) {
        return None;
    }
    Some(result)
}

/// The bytes for `lines` in `format`. None only if the text cannot be represented in the file's encoding.
pub fn encode<S: AsRef<str>>(lines: &[S], format: &TextFormat, crlf: Option<&[bool]>) -> Option<Vec<u8>> {
    let mut text = String::with_capacity(lines.iter().map(|l| l.as_ref().len() + 2).sum());
    let last = lines.len().saturating_sub(1);
    for (index, line) in lines.iter().enumerate() {
        text.push_str(line.as_ref());
        if index < last || format.final_newline {
            match crlf {
                Some(flags) if index < flags.len() => text.push_str(if flags[index] { "\r\n" } else { "\n" }),
                _ => text.push_str(format.newline),
            }
        }
    }
    let mut data = Vec::with_capacity(text.len() + 3);
    match format.encoding {
        Encoding::Utf8 => {
            if format.bom {
                data.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            data.extend_from_slice(text.as_bytes());
        }
        Encoding::Utf16LittleEndian => {
            if format.bom {
                data.extend_from_slice(&[0xFF, 0xFE]);
            }
            for unit in text.encode_utf16() {
                data.extend_from_slice(&unit.to_le_bytes());
            }
        }
        Encoding::Utf16BigEndian => {
            if format.bom {
                data.extend_from_slice(&[0xFE, 0xFF]);
            }
            for unit in text.encode_utf16() {
                data.extend_from_slice(&unit.to_be_bytes());
            }
        }
        Encoding::Gb18030 => {
            let (bytes, _, had_errors) = encoding_rs::GB18030.encode(&text);
            if had_errors {
                return None;
            }
            data.extend_from_slice(&bytes);
        }
    }
    Some(data)
}

/// Identifies a file's contents, to tell whether something else rewrote it.
pub fn fingerprint(data: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Splits on `\n`, dropping a trailing `\r` from each line. A final newline does not
/// produce an extra empty line, matching how git counts lines.
pub fn split_lines(text: &str) -> Vec<String> {
    let buf = text.as_bytes();
    let mut lines = Vec::with_capacity(buf.len() / 32 + 1);
    if buf.is_empty() {
        return lines;
    }
    let mut start = 0;
    for i in 0..buf.len() {
        if buf[i] == b'\n' {
            let mut end = i;
            if end > start && buf[end - 1] == b'\r' {
                end -= 1;
            }
            lines.push(text[start..end].to_string());
            start = i + 1;
        }
    }
    if start < buf.len() {
        let mut end = buf.len();
        if end > start && buf[end - 1] == b'\r' {
            end -= 1;
        }
        lines.push(text[start..end].to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_detection() {
        assert!(looks_binary(b"ab\0cd"));
        assert!(!looks_binary(b"plain text"));
        assert!(!looks_binary(&[0xFF, 0xFE, b'a', 0]));
    }

    #[test]
    fn split_matches_git() {
        assert_eq!(split_lines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
        assert!(split_lines("").is_empty());
        assert_eq!(split_lines("\n"), vec![""]);
    }

    #[test]
    fn lossless_round_trips() {
        let samples: Vec<Vec<u8>> = vec![
            b"one\ntwo\n".to_vec(),
            b"one\r\ntwo\r\n".to_vec(),
            b"mixed\r\nends\nhere".to_vec(),
            b"\xEF\xBB\xBFbom\n".to_vec(),
            vec![],
        ];
        for data in samples {
            let text = decode_for_editing(&data).expect("decodes");
            assert_eq!(encode(&text.lines, &text.format, text.crlf.as_deref()).unwrap(), data);
        }
    }

    #[test]
    fn mixed_endings_are_kept_per_line() {
        let text = decode_for_editing(b"a\r\nb\nc\r\n").unwrap();
        assert_eq!(text.crlf, Some(vec![true, false, true]));
        assert_eq!(text.format.newline, "\r\n");
    }

    #[test]
    fn stray_carriage_return_is_read_only() {
        assert!(decode_for_editing(b"a\rb\n").is_none());
    }

    #[test]
    fn utf16_and_gb18030() {
        let mut le = vec![0xFF, 0xFE];
        for u in "hi\n中".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        let text = decode_for_editing(&le).unwrap();
        assert_eq!(text.format.encoding, Encoding::Utf16LittleEndian);
        assert_eq!(text.lines, vec!["hi", "中"]);
        assert_eq!(encode(&text.lines, &text.format, None).unwrap(), le);

        let (gb, _, _) = encoding_rs::GB18030.encode("中文\n");
        let text = decode_for_editing(&gb).unwrap();
        assert_eq!(text.format.encoding, Encoding::Gb18030);
        assert_eq!(text.lines, vec!["中文"]);
        assert_eq!(decode(&gb), "中文\n");
    }
}
