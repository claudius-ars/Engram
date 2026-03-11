use std::path::{Path, PathBuf};

use engram_core::{validate, CompileWarning, FactRecord, RawFrontmatter};
use rayon::prelude::*;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("{path}: {message}")]
    FrontmatterError { path: PathBuf, message: String },

    #[error("{path}: io error: {source}")]
    IoError {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub struct ParseResult {
    pub records: Vec<FactRecord>,
    pub errors: Vec<ParseError>,
    pub warnings: Vec<CompileWarning>,
    pub file_count: usize,
    pub error_count: usize,
}

impl ParseResult {
    pub fn empty() -> Self {
        ParseResult {
            records: vec![],
            errors: vec![],
            warnings: vec![],
            file_count: 0,
            error_count: 0,
        }
    }
}

/// Splits a raw file string into (frontmatter_yaml, body).
pub fn extract_frontmatter(content: &str) -> (Option<&str>, &str) {
    // Must start with "---\n" or be exactly "---"
    if !content.starts_with("---\n") && content != "---" {
        return (None, content);
    }

    // Find the closing "---" after the opening one
    let after_open = &content[4..]; // skip "---\n"
    if let Some(close_pos) = find_closing_delimiter(after_open) {
        let yaml = &after_open[..close_pos];
        let after_close = &after_open[close_pos + 3..]; // skip "---"
        // Skip the newline right after closing ---
        let body = if after_close.starts_with('\n') {
            &after_close[1..]
        } else {
            after_close
        };
        let body = body.trim_start_matches('\n');
        (Some(yaml), body)
    } else {
        // No closing delimiter found — treat entire content as body
        (None, content)
    }
}

/// Find "---" at the start of a line within the content.
fn find_closing_delimiter(content: &str) -> Option<usize> {
    // Check if content starts with "---"
    if content.starts_with("---") && (content.len() == 3 || content.as_bytes().get(3) == Some(&b'\n'))
    {
        return Some(0);
    }
    // Search for "\n---" followed by newline or end of string
    let mut search_from = 0;
    while let Some(pos) = content[search_from..].find("\n---") {
        let abs_pos = search_from + pos + 1; // position of "---"
        let after = abs_pos + 3;
        if after >= content.len() || content.as_bytes()[after] == b'\n' {
            return Some(abs_pos);
        }
        search_from = abs_pos + 3;
    }
    None
}

/// Reads the file at path, extracts frontmatter, parses YAML, validates, and
/// returns the FactRecord.
pub fn parse_file(path: &Path) -> Result<FactRecord, ParseError> {
    let content = std::fs::read_to_string(path).map_err(|e| ParseError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;

    let (yaml_str, body) = extract_frontmatter(&content);

    match yaml_str {
        Some(yaml) => {
            let raw: RawFrontmatter =
                serde_yaml::from_str(yaml).map_err(|e| ParseError::FrontmatterError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;

            let mut record =
                validate(raw, path).map_err(|e| ParseError::FrontmatterError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;
            record.body = body.to_string();
            Ok(record)
        }
        None => {
            // No frontmatter — use defaults and add a warning
            let raw = RawFrontmatter::default();
            let mut record = validate(raw, path).map_err(|e| ParseError::FrontmatterError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
            record.body = body.to_string();
            // Prepend the "no frontmatter" warning before the "fact_type not set" warning
            record.warnings.insert(
                0,
                CompileWarning {
                    path: path.to_path_buf(),
                    message: "no frontmatter found, all fields defaulted".to_string(),
                },
            );
            Ok(record)
        }
    }
}

/// Calls parse_file on every path in parallel using rayon.
pub fn parse_all(paths: Vec<PathBuf>) -> ParseResult {
    let file_count = paths.len();

    let results: Vec<Result<FactRecord, ParseError>> =
        paths.into_par_iter().map(|p| parse_file(&p)).collect();

    let mut records = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    for result in results {
        match result {
            Ok(record) => {
                warnings.extend(record.warnings.clone());
                records.push(record);
            }
            Err(e) => {
                errors.push(e);
            }
        }
    }

    let error_count = errors.len();

    ParseResult {
        records,
        errors,
        warnings,
        file_count,
        error_count,
    }
}
