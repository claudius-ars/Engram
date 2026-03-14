use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct OntologyFile {
    pub version: u32,
    pub namespaces: HashMap<String, NamespaceDef>,
}

#[derive(Debug, Deserialize)]
pub struct NamespaceDef {
    pub label: Option<String>,
    #[serde(default)]
    pub terms: HashMap<String, TermDef>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct TermDef {
    pub parent: Option<String>,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub equivalent: Vec<String>,
}

#[derive(Debug, Default)]
pub struct OntologyIndex {
    /// namespace (lowercase) → NamespaceEntry
    namespaces: HashMap<String, NamespaceEntry>,
}

#[derive(Debug, Default)]
struct NamespaceEntry {
    #[allow(dead_code)]
    label: Option<String>,
    /// None = namespace registered but no term list (prefix-only validation)
    /// Some(empty map) = namespace with terms key present but empty
    /// Some(populated map) = normal term validation
    terms: Option<HashMap<String, TermDef>>,
}

pub enum TagValidation {
    /// No colon in tag, or namespace prefix not registered.
    NoValidation,
    /// Namespace registered and term found (or namespace has no terms list).
    Valid,
    /// Namespace registered with a terms list, term not found.
    UnknownTerm { namespace: String, term: String },
}

impl OntologyIndex {
    pub fn load(path: &Path) -> Result<Self, OntologyError> {
        let content = std::fs::read_to_string(path)?;
        let file: OntologyFile = serde_json::from_str(&content)?;
        Ok(Self::from_file(file))
    }

    pub fn from_file(file: OntologyFile) -> Self {
        let mut namespaces = HashMap::new();
        for (ns_key, ns_def) in file.namespaces {
            let ns_lower = ns_key.to_lowercase();
            let terms = if ns_def.terms.is_empty() {
                None // no terms key or empty → prefix-only validation
            } else {
                let mut term_map = HashMap::new();
                for (term_key, term_def) in ns_def.terms {
                    term_map.insert(term_key.to_lowercase(), term_def);
                }
                Some(term_map)
            };
            namespaces.insert(
                ns_lower,
                NamespaceEntry {
                    label: ns_def.label,
                    terms,
                },
            );
        }
        let index = Self { namespaces };
        index.detect_cycles();
        index
    }

    /// Detect cycles in the ontology graph at load time (informational).
    /// Uses DFS with three-color marking: white (unvisited), gray (in stack), black (done).
    fn detect_cycles(&self) {
        let mut color: HashMap<String, u8> = HashMap::new(); // 0=white, 1=gray, 2=black

        // Collect all term keys as "ns:term" for global graph traversal
        let mut all_terms: Vec<String> = Vec::new();
        for (ns, entry) in &self.namespaces {
            if let Some(ref terms) = entry.terms {
                for term_id in terms.keys() {
                    all_terms.push(format!("{}:{}", ns, term_id));
                }
            }
        }

        for term_id in &all_terms {
            if *color.get(term_id).unwrap_or(&0) == 0 {
                self.dfs_cycle_detect(term_id, &mut color);
            }
        }
    }

    fn dfs_cycle_detect(&self, term_id: &str, color: &mut HashMap<String, u8>) {
        color.insert(term_id.to_string(), 1); // gray

        for neighbor in self.get_neighbors(term_id) {
            match color.get(&neighbor).unwrap_or(&0) {
                0 => self.dfs_cycle_detect(&neighbor, color),
                1 => {
                    eprintln!(
                        "WARN [ontology] cycle detected involving term '{}'",
                        neighbor
                    );
                }
                _ => {} // black — already fully explored
            }
        }

        color.insert(term_id.to_string(), 2); // black
    }

    /// Get all neighbor term IDs for a given "ns:term" key.
    fn get_neighbors(&self, term_id: &str) -> Vec<String> {
        let mut neighbors = Vec::new();
        let Some((ns, term)) = term_id.split_once(':') else {
            return neighbors;
        };
        let Some(entry) = self.namespaces.get(ns) else {
            return neighbors;
        };
        let Some(ref terms) = entry.terms else {
            return neighbors;
        };
        let Some(def) = terms.get(term) else {
            return neighbors;
        };

        // Resolve neighbor references: they may be bare term names within the same namespace
        let resolve = |name: &str| -> String {
            if name.contains(':') {
                name.to_lowercase()
            } else {
                format!("{}:{}", ns, name.to_lowercase())
            }
        };

        if let Some(ref p) = def.parent {
            neighbors.push(resolve(p));
        }
        for r in &def.related {
            neighbors.push(resolve(r));
        }
        for e in &def.equivalent {
            neighbors.push(resolve(e));
        }
        neighbors
    }

