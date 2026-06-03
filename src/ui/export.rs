//! Export helpers for the results table.
//!
//! Pure functions, no I/O — callers decide whether to write to a file or put
//! the result on the clipboard. Both formats are tested independently.

use crate::datasource::QueryResult;

/// Render a [`QueryResult`] as CSV. Fields are quoted if they contain a comma,
/// double-quote, or newline; embedded double-quotes are doubled. NULL cells
/// become empty fields (two consecutive commas / nothing between delimiters),
/// consistent with how most tools import CSVs.
pub fn to_csv(r: &QueryResult) -> String {
    let mut out = String::new();

    // Header row.
    for (i, col) in r.columns.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        csv_field(&col.name, &mut out);
    }
    out.push('\n');

    // Data rows.
    for row in &r.rows {
        for (i, cell) in row.cells.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            match cell {
                None => {}
                Some(v) => csv_field(v, &mut out),
            }
        }
        out.push('\n');
    }
    out
}

fn csv_field(s: &str, out: &mut String) {
    if s.contains([',', '"', '\n', '\r']) {
        out.push('"');
        for ch in s.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(s);
    }
}

/// Render a [`QueryResult`] as a pretty-printed JSON array.
/// NULL cells become JSON `null`; all other values are JSON strings.
pub fn to_json(r: &QueryResult) -> String {
    let rows: Vec<serde_json::Value> = r
        .rows
        .iter()
        .map(|row| {
            let obj: serde_json::Map<String, serde_json::Value> = r
                .columns
                .iter()
                .zip(row.cells.iter())
                .map(|(col, cell)| {
                    let val = match cell {
                        None => serde_json::Value::Null,
                        Some(v) => serde_json::Value::String(v.clone()),
                    };
                    (col.name.clone(), val)
                })
                .collect();
            serde_json::Value::Object(obj)
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasource::{Column, Row};

    fn sample() -> QueryResult {
        QueryResult {
            columns: vec![
                Column { name: "id".into(), type_name: "INT".into() },
                Column { name: "name".into(), type_name: "VARCHAR".into() },
                Column { name: "note".into(), type_name: "TEXT".into() },
            ],
            rows: vec![
                Row { cells: vec![Some("1".into()), Some("Alice".into()), None] },
                Row {
                    cells: vec![
                        Some("2".into()),
                        Some("Bob, \"the\" guy".into()),
                        Some("ok".into()),
                    ],
                },
            ],
        }
    }

    #[test]
    fn csv_header_and_null() {
        let csv = to_csv(&sample());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "id,name,note");
        // Row 1: id=1, name=Alice, note=NULL → empty field
        assert_eq!(lines[1], "1,Alice,");
    }

    #[test]
    fn csv_quotes_special_chars() {
        let csv = to_csv(&sample());
        let lines: Vec<&str> = csv.lines().collect();
        // Row 2: name has comma and quotes → quoted with doubled inner quotes
        assert!(lines[2].contains(r#""Bob, ""the"" guy""#));
    }

    #[test]
    fn json_null_and_string() {
        let json = to_json(&sample());
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0]["id"], "1");
        assert_eq!(arr[0]["note"], serde_json::Value::Null);
        assert_eq!(arr[1]["name"], "Bob, \"the\" guy");
    }

    #[test]
    fn json_is_valid_for_empty_result() {
        let empty = QueryResult::default();
        let json = to_json(&empty);
        assert_eq!(json.trim(), "[]");
    }
}
