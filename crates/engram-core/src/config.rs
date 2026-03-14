use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Top-level TOML structure for `.brv/engram.toml`.
#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    query: QuerySection,
    #[serde(default)]
    compile: CompileSection,
    #[serde(default)]
    access_tracking: AccessTrackingSection,
    #[serde(default)]
    ontology: OntologySection,
    #[serde(default)]
    audit: AuditSection,
}

/// The `[query]` section of the config file.
#[derive(Debug, Default, Deserialize)]
struct QuerySection {
    score_threshold: Option<f64>,
    score_gap: Option<f64>,
    jaccard_threshold: Option<f64>,
    exact_cache_ttl_secs: Option<u64>,
    causal_max_hops: Option<u8>,
    tier3_enabled: Option<bool>,
    tier3_top_n: Option<usize>,
    tier3_score_threshold: Option<f64>,
}

/// The `[compile]` section of the config file.
#[derive(Debug, Default, Deserialize)]
struct CompileSection {
    classify: Option<bool>,
    max_tokens_per_compile: Option<u32>,
}

/// The `[access_tracking]` section of the config file.
#[derive(Debug, Default, Deserialize)]
struct AccessTrackingSection {
    enabled: Option<bool>,
    access_log: Option<String>,
    importance_delta: Option<f64>,
}


/// The `[ontology]` section of the config file.
#[derive(Debug, Default, Deserialize)]
struct OntologySection {
    file: Option<String>,
    expansion_depth: Option<u8>,
}

/// The `[audit]` section of the config file.
#[derive(Debug, Default, Deserialize)]
struct AuditSection {
    max_log_bytes: Option<u64>,
    siem_endpoint: Option<String>,
    siem_token_env: Option<String>,
    siem_required: Option<bool>,
}

/// Ontology configuration.
#[derive(Debug, Clone)]
pub struct OntologyConfig {
    /// Path to the ontology JSON file. Default: `.brv/ontology.json`.
    pub file: Option<PathBuf>,
    /// Expansion depth for query token enrichment. 0 = no expansion; max = 3.
    pub expansion_depth: u8,
}

impl Default for OntologyConfig {
    fn default() -> Self {
        OntologyConfig {
            file: None,
            expansion_depth: 1,
        }
    }
}

/// Audit log configuration.
#[derive(Debug, Clone)]
pub struct AuditConfig {
    /// Maximum audit log size in bytes before rotation. 0 = no rotation.
    pub max_log_bytes: u64,
    /// SIEM endpoint URL. None = SIEM disabled.
    pub siem_endpoint: Option<String>,
    /// Name of env var holding the SIEM bearer token.
    pub siem_token_env: Option<String>,
    /// If true, `new_from_config()` fails at startup when SIEM endpoint is unreachable.
    pub siem_required: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        AuditConfig {
            max_log_bytes: 52_428_800, // 50 MB
            siem_endpoint: None,
            siem_token_env: None,
            siem_required: false,
        }
    }
}

/// Compile-time configuration.
#[derive(Debug, Clone)]
pub struct CompileConfig {
    /// Whether to run the classification pipeline on unclassified facts.
    pub classify: bool,
    /// LLM cost cap: max estimated tokens to send per compile.
    pub max_tokens_per_compile: u32,
}

impl Default for CompileConfig {
    fn default() -> Self {
        CompileConfig {
            classify: false,
            max_tokens_per_compile: 10_000,
        }
    }
}

/// Access tracking configuration.
#[derive(Debug, Clone)]
pub struct AccessTrackingConfig {
    /// Whether access tracking is enabled. Default: true.
    pub enabled: bool,
    /// Path to the access log file. Default: resolved at call site to `.brv/index/access.log`.
    pub access_log: Option<PathBuf>,
    /// Importance boost per access. Default: 0.001.
    pub importance_delta: f64,
}

impl Default for AccessTrackingConfig {
    fn default() -> Self {
        AccessTrackingConfig {
            enabled: true,
            access_log: None,
            importance_delta: 0.001,
        }
    }
}

/// Tier 3 LLM pre-fetch configuration.
#[derive(Debug, Clone)]
pub struct Tier3Config {
    /// Whether Tier 3 LLM synthesis is enabled. Default: false (explicit opt-in).
    pub enabled: bool,
    /// Number of top Tier 2 hits to include as context for the LLM. Default: 5.
    pub top_n: usize,
    /// Score threshold below which Tier 3 triggers. Default: 0.75.
    pub score_threshold: f64,
}

