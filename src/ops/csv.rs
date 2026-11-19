// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! A small CSV reader for the `ops` inputs: a header row, comma separated, double quotes
//! around a field that holds a comma or a quote (`""` inside), blank lines and lines
//! starting with `#` skipped. Columns are looked up by header name, so extra columns are
//! allowed and order does not matter.

use super::err;
use kshana::api::KshanaError;

/// A parsed table.
#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub header: Vec<String>,
    /// Each row with its 1-based line number.
    pub rows: Vec<(usize, Vec<String>)>,
}

fn split(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => cur.push(c),
            (false, '"') if cur.trim().is_empty() => {
                cur.clear();
                quoted = true
            }
            (false, ',') => out.push(std::mem::take(&mut cur).trim().to_string()),
            (false, c) => cur.push(c),
        }
    }
    if quoted {
        return Err("unterminated quote".into());
    }
    out.push(cur.trim().to_string());
    Ok(out)
}

impl Table {
    /// Parse `text` (named `name` in errors).
    pub fn parse(name: &str, text: &str) -> Result<Table, KshanaError> {
        let mut header: Option<Vec<String>> = None;
        let mut rows = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim_start_matches('\u{feff}').trim_end_matches('\r');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let fields = split(line).map_err(|e| err(format!("{name} line {}: {e}", i + 1)))?;
            match &header {
                None => header = Some(fields),
                Some(h) => {
                    if fields.len() > h.len() {
                        return Err(err(format!(
                            "{name} line {}: {} fields, but the header has {}",
                            i + 1,
                            fields.len(),
                            h.len()
                        )));
                    }
                    rows.push((i + 1, fields));
                }
            }
        }
        let header = header.ok_or_else(|| err(format!("{name} is empty (no header row)")))?;
        Ok(Table {
            name: name.into(),
            header,
            rows,
        })
    }

    /// Index of a column, or an error naming the missing column.
    pub fn col(&self, name: &str) -> Result<usize, KshanaError> {
        self.opt_col(name).ok_or_else(|| {
            err(format!(
                "{} has no column '{name}' (header: {})",
                self.name,
                self.header.join(",")
            ))
        })
    }

    /// Index of a column, matched without regard to case.
    pub fn opt_col(&self, name: &str) -> Option<usize> {
        self.header
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))
    }

    /// The field at `col` of a row (empty when the row is short).
    pub fn get(row: &[String], col: usize) -> &str {
        row.get(col).map(String::as_str).unwrap_or("")
    }

    /// A number at `col`, or an error naming the file, line and column.
    pub fn num(&self, line: usize, row: &[String], col: usize) -> Result<f64, KshanaError> {
        let s = Self::get(row, col);
        s.parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| {
                err(format!(
                    "{} line {line}: {} '{s}' is not a number",
                    self.name, self.header[col]
                ))
            })
    }

    /// A UTC stamp at `col`, as Unix seconds.
    pub fn time(&self, line: usize, row: &[String], col: usize) -> Result<f64, KshanaError> {
        super::parse_utc(Self::get(row, col))
            .map_err(|e| err(format!("{} line {line}: {e}", self.name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quotes_comments_and_named_columns() {
        let t = Table::parse(
            "t.csv",
            "# comment\nA,b,c\n1,\"x, y\",\"say \"\"hi\"\"\"\n\n2,,\n",
        )
        .unwrap();
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].1, vec!["1", "x, y", "say \"hi\""]);
        assert_eq!(t.col("a").unwrap(), 0);
        assert!(t
            .col("d")
            .unwrap_err()
            .to_string()
            .contains("no column 'd'"));
        assert_eq!(t.num(3, &t.rows[0].1, 0).unwrap(), 1.0);
        assert!(t.num(3, &t.rows[0].1, 1).is_err());
        assert!(Table::parse("e.csv", "").is_err());
        assert!(Table::parse("e.csv", "a,b\n1,2,3\n").is_err());
        assert!(Table::parse("e.csv", "a\n\"open\n").is_err());
    }
}
