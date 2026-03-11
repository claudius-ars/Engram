use std::path::{Path, PathBuf};

use engram_core::{FactRecord, FactType};
use tantivy::schema::*;
use tantivy::{doc, Index};

/// Sentinel value for null timestamps and null i64 fields.
/// Do not use 0 — that is a valid Unix timestamp (1970-01-01).
pub const NULL_TIMESTAMP: i64 = i64::MIN;

/// Current schema version. Increment when the Tantivy schema changes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

const SCHEMA_VERSION_FILE: &str = "engram_schema_version";

#[derive(Debug)]
pub struct IndexStats {
    pub documents_written: usize,
    pub documents_skipped: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("tantivy error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("directory error: {0}")]
    Directory(#[from] tantivy::directory::error::OpenDirectoryError),

    #[error("schema version error: {0}")]
    SchemaVersion(String),

    #[error("policy denied: {0}")]
    PolicyDenied(String),
}

/// Builds the Tantivy schema used for the Engram index.
pub fn build_schema() -> Schema {
    let mut builder = Schema::builder();

    // TEXT fields — full-text indexed, tokenized, used in BM25 scoring
    builder.add_text_field("title", TEXT | STORED);
    builder.add_text_field("body", TEXT);
    builder.add_text_field("tags", TEXT | STORED);
    builder.add_text_field("keywords", TEXT | STORED);
    builder.add_text_field("domain_tags", TEXT | STORED);
    builder.add_text_field("id", TEXT | STORED);

    // FAST fields — column-oriented, used in scoring formula and filtering
    builder.add_f64_field("importance", FAST);
    builder.add_f64_field("recency", FAST);
    builder.add_f64_field("confidence", FAST);
    builder.add_u64_field("fact_type_int", FAST);
    builder.add_i64_field("valid_until_ts", FAST);
    builder.add_i64_field("event_sequence", FAST);
    builder.add_i64_field("created_at_ts", FAST);
    builder.add_i64_field("updated_at_ts", FAST);

    // STORED-only fields — retrievable for result reconstruction, not searched
    builder.add_text_field("source_path", STORED);
    builder.add_text_field("caused_by", STORED);
    builder.add_text_field("causes", STORED);
    builder.add_text_field("related", STORED);
    builder.add_f64_field("maturity", STORED);
    builder.add_u64_field("access_count", STORED);
    builder.add_u64_field("update_count", STORED);

    builder.build()
}

fn fact_type_to_u64(ft: &FactType) -> u64 {
    match ft {
        FactType::Durable => 0,
        FactType::State => 1,
        FactType::Event => 2,
    }
}

fn handle_schema_version(index_dir: &Path) -> Result<(), IndexError> {
    let version_path = index_dir.join(SCHEMA_VERSION_FILE);

    if !version_path.exists() {
        std::fs::write(&version_path, CURRENT_SCHEMA_VERSION.to_string())?;
        return Ok(());
    }

    let needs_rebuild = match std::fs::read_to_string(&version_path) {
        Ok(content) => match content.trim().parse::<u32>() {
            Ok(found) if found == CURRENT_SCHEMA_VERSION => false,
            Ok(found) => {
                eprintln!(
                    "WARN: Engram schema version mismatch. Index was built with schema v{}. \
                     Current schema is v{}. Wiping and rebuilding the index.",
                    found, CURRENT_SCHEMA_VERSION
                );
                true
            }
            Err(_) => {
                eprintln!(
                    "WARN: Engram schema version mismatch. Index was built with schema v{}. \
                     Current schema is v{}. Wiping and rebuilding the index.",
                    content.trim(),
                    CURRENT_SCHEMA_VERSION
                );
                true
            }
        },
        Err(_) => {
            eprintln!(
                "WARN: Engram schema version mismatch. Index was built with schema v{}. \
                 Current schema is v{}. Wiping and rebuilding the index.",
                "?", CURRENT_SCHEMA_VERSION
            );
            true
        }
    };

    if needs_rebuild {
        std::fs::remove_dir_all(index_dir)?;
        std::fs::create_dir_all(index_dir)?;
        std::fs::write(&version_path, CURRENT_SCHEMA_VERSION.to_string())?;
    }

    Ok(())
}

pub struct IndexWriter {
    root: PathBuf,
}

impl IndexWriter {
    pub fn new(root: &Path) -> Self {
        IndexWriter {
            root: root.to_path_buf(),
        }
    }

    pub fn write(&self, records: Vec<FactRecord>) -> Result<IndexStats, IndexError> {
        let start = std::time::Instant::now();

        let index_dir = self.root.join(".brv").join("index").join("tantivy");
        std::fs::create_dir_all(&index_dir)?;

        handle_schema_version(&index_dir)?;

        let schema = build_schema();

        // Open or create the Tantivy index
        let index = Index::open_or_create(
            tantivy::directory::MmapDirectory::open(&index_dir)?,
            schema.clone(),
        )?;

        // 50MB writer heap
        let mut writer = index.writer(50_000_000)?;

        // Delete all existing documents to prevent duplicates on recompile.
        // The delete is committed together with the new documents atomically.
        writer.delete_all_documents()?;

        // Get field handles
        let f_title = schema.get_field("title").unwrap();
        let f_body = schema.get_field("body").unwrap();
        let f_tags = schema.get_field("tags").unwrap();
        let f_keywords = schema.get_field("keywords").unwrap();
        let f_domain_tags = schema.get_field("domain_tags").unwrap();
        let f_id = schema.get_field("id").unwrap();
        let f_importance = schema.get_field("importance").unwrap();
        let f_recency = schema.get_field("recency").unwrap();
        let f_confidence = schema.get_field("confidence").unwrap();
        let f_fact_type_int = schema.get_field("fact_type_int").unwrap();
        let f_valid_until_ts = schema.get_field("valid_until_ts").unwrap();
        let f_event_sequence = schema.get_field("event_sequence").unwrap();
        let f_created_at_ts = schema.get_field("created_at_ts").unwrap();
        let f_updated_at_ts = schema.get_field("updated_at_ts").unwrap();
        let f_source_path = schema.get_field("source_path").unwrap();
        let f_caused_by = schema.get_field("caused_by").unwrap();
        let f_causes = schema.get_field("causes").unwrap();
        let f_related = schema.get_field("related").unwrap();
        let f_maturity = schema.get_field("maturity").unwrap();
        let f_access_count = schema.get_field("access_count").unwrap();
        let f_update_count = schema.get_field("update_count").unwrap();

        let mut documents_written = 0usize;
        let mut documents_skipped = 0usize;

        for record in records {
            let doc_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                doc!(
                    f_title => record.title.unwrap_or_default(),
                    f_body => record.body,
                    f_tags => record.tags.join(" "),
                    f_keywords => record.keywords.join(" "),
                    f_domain_tags => record.domain_tags.join(" "),
                    f_id => record.id,
                    f_importance => record.importance,
                    f_recency => record.recency,
                    f_confidence => record.confidence,
                    f_fact_type_int => fact_type_to_u64(&record.fact_type),
                    f_valid_until_ts => record.valid_until.map(|dt| dt.timestamp()).unwrap_or(NULL_TIMESTAMP),
                    f_event_sequence => record.event_sequence.unwrap_or(NULL_TIMESTAMP),
                    f_created_at_ts => record.created_at.map(|dt| dt.timestamp()).unwrap_or(NULL_TIMESTAMP),
                    f_updated_at_ts => record.updated_at.map(|dt| dt.timestamp()).unwrap_or(NULL_TIMESTAMP),
                    f_source_path => record.source_path.to_string_lossy().to_string(),
                    f_caused_by => serde_json::to_string(&record.caused_by).unwrap_or_default(),
                    f_causes => serde_json::to_string(&record.causes).unwrap_or_default(),
                    f_related => serde_json::to_string(&record.related).unwrap_or_default(),
                    f_maturity => record.maturity,
                    f_access_count => record.access_count,
                    f_update_count => record.update_count
                )
            }));

            match doc_result {
                Ok(document) => {
                    writer.add_document(document)?;
                    documents_written += 1;
                }
                Err(_) => {
                    documents_skipped += 1;
                }
            }
        }

        writer.commit()?;

        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(IndexStats {
            documents_written,
            documents_skipped,
            elapsed_ms,
        })
    }
}
