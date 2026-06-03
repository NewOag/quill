//! Value transforms for the results detail panel.
//!
//! Each [`DetailFormat`] turns a raw cell string into a display string. All
//! transforms are pure and dependency-light (JSON via the already-present
//! `serde_json`, URL via `percent-encoding`; Base64 and timestamp are
//! hand-rolled). A transform that doesn't apply returns the original value with
//! a short note rather than panicking.

use std::ops::Range;

use percent_encoding::percent_decode_str;

/// How the detail panel renders the selected value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailFormat {
    /// Verbatim.
    Raw,
    /// Pretty-printed JSON.
    Json,
    /// Base64-decoded (shown as UTF-8, lossy).
    Base64,
    /// Percent-decoded URL text.
    Url,
    /// Unix timestamp (s or ms) → UTC datetime.
    Timestamp,
}

/// Apply a format to a raw value.
pub fn format_value(raw: &str, fmt: DetailFormat) -> String {
    match fmt {
        DetailFormat::Raw => raw.to_string(),
        DetailFormat::Json => pretty_json(raw),
        DetailFormat::Base64 => decode_base64(raw),
        DetailFormat::Url => percent_decode_str(raw)
            .decode_utf8()
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| format!("(invalid UTF-8 after URL decode)\n{raw}")),
        DetailFormat::Timestamp => format_timestamp(raw),
    }
}

fn pretty_json(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw.trim()) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()),
        Err(e) => format!("(not valid JSON: {e})\n\n{raw}"),
    }
}

/// Decode standard Base64 (with optional `=` padding). Hand-rolled to avoid a
/// dependency. Non-Base64 input returns a note + the original.
fn decode_base64(raw: &str) -> String {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = raw
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let mut out = Vec::with_capacity(cleaned.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for &b in &cleaned {
        let Some(v) = val(b) else {
            return format!("(not valid Base64)\n\n{raw}");
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Interpret an integer as a Unix timestamp (seconds, or milliseconds if it
/// looks too large) and format as `YYYY-MM-DD HH:MM:SS` UTC. Hand-rolled
/// civil-date conversion (no chrono).
fn format_timestamp(raw: &str) -> String {
    let t = raw.trim();
    let Ok(n) = t.parse::<i64>() else {
        return format!("(not an integer timestamp)\n\n{raw}");
    };
    // Heuristic: values past ~year 33658 in seconds are almost certainly ms.
    let secs = if n.abs() >= 1_000_000_000_000 { n / 1000 } else { n };
    if secs < 0 {
        return format!("(negative timestamp)\n\n{raw}");
    }
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// Days-since-epoch → civil date, via Howard Hinnant's algorithm.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32, h as u32, mi as u32, s as u32)
}

/// Token classes for JSON syntax highlighting in the detail panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonTok {
    Key,
    Str,
    Number,
    Keyword, // true / false / null
    Punct,   // { } [ ] : ,
    Plain,   // whitespace / anything else
}

/// Classify a (pretty-printed) JSON string into gap-free, byte-indexed spans.
/// A best-effort lexer: a `"…"` immediately followed by `:` is a Key, other
/// strings are Str. Covers `0..s.len()` with no gaps (unrecognized → Plain).
pub fn tokenize_json(s: &str) -> Vec<(Range<usize>, JsonTok)> {
    // Char-based scan: every range boundary is a valid UTF-8 char boundary, so
    // slicing `s[range]` can never split a multi-byte char (e.g. Chinese).
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let len = s.len();
    // byte offset just past char index k (or end of string).
    let end_of = |k: usize| chars.get(k).map(|&(b, _)| b).unwrap_or(len);
    let mut out = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        let (start, c) = chars[k];
        if c == '"' {
            k += 1;
            while k < chars.len() {
                let ch = chars[k].1;
                if ch == '\\' {
                    k += 2; // skip the escape and the escaped char
                    continue;
                }
                if ch == '"' {
                    k += 1;
                    break;
                }
                k += 1;
            }
            let str_end = end_of(k);
            // Look past whitespace for ':' → this string is an object key.
            let mut j = k;
            while j < chars.len() && chars[j].1.is_whitespace() {
                j += 1;
            }
            let kind = if chars.get(j).map(|&(_, c)| c) == Some(':') {
                JsonTok::Key
            } else {
                JsonTok::Str
            };
            out.push((start..str_end, kind));
        } else if c.is_ascii_digit()
            || (c == '-' && chars.get(k + 1).map(|&(_, c)| c.is_ascii_digit()) == Some(true))
        {
            k += 1;
            while k < chars.len() && matches!(chars[k].1, '0'..='9' | '.' | 'e' | 'E' | '+' | '-') {
                k += 1;
            }
            out.push((start..end_of(k), JsonTok::Number));
        } else if c.is_ascii_alphabetic() {
            let kstart = k;
            while k < chars.len() && chars[k].1.is_ascii_alphabetic() {
                k += 1;
            }
            let end = end_of(k);
            let kind = match &s[chars[kstart].0..end] {
                "true" | "false" | "null" => JsonTok::Keyword,
                _ => JsonTok::Plain,
            };
            out.push((start..end, kind));
        } else if matches!(c, '{' | '}' | '[' | ']' | ':' | ',') {
            k += 1;
            out.push((start..end_of(k), JsonTok::Punct));
        } else {
            k += 1;
            out.push((start..end_of(k), JsonTok::Plain));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_json_handles_multibyte_and_covers_all() {
        // Chinese inside a string must not split a char; spans must be gap-free
        // and every range must be a valid slice.
        let s = r#"{"role":"转至续租协议","n":12}"#;
        let toks = tokenize_json(s);
        let mut next = 0;
        for (r, _) in &toks {
            assert_eq!(r.start, next, "spans must be contiguous");
            let _ = &s[r.clone()]; // must not panic on a char boundary
            next = r.end;
        }
        assert_eq!(next, s.len(), "spans must cover the whole string");
    }

    #[test]
    fn raw_is_identity() {
        assert_eq!(format_value("hello", DetailFormat::Raw), "hello");
    }

    #[test]
    fn json_pretty_prints() {
        let out = format_value(r#"{"a":1,"b":[2,3]}"#, DetailFormat::Json);
        assert!(out.contains("\n"), "should be multi-line");
        assert!(out.contains("\"a\""));
    }

    #[test]
    fn json_invalid_notes_and_keeps_original() {
        let out = format_value("not json", DetailFormat::Json);
        assert!(out.contains("not valid JSON"));
        assert!(out.contains("not json"));
    }

    #[test]
    fn base64_decodes() {
        // "SGVsbG8=" -> "Hello"
        assert_eq!(format_value("SGVsbG8=", DetailFormat::Base64), "Hello");
    }

    #[test]
    fn url_decodes() {
        assert_eq!(
            format_value("a%20b%2Fc", DetailFormat::Url),
            "a b/c"
        );
    }

    #[test]
    fn timestamp_seconds() {
        // 1700000000 = 2023-11-14 22:13:20 UTC
        assert_eq!(
            format_value("1700000000", DetailFormat::Timestamp),
            "2023-11-14 22:13:20 UTC"
        );
    }

    #[test]
    fn timestamp_millis_detected() {
        // 1700000000000 ms == same instant as above.
        assert_eq!(
            format_value("1700000000000", DetailFormat::Timestamp),
            "2023-11-14 22:13:20 UTC"
        );
    }
}
