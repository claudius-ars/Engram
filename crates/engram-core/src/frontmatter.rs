use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fact_type::FactType;
use crate::validation::CompileWarning;

/// Direct serde target for YAML frontmatter parsing.
/// Every field is Option<T> so that missing fields (legacy files) do not cause parse errors.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RawFrontmatter {
    // Legacy ByteRover fields
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub keywords: Option<Vec<String>>,
    pub related: Option<Vec<String>>,
    pub importance: Option<f64>,
    pub recency: Option<f64>,
    pub maturity: Option<f64>,
    #[serde(alias = "accessCount")]
    pub access_count: Option<u64>,
    #[serde(alias = "updateCount")]
    pub update_count: Option<u64>,
    #[serde(alias = "createdAt")]
    pub created_at: Option<String>,
    #[serde(alias = "updatedAt")]
    pub updated_at: Option<String>,

    // Engram-native fields
    pub id: Option<String>,
    #[serde(alias = "factType")]
    pub fact_type: Option<FactType>,
    #[serde(alias = "validUntil")]
    pub valid_until: Option<String>,
    #[serde(alias = "causedBy")]
    pub caused_by: Option<Vec<String>>,
    pub causes: Option<Vec<String>>,
    #[serde(alias = "eventSequence")]
    pub event_sequence: Option<i64>,
    pub confidence: Option<f64>,
    #[serde(alias = "domainTags")]
    pub domain_tags: Option<Vec<String>>,
}

/// Validated, normalized struct produced by the validation step.
/// All fields have their final types with defaults applied.
#[derive(Debug, Clone, Serialize)]
pub struct FactRecord {
    // Identity
    pub id: String,
    pub source_path: PathBuf,

    // Legacy ByteRover fields (with defaults)
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
    pub related: Vec<String>,
    pub importance: f64,
    pub recency: f64,
    pub maturity: f64,
    pub access_count: u64,
    pub update_count: u64,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,

    // Engram-native fields (with defaults)
    pub fact_type: FactType,
    pub valid_until: Option<DateTime<Utc>>,
    pub caused_by: Vec<String>,
    pub causes: Vec<String>,
    pub event_sequence: Option<i64>,
    pub confidence: f64,
    pub domain_tags: Vec<String>,

    // Content
    pub body: String,

    // Compiler metadata
    pub warnings: Vec<CompileWarning>,

    /// True if frontmatter explicitly set `factType`. False if defaulted to durable.
    pub fact_type_explicit: bool,
}