impl Default for Tier3Config {
    fn default() -> Self {
        Tier3Config {
            enabled: false,
            top_n: 5,
            score_threshold: 0.75,
        }
    }
}

/// Workspace-level configuration for Engram.
///
/// All fields have sensible defaults matching the Phase 1 hardcoded values.
/// A missing or partial `.brv/engram.toml` file leaves unspecified fields
/// at their defaults.
#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    pub score_threshold: f64,
    pub score_gap: f64,
    pub jaccard_threshold: f64,
    pub exact_cache_ttl_secs: u64,
    pub causal_max_hops: u8,
    pub compile: CompileConfig,
    pub access_tracking: AccessTrackingConfig,
    pub tier3: Tier3Config,
    pub ontology: OntologyConfig,
    pub audit: AuditConfig,
}

/// Hard cap for causal max_hops. Values above this are clamped.
pub const CAUSAL_MAX_HOPS_CAP: u8 = 6;

impl Default for WorkspaceConfig {
    fn default() -> Self {
        WorkspaceConfig {
            score_threshold: 0.85,
            score_gap: 0.10,
            jaccard_threshold: 0.60,
            exact_cache_ttl_secs: 60,
            causal_max_hops: 3,
            compile: CompileConfig::default(),
            access_tracking: AccessTrackingConfig::default(),
            tier3: Tier3Config::default(),
            ontology: OntologyConfig::default(),
            audit: AuditConfig::default(),
        }
    }
}

