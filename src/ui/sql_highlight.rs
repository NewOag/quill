//! A tiny hand-rolled SQL tokenizer for syntax highlighting in the editor.
//!
//! [`tokenize_sql`] returns **gap-free, byte-indexed** spans covering the whole
//! input — anything unrecognized is emitted as [`TokenKind::Plain`] — so the
//! caller can build one `gpui::TextRun` per token with `sum(len) == s.len()`,
//! which `shape_line` requires.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Str,
    Number,
    Comment,
    Punct,
    Plain,
}

/// Common SQL keywords (compared case-insensitively, uppercased).
const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "FULL", "CROSS", "ON",
    "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "INSERT", "INTO", "VALUES", "UPDATE",
    "SET", "DELETE", "CREATE", "TABLE", "VIEW", "INDEX", "DROP", "ALTER", "ADD", "AND", "OR",
    "NOT", "NULL", "AS", "IN", "IS", "LIKE", "BETWEEN", "DISTINCT", "COUNT", "SUM", "AVG", "MIN",
    "MAX", "ASC", "DESC", "UNION", "ALL", "CASE", "WHEN", "THEN", "ELSE", "END", "EXISTS", "INNER",
    "USING", "DEFAULT", "PRIMARY", "KEY", "FOREIGN", "REFERENCES", "TRUE", "FALSE",
];

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Tokenize `s` into contiguous byte spans. Spans are in order and cover
/// `0..s.len()` with no gaps.
pub fn tokenize_sql(s: &str) -> Vec<(Range<usize>, TokenKind)> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;

    while i < len {
        let c = bytes[i] as char;

        // Line comment: -- ... to end of line.
        if c == '-' && i + 1 < len && bytes[i + 1] == b'-' {
            let start = i;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            out.push((start..i, TokenKind::Comment));
            continue;
        }

        // String literal: '...'. Unterminated runs to EOF.
        if c == '\'' {
            let start = i;
            i += 1;
            while i < len && bytes[i] != b'\'' {
                i += 1;
            }
            if i < len {
                i += 1; // include closing quote
            }
            out.push((start..i, TokenKind::Str));
            continue;
        }

        // Number: digits with optional single dot.
        if c.is_ascii_digit() {
            let start = i;
            let mut seen_dot = false;
            while i < len {
                let d = bytes[i] as char;
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            out.push((start..i, TokenKind::Number));
            continue;
        }

        // Identifier / keyword.
        if is_ident_start(c) {
            let start = i;
            while i < len && is_ident_continue(bytes[i] as char) {
                i += 1;
            }
            let word = &s[start..i];
            let kind = if KEYWORDS.contains(&word.to_ascii_uppercase().as_str()) {
                TokenKind::Keyword
            } else {
                TokenKind::Plain
            };
            out.push((start..i, kind));
            continue;
        }

        // Punctuation.
        if matches!(
            c,
            '(' | ')' | ',' | ';' | '.' | '*' | '=' | '<' | '>' | '+' | '-' | '/' | '%' | '!'
        ) {
            out.push((i..i + 1, TokenKind::Punct));
            i += 1;
            continue;
        }

        // Anything else (whitespace, backticks, unicode) → Plain, advancing by
        // the full char width so we never split a multi-byte char.
        let ch_len = c.len_utf8();
        out.push((i..i + ch_len, TokenKind::Plain));
        i += ch_len;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers(s: &str, toks: &[(Range<usize>, TokenKind)]) -> bool {
        let mut next = 0;
        for (r, _) in toks {
            if r.start != next {
                return false;
            }
            next = r.end;
        }
        next == s.len()
    }

    #[test]
    fn spans_are_gap_free_and_cover_all() {
        let s = "SELECT id, 'x' FROM t -- note\nWHERE n = 12.5";
        let toks = tokenize_sql(s);
        assert!(covers(s, &toks), "spans must cover the whole string");
    }

    #[test]
    fn classifies_basic_tokens() {
        let toks = tokenize_sql("SELECT 1 'a' -- c");
        let kinds: Vec<_> = toks.iter().map(|(_, k)| *k).collect();
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Number));
        assert!(kinds.contains(&TokenKind::Str));
        assert!(kinds.contains(&TokenKind::Comment));
    }

    #[test]
    fn lowercase_keyword_detected() {
        let toks = tokenize_sql("select");
        assert_eq!(toks[0].1, TokenKind::Keyword);
    }

    #[test]
    fn multibyte_is_not_split() {
        let s = "名字 = '值'";
        let toks = tokenize_sql(s);
        assert!(covers(s, &toks));
    }
}