    /// Validate a single domain_tag value.
    /// Tag format: "namespace:term" → validated against registered namespaces.
    /// "freeform" (no colon) → NoValidation.
    /// All comparisons are case-insensitive.
    pub fn validate_tag(&self, tag: &str) -> TagValidation {
        let tag_lower = tag.to_lowercase();
        let Some((ns, term)) = tag_lower.split_once(':') else {
            return TagValidation::NoValidation;
        };
        let Some(entry) = self.namespaces.get(ns) else {
            return TagValidation::NoValidation; // unregistered prefix
        };
        match &entry.terms {
            None => TagValidation::Valid, // prefix-only namespace
            Some(terms) if terms.contains_key(term) => TagValidation::Valid,
            Some(_) => TagValidation::UnknownTerm {
                namespace: ns.to_owned(),
                term: term.to_owned(),
            },
        }
    }

    /// Expand query tokens with ontology relationships via BFS.
    ///
    /// `depth = 0` returns the input tokens unchanged (no expansion).
    /// `depth = 1` expands direct neighbors (parent, related, equivalent).
    /// `depth = 2..=3` continues BFS to second/third-degree neighbors.
    ///
    /// A visited set prevents cycles and duplicates. Original tokens appear
    /// first in the output, in input order. Expansion terms appear in BFS order.
    pub fn expand_tokens(&self, tokens: &[&str], depth: u8) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();

        // Initialize with input tokens
        let mut frontier: Vec<String> = Vec::new();
        for token in tokens {
            let s = token.to_string();
            if visited.insert(s.clone()) {
                result.push(s.clone());
                frontier.push(s);
            }
        }

        if depth == 0 {
            return result;
        }