/// Load workspace configuration from `.brv/engram.toml`.
///
/// - Returns defaults if the file is absent (no error, no log noise).
/// - Logs a WARN and returns defaults if the file exists but fails to parse.
/// - Never returns `Err` — config loading is infallible from the caller's perspective.
pub fn load_workspace_config(brv_dir: &Path) -> WorkspaceConfig {
    let config_path = brv_dir.join("engram.toml");

    if !config_path.exists() {
        return WorkspaceConfig::default();
    }

    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("WARN: failed to read {}: {}", config_path.display(), e);
            return WorkspaceConfig::default();
        }
    };

    let file: ConfigFile = match toml::from_str(&content) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("WARN: failed to parse {}: {}", config_path.display(), e);
            return WorkspaceConfig::default();
        }
    };

    let defaults = WorkspaceConfig::default();
    let causal_max_hops = match file.query.causal_max_hops {
        Some(v) if v > CAUSAL_MAX_HOPS_CAP => {
            eprintln!(
                "WARN: causal_max_hops={} exceeds cap {}, clamping",
                v, CAUSAL_MAX_HOPS_CAP
            );
            CAUSAL_MAX_HOPS_CAP
        }
        Some(v) => v,
        None => defaults.causal_max_hops,
    };
    let at_defaults = AccessTrackingConfig::default();
    WorkspaceConfig {
        score_threshold: file.query.score_threshold.unwrap_or(defaults.score_threshold),
        score_gap: file.query.score_gap.unwrap_or(defaults.score_gap),
        jaccard_threshold: file.query.jaccard_threshold.unwrap_or(defaults.jaccard_threshold),
        exact_cache_ttl_secs: file.query.exact_cache_ttl_secs.unwrap_or(defaults.exact_cache_ttl_secs),
        causal_max_hops,
        compile: CompileConfig {
            classify: file.compile.classify.unwrap_or(defaults.compile.classify),
            max_tokens_per_compile: file
                .compile
                .max_tokens_per_compile
                .unwrap_or(defaults.compile.max_tokens_per_compile),
        },
        access_tracking: AccessTrackingConfig {
            enabled: file.access_tracking.enabled.unwrap_or(at_defaults.enabled),
            access_log: file.access_tracking.access_log.map(PathBuf::from),
            importance_delta: file
                .access_tracking
                .importance_delta
                .unwrap_or(at_defaults.importance_delta),
        },
        tier3: {
            let t3_defaults = Tier3Config::default();
            Tier3Config {
                enabled: file.query.tier3_enabled.unwrap_or(t3_defaults.enabled),
                top_n: file.query.tier3_top_n.unwrap_or(t3_defaults.top_n),
                score_threshold: file.query.tier3_score_threshold.unwrap_or(t3_defaults.score_threshold),
            }
        },
        ontology: OntologyConfig {
            file: file.ontology.file.map(PathBuf::from),
            expansion_depth: {
                let raw = file.ontology.expansion_depth.unwrap_or(1);
                if raw > 3 {
                    eprintln!(
                        "WARN: ontology.expansion_depth {} exceeds maximum of 3, clamping to 3",
                        raw
                    );
                    3
                } else {
                    raw
                }
            },
        },
        audit: AuditConfig {
            max_log_bytes: file
                .audit
                .max_log_bytes
                .unwrap_or(AuditConfig::default().max_log_bytes),
            siem_endpoint: file.audit.siem_endpoint,
            siem_token_env: file.audit.siem_token_env,
            siem_required: file.audit.siem_required.unwrap_or(false),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_absent_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let brv_dir = tmp.path().join(".brv");
        fs::create_dir_all(&brv_dir).unwrap();

        let config = load_workspace_config(&brv_dir);
        let defaults = WorkspaceConfig::default();
        assert_eq!(config.score_threshold, defaults.score_threshold);
        assert_eq!(config.score_gap, defaults.score_gap);
        assert_eq!(config.jaccard_threshold, defaults.jaccard_threshold);
        assert_eq!(config.exact_cache_ttl_secs, defaults.exact_cache_ttl_secs);
    }

    #[test]
    fn test_empty_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let brv_dir = tmp.path().join(".brv");
        fs::create_dir_all(&brv_dir).unwrap();
        fs::write(brv_dir.join("engram.toml"), "").unwrap();

        let config = load_workspace_config(&brv_dir);
        let defaults = WorkspaceConfig::default();
        assert_eq!(config.score_threshold, defaults.score_threshold);
        assert_eq!(config.score_gap, defaults.score_gap);
        assert_eq!(config.jaccard_threshold, defaults.jaccard_threshold);
        assert_eq!(config.exact_cache_ttl_secs, defaults.exact_cache_ttl_secs);
    }

    #[test]
    fn test_partial_file_overrides_one_field() {
        let tmp = tempfile::tempdir().unwrap();
        let brv_dir = tmp.path().join(".brv");
        fs::create_dir_all(&brv_dir).unwrap();
        fs::write(
            brv_dir.join("engram.toml"),
            "[query]\nscore_threshold = 0.50\n",
        )
        .unwrap();

        let config = load_workspace_config(&brv_dir);
        assert!((config.score_threshold - 0.50).abs() < f64::EPSILON);
        // Others remain at defaults
        let defaults = WorkspaceConfig::default();
        assert_eq!(config.score_gap, defaults.score_gap);
        assert_eq!(config.jaccard_threshold, defaults.jaccard_threshold);
        assert_eq!(config.exact_cache_ttl_secs, defaults.exact_cache_ttl_secs);
    }

    #[test]
    fn test_full_file_overrides_all_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let brv_dir = tmp.path().join(".brv");
        fs::create_dir_all(&brv_dir).unwrap();
        fs::write(
            brv_dir.join("engram.toml"),
            r#"[query]
score_threshold = 0.50
score_gap = 0.20
jaccard_threshold = 0.70
exact_cache_ttl_secs = 120
"#,
        )
        .unwrap();

        let config = load_workspace_config(&brv_dir);
        assert!((config.score_threshold - 0.50).abs() < f64::EPSILON);
        assert!((config.score_gap - 0.20).abs() < f64::EPSILON);
        assert!((config.jaccard_threshold - 0.70).abs() < f64::EPSILON);
        assert_eq!(config.exact_cache_ttl_secs, 120);
    }

    #[test]
    fn test_malformed_toml_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let brv_dir = tmp.path().join(".brv");
        fs::create_dir_all(&brv_dir).unwrap();
        fs::write(brv_dir.join("engram.toml"), "this is not valid toml {{{}}}").unwrap();

        let config = load_workspace_config(&brv_dir);
        let defaults = WorkspaceConfig::default();
        assert_eq!(config.score_threshold, defaults.score_threshold);
        assert_eq!(config.score_gap, defaults.score_gap);
        assert_eq!(config.jaccard_threshold, defaults.jaccard_threshold);
        assert_eq!(config.exact_cache_ttl_secs, defaults.exact_cache_ttl_secs);
    }
}
