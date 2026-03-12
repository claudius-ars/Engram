use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct QueryHit {
    pub id: String,
    pub title: Option<String>,
    pub source_path: String,
    pub tags: Vec<String>,
    pub domain_tags: Vec<String>,
    pub score: f64,
    pub bm25_score: f64,
    pub fact_type: String,
    pub confidence: f64,
    pub importance: f64,
    pub recency: f64,
    pub caused_by: Vec<String>,
    pub causes: Vec<String>,
    pub keywords: Vec<String>,
    pub related: Vec<String>,
    pub maturity: f64,
    pub access_count: u64,
    pub update_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryMeta {
    pub cache_tier: u8,
    pub stale: bool,
    pub dirty_since: Option<DateTime<Utc>>,
    pub query_ms: u64,
    pub total_hits: usize,
    pub index_generation: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub hits: Vec<QueryHit>,
    pub meta: QueryMeta,
}
