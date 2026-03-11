use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;

use crate::fact_type::FactType;
use crate::frontmatter::{FactRecord, RawFrontmatter};

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("{path}: {message}")]
    FieldError { path: PathBuf, message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct CompileWarning {
    pub path: PathBuf,
    pub message: String,
}

fn parse_datetime(
    value: &str,
    field_name: &str,
    source_path: &Path,
    warnings: &mut Vec<CompileWarning>,
) -> Result<DateTime<Utc>, ValidationError> {
    // Try parsing as RFC 3339 / ISO 8601 with timezone
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try parsing as ISO 8601 with timezone offset (e.g. "2024-01-15T10:30:00+05:00")
    if let Ok(dt) = DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%z") {
        return Ok(dt.with_timezone(&Utc));
    }

    // Check if it lacks a timezone specifier
    let has_tz = value.contains('Z') || value.contains('+') || {
        // Check for negative offset — but not the dash in the date portion
        // A timezone offset like -05:00 appears after the time portion
        let after_t = value.find('T').map(|i| &value[i..]);
        after_t.map_or(false, |s| s.contains('-'))
    };

    // Try parsing as naive datetime (no timezone)
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        if !has_tz {
            warnings.push(CompileWarning {
                path: source_path.to_path_buf(),
                message: format!(
                    "{field_name} has no timezone specifier, assuming UTC"
                ),
            });
        }
        return Ok(naive.and_utc());
    }

    // Try date-only format
    if let Ok(naive) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        if !has_tz {
            warnings.push(CompileWarning {
                path: source_path.to_path_buf(),
                message: format!(
                    "{field_name} has no timezone specifier, assuming UTC"
                ),
            });
        }
        return Ok(naive
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always valid")
            .and_utc());
    }

    Err(ValidationError::FieldError {
        path: source_path.to_path_buf(),
        message: format!("{field_name} '{value}' is not a valid ISO 8601 datetime"),
    })
}

fn derive_id_from_path(source_path: &Path) -> String {
    let path_str = source_path.to_string_lossy();
    let prefix = ".brv/context-tree/";
    if let Some(rest) = path_str.find(prefix).map(|i| &path_str[i + prefix.len()..]) {
        rest.strip_suffix(".md").unwrap_or(rest).to_string()
    } else {
        source_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

fn filter_empty(v: Option<Vec<String>>) -> Vec<String> {
    v.unwrap_or_default()
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect()
}

pub fn validate(
    raw: RawFrontmatter,
    source_path: &Path,
) -> Result<FactRecord, ValidationError> {
    let mut warnings: Vec<CompileWarning> = Vec::new();
    let path_display = source_path.to_path_buf();

    // --- Normalize and validate id ---
    let id = match raw.id {
        Some(ref v) => {
            let normalized = v.trim().to_lowercase();
            let id_pattern = regex_lite_match_id(&normalized);
            if !id_pattern {
                return Err(ValidationError::FieldError {
                    path: path_display,
                    message: format!(
                        "id '{}' contains invalid characters — must match [a-z0-9_/-]",
                        normalized
                    ),
                });
            }
            normalized
        }
        None => derive_id_from_path(source_path),
    };

    // --- Validate confidence ---
    let confidence = raw.confidence.unwrap_or(1.0);
    if !(0.0..=1.0).contains(&confidence) {
        return Err(ValidationError::FieldError {
            path: path_display,
            message: format!("confidence '{}' out of range [0.0, 1.0]", confidence),
        });
    }
    if confidence < 0.1 {
        warnings.push(CompileWarning {
            path: path_display.clone(),
            message: format!(
                "confidence {} is very low — consider removing this fact",
                confidence
            ),
        });
    }

    // --- Parse datetimes ---
    let created_at = match raw.created_at {
        Some(ref v) => Some(parse_datetime(v, "created_at", source_path, &mut warnings)?),
        None => None,
    };
    let updated_at = match raw.updated_at {
        Some(ref v) => Some(parse_datetime(v, "updated_at", source_path, &mut warnings)?),
        None => None,
    };
    let valid_until = match raw.valid_until {
        Some(ref v) => Some(parse_datetime(v, "valid_until", source_path, &mut warnings)?),
        None => None,
    };

    // --- Validate domain_tags ---
    let domain_tags: Vec<String> = filter_empty(raw.domain_tags)
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .collect();

    for tag in &domain_tags {
        if !tag.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' || c == ':') {
            return Err(ValidationError::FieldError {
                path: path_display,
                message: format!(
                    "domain_tags contains invalid tag '{}' — tags must match [a-z0-9_-:]",
                    tag
                ),
            });
        }
    }

    // --- Normalize fact_type and collect warnings ---
    let fact_type = raw.fact_type.unwrap_or_else(|| {
        warnings.push(CompileWarning {
            path: path_display.clone(),
            message: "fact_type not set, defaulting to 'durable'".to_string(),
        });
        FactType::Durable
    });

    if fact_type == FactType::Durable && valid_until.is_some() {
        warnings.push(CompileWarning {
            path: path_display.clone(),
            message: "durable facts should not expire — consider fact_type: state".to_string(),
        });
    }

    if fact_type == FactType::Event && raw.event_sequence.is_none() {
        warnings.push(CompileWarning {
            path: path_display.clone(),
            message: "event fact without event_sequence may produce non-deterministic ordering"
                .to_string(),
        });
    }

    if raw.event_sequence.is_some() && fact_type != FactType::Event {
        warnings.push(CompileWarning {
            path: path_display.clone(),
            message: "event_sequence set on non-event fact — consider fact_type: event".to_string(),
        });
    }

    // --- Validate caused_by self-reference ---
    let caused_by = filter_empty(raw.caused_by);
    if caused_by.iter().any(|entry| entry == &id) {
        return Err(ValidationError::FieldError {
            path: path_display,
            message: format!("caused_by references its own id '{}'", id),
        });
    }

    Ok(FactRecord {
        id,
        source_path: source_path.to_path_buf(),
        body: String::new(),
        title: raw.title,
        tags: filter_empty(raw.tags),
        keywords: filter_empty(raw.keywords),
        related: filter_empty(raw.related),
        importance: raw.importance.unwrap_or(1.0),
        recency: raw.recency.unwrap_or(1.0),
        maturity: raw.maturity.unwrap_or(1.0),
        access_count: raw.access_count.unwrap_or(0),
        update_count: raw.update_count.unwrap_or(0),
        created_at,
        updated_at,
        fact_type,
        valid_until,
        caused_by,
        causes: filter_empty(raw.causes),
        event_sequence: raw.event_sequence,
        confidence,
        domain_tags,
        warnings,
    })
}

fn regex_lite_match_id(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/' || c == '-')
}
