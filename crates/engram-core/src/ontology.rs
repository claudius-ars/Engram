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
        Self { namespaces }
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

    /// Expand query tokens with ontology relationships (depth-1).
    /// Returns original tokens plus parent, related, and equivalent terms
    /// for any token that matches a registered namespace:term.
    /// Also matches bare tokens against all namespaces.
    /// Output is deduplicated, order preserved (originals first).
    pub fn expand_tokens(&self, tokens: &[&str]) -> Vec<String> {
        let mut expanded: Vec<String> = tokens.iter().map(|s| s.to_string()).collect();
        let mut seen: HashSet<String> = expanded.iter().cloned().collect();

        for token in tokens {
            let token_lower = token.to_lowercase();
            for def in self.find_term_defs(&token_lower) {
                let mut add = |s: &str| {
                    let s_owned = s.to_string();
                    if !s.is_empty() && seen.insert(s_owned.clone()) {
                        expanded.push(s_owned);
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
        expanded
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
        let expanded = idx.expand_tokens(&["WellIntegrityTest"]);
        assert!(expanded.contains(&"WellIntegrityTest".to_string()));
        assert!(expanded.contains(&"ComplianceCheck".to_string()));
        assert!(expanded.contains(&"WellBarrierTest".to_string()));
        assert_eq!(expanded.len(), 3);
    }

    #[test]
    fn test_expand_tokens_namespaced() {
        let idx = test_index();
        let expanded = idx.expand_tokens(&["iso16530:WellIntegrityTest"]);
        assert!(expanded.contains(&"iso16530:WellIntegrityTest".to_string()));
        assert!(expanded.contains(&"ComplianceCheck".to_string()));
        assert!(expanded.contains(&"WellBarrierTest".to_string()));
    }

    #[test]
    fn test_expand_tokens_no_match() {
        let idx = test_index();
        let expanded = idx.expand_tokens(&["unknowntoken"]);
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
        let expanded = idx.expand_tokens(&["anything", "at", "all"]);
        assert_eq!(expanded, vec!["anything", "at", "all"]);
    }
}