        for _level in 1..=depth {
            let mut next_frontier: Vec<String> = Vec::new();
            for term in &frontier {
                let term_lower = term.to_lowercase();
                for def in self.find_term_defs(&term_lower) {
                    let mut add = |s: &str| {
                        let s_owned = s.to_string();
                        if !s.is_empty() && visited.insert(s_owned.clone()) {
                            result.push(s_owned.clone());
                            next_frontier.push(s_owned);
                        }
                    };
                    if let Some(ref p) = def.parent {
                        add(p);
                    }
                    for r in &def.related {
                        add(r);
                    }
                    for e in &def.equivalent {
                        add(e);
                    }
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }

        result
    }

    fn find_term_defs(&self, token_lower: &str) -> Vec<&TermDef> {
        let mut results = Vec::new();
        if let Some((ns, term)) = token_lower.split_once(':') {
            if let Some(entry) = self.namespaces.get(ns) {
                if let Some(ref terms) = entry.terms {
                    if let Some(def) = terms.get(term) {
                        results.push(def);
                    }
                }
            }
        } else {
            // Bare token — search all namespaces
            for entry in self.namespaces.values() {
                if let Some(ref terms) = entry.terms {
                    if let Some(def) = terms.get(token_lower) {
                        results.push(def);
                    }
                }
            }
        }
        results
    }

    /// True if no namespaces are registered (fast path: no-op).
    pub fn is_empty(&self) -> bool {
        self.namespaces.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OntologyError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_index() -> OntologyIndex {
        let json = include_str!("../../../tests/fixtures/ontology_test.json");
        OntologyIndex::from_file(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn test_validate_known_term() {
        let idx = test_index();
        assert!(matches!(
            idx.validate_tag("iso16530:WellIntegrityTest"),
            TagValidation::Valid
        ));
    }

    #[test]
    fn test_validate_unknown_term() {
        let idx = test_index();
        assert!(matches!(
            idx.validate_tag("iso16530:NoSuchTerm"),
            TagValidation::UnknownTerm { .. }
        ));
    }

    #[test]
    fn test_validate_unregistered_namespace() {
        let idx = test_index();
        assert!(matches!(
            idx.validate_tag("custom:anything"),
            TagValidation::NoValidation
        ));
    }

    #[test]
    fn test_validate_freeform_tag() {
        let idx = test_index();
        assert!(matches!(
            idx.validate_tag("no_colon_here"),
            TagValidation::NoValidation
        ));
    }

    #[test]
    fn test_validate_prefix_only_namespace() {
        let idx = test_index();
        // "osdu" has no terms list — any osdu: tag is Valid
        assert!(matches!(
            idx.validate_tag("osdu:AnythingAtAll"),
            TagValidation::Valid
        ));
    }

    #[test]
    fn test_expand_tokens_depth_one() {
        let idx = test_index();
        let expanded = idx.expand_tokens(&["WellIntegrityTest"], 1);
        assert!(expanded.contains(&"WellIntegrityTest".to_string()));
        assert!(expanded.contains(&"ComplianceCheck".to_string()));
        assert!(expanded.contains(&"WellBarrierTest".to_string()));
        assert_eq!(expanded.len(), 3);
    }

    #[test]
    fn test_expand_tokens_namespaced() {
        let idx = test_index();
        let expanded = idx.expand_tokens(&["iso16530:WellIntegrityTest"], 1);
        assert!(expanded.contains(&"iso16530:WellIntegrityTest".to_string()));
        assert!(expanded.contains(&"ComplianceCheck".to_string()));
        assert!(expanded.contains(&"WellBarrierTest".to_string()));
    }

    #[test]
    fn test_expand_tokens_no_match() {
        let idx = test_index();
        let expanded = idx.expand_tokens(&["unknowntoken"], 1);
        assert_eq!(expanded, vec!["unknowntoken".to_string()]);
    }

    #[test]
    fn test_case_normalization_validate() {
        let idx = test_index();
        assert!(matches!(
            idx.validate_tag("ISO16530:WellIntegrityTest"),
            TagValidation::Valid
        ));
        assert!(matches!(
            idx.validate_tag("iso16530:wellintegritytest"),
            TagValidation::Valid
        ));
        assert!(matches!(
            idx.validate_tag("ISO16530:WELLINTEGRITYTEST"),
            TagValidation::Valid
        ));
    }

    #[test]
    fn test_no_ontology_passthrough() {
        let idx = OntologyIndex::default();
        let expanded = idx.expand_tokens(&["anything", "at", "all"], 1);
        assert_eq!(expanded, vec!["anything", "at", "all"]);
    }

    // ── Prompt 6: configurable expansion depth tests ──

    #[test]
    fn test_depth_zero_no_expansion() {
        let idx = test_index();
        let expanded = idx.expand_tokens(&["WellIntegrityTest"], 0);
        assert_eq!(expanded, vec!["WellIntegrityTest".to_string()]);
    }

    #[test]
    fn test_depth_one_direct_neighbors() {
        let idx = test_index();
        let expanded = idx.expand_tokens(&["WellIntegrityTest"], 1);
        assert!(expanded.contains(&"WellIntegrityTest".to_string()));
        assert!(expanded.contains(&"ComplianceCheck".to_string()));
        assert!(expanded.contains(&"WellBarrierTest".to_string()));
        assert_eq!(expanded.len(), 3);
    }

    #[test]
    fn test_depth_two_grandparent() {
        // Build a chain: A → parent B → parent C
        // At depth 1 from A we get B; at depth 2 we also get C.
        let json = r#"{
            "version": 1,
            "namespaces": {
                "test": {
                    "label": "Test chain",
                    "terms": {
                        "A": { "parent": "B", "related": [], "equivalent": [] },
                        "B": { "parent": "C", "related": [], "equivalent": [] },
                        "C": { "parent": null, "related": [], "equivalent": [] }
                    }
                }
            }
        }"#;
        let idx = OntologyIndex::from_file(serde_json::from_str(json).unwrap());

        // Depth 1: A + B
        let d1 = idx.expand_tokens(&["A"], 1);
        assert_eq!(d1.len(), 2);
        assert!(d1.contains(&"A".to_string()));
        assert!(d1.contains(&"B".to_string()));

        // Depth 2: A + B + C
        let d2 = idx.expand_tokens(&["A"], 2);
        assert_eq!(d2.len(), 3);
        assert!(d2.contains(&"A".to_string()));
        assert!(d2.contains(&"B".to_string()));
        assert!(d2.contains(&"C".to_string()));
    }

    #[test]
    fn test_depth_cap_clamped() {
        // Config loading clamps expansion_depth > 3 to 3.
        // Verify by loading a config with expansion_depth = 10.
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let brv_dir = tmp.path().join(".brv");
        fs::create_dir_all(&brv_dir).unwrap();
        fs::write(
            brv_dir.join("engram.toml"),
            "[ontology]\nexpansion_depth = 10\n",
        )
        .unwrap();
        let config = crate::config::load_workspace_config(&brv_dir);
        assert_eq!(config.ontology.expansion_depth, 3);
    }

    #[test]
    fn test_cycle_safe_expansion() {
        // A → related B, B → related A  (mutual cycle)
        let json = r#"{
            "version": 1,
            "namespaces": {
                "cyc": {
                    "label": "Cycle ns",
                    "terms": {
                        "A": { "parent": null, "related": ["B"], "equivalent": [] },
                        "B": { "parent": null, "related": ["A"], "equivalent": [] }
                    }
                }
            }
        }"#;
        let idx = OntologyIndex::from_file(serde_json::from_str(json).unwrap());

        // BFS with visited set must terminate and include both
        let expanded = idx.expand_tokens(&["A"], 3);
        assert!(expanded.contains(&"A".to_string()));
        assert!(expanded.contains(&"B".to_string()));
        assert_eq!(expanded.len(), 2); // no duplicates
    }

    #[test]
    fn test_depth_one_backward_compat() {
        // Default expansion_depth is 1 — same as pre-Prompt-6 behavior
        let defaults = crate::config::OntologyConfig::default();
        assert_eq!(defaults.expansion_depth, 1);

        let idx = test_index();
        let expanded = idx.expand_tokens(&["WellIntegrityTest"], defaults.expansion_depth);
        assert_eq!(expanded.len(), 3);
        assert!(expanded.contains(&"WellIntegrityTest".to_string()));
        assert!(expanded.contains(&"ComplianceCheck".to_string()));
        assert!(expanded.contains(&"WellBarrierTest".to_string()));
    }
}
