use std::path::Path;

use serde::Deserialize;

use crate::policy::{AccessType, PolicyDecision, PolicyRequest};

/// A single policy rule from `bulwark.toml`.
///
/// Rules are evaluated in order; the first match wins.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyRule {
    pub name: String,
    /// "allow" or "deny"
    pub effect: String,
    /// Access type to match: "read", "write", "llm_call", or "*"
    #[serde(default = "default_wildcard")]
    pub access_type: String,
    /// Agent ID glob: "*" matches all
    #[serde(default = "default_wildcard")]
    pub agent: String,
    /// Optional human-readable reason (used in Deny decisions)
    #[serde(default)]
    pub reason: Option<String>,
}

fn default_wildcard() -> String {
    "*".to_string()
}

/// Top-level structure of `bulwark.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyFile {
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

/// Runtime policy state. Holds parsed rules and metadata.
#[derive(Debug, Clone)]
pub struct PolicyState {
    pub rules: Vec<PolicyRule>,
    /// True when loaded from a valid file; false for synthetic allow-all/deny-all.
    pub from_file: bool,
}

impl PolicyState {
    /// Synthetic allow-all state (no file, or file absent).
    pub fn allow_all() -> Self {
        PolicyState {
            rules: vec![PolicyRule {
                name: "default-allow".to_string(),
                effect: "allow".to_string(),
                access_type: "*".to_string(),
                agent: "*".to_string(),
                reason: None,
            }],
            from_file: false,
        }
    }

    /// Synthetic deny-all state (invalid file parse).
    pub fn deny_all() -> Self {
        PolicyState {
            rules: vec![PolicyRule {
                name: "default-deny".to_string(),
                effect: "deny".to_string(),
                access_type: "*".to_string(),
                agent: "*".to_string(),
                reason: Some("Policy file invalid — deny-all failsafe".to_string()),
            }],
            from_file: false,
        }
    }
}

/// Evaluate a policy request against the rule list (first-match).
///
/// Returns `Allow` if the first matching rule has effect "allow",
/// `Deny` if effect is "deny". If no rule matches, defaults to Deny (fail-closed).
pub fn evaluate_policy(state: &PolicyState, request: &PolicyRequest) -> PolicyDecision {
    for rule in &state.rules {
        if matches_rule(rule, request) {
            return match rule.effect.as_str() {
                "deny" => PolicyDecision::Deny {
                    reason: rule
                        .reason
                        .clone()
                        .unwrap_or_else(|| format!("denied by rule '{}'", rule.name)),
                    rule_name: Some(rule.name.clone()),
                },
                _ => PolicyDecision::Allow,
            };
        }
    }
    // No matching rule → fail-closed default deny
    PolicyDecision::Deny {
        reason: "no matching rule; default deny".to_string(),
        rule_name: None,
    }
}

/// Check if a rule matches a request.
fn matches_rule(rule: &PolicyRule, request: &PolicyRequest) -> bool {
    // Match access_type
    if rule.access_type != "*" {
        let rule_at = match rule.access_type.as_str() {
            "read" => AccessType::Read,
            "write" => AccessType::Write,
            "llm_call" => AccessType::LlmCall,
            _ => return false, // unknown access type in rule → no match
        };
        if rule_at != request.access_type {
            return false;
        }
    }

    // Match agent
    if rule.agent != "*" {
        match &request.agent_id {
            Some(agent_id) => {
                if !glob_match(&rule.agent, agent_id) {
                    return false;
                }
            }
            None => return false, // rule requires specific agent but request has none
        }
    }

    true
}

/// Simple glob matching: only supports "*" (match all) and trailing "*" (prefix match).
/// For more complex patterns, a real glob library would be used.
fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    pattern == value
}

/// Load a policy file from disk.
///
/// - If the file does not exist → `PolicyState::allow_all()`
/// - If the file exists but is invalid TOML → `PolicyState::deny_all()` + eprintln warning
/// - If the file is valid → parsed rules with lint warnings
pub fn load_policy_file(path: &Path) -> PolicyState {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return PolicyState::allow_all();
        }
        Err(e) => {
            eprintln!(
                "WARN [bulwark] cannot read policy file {}: {} — deny-all failsafe",
                path.display(),
                e
            );
            return PolicyState::deny_all();
        }
    };

    let policy_file: PolicyFile = match toml::from_str(&content) {
        Ok(pf) => pf,
        Err(e) => {
            eprintln!(
                "WARN [bulwark] invalid policy file {}: {} — deny-all failsafe",
                path.display(),
                e
            );
            return PolicyState::deny_all();
        }
    };

    // Rule ordering lint: warn if wildcard deny appears before any allow
    lint_rule_ordering(&policy_file.rules);

    PolicyState {
        rules: policy_file.rules,
        from_file: true,
    }
}

/// Warn if a wildcard deny rule appears before any allow rule.
fn lint_rule_ordering(rules: &[PolicyRule]) {
    let mut seen_allow = false;
    for rule in rules {
        if rule.effect == "allow" {
            seen_allow = true;
        }
        if rule.effect == "deny" && rule.access_type == "*" && rule.agent == "*" && !seen_allow {
            eprintln!(
                "WARN [bulwark] rule '{}' is a wildcard deny before any allow rule — \
                 all subsequent allow rules will be unreachable",
                rule.name
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allow_all_state() {
        let state = PolicyState::allow_all();
        let req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: None,
            operation: "query".to_string(),
        };
        assert_eq!(evaluate_policy(&state, &req), PolicyDecision::Allow);
    }

    #[test]
    fn test_deny_all_state() {
        let state = PolicyState::deny_all();
        let req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: None,
            operation: "query".to_string(),
        };
        assert!(matches!(
            evaluate_policy(&state, &req),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_first_match_wins() {
        let state = PolicyState {
            rules: vec![
                PolicyRule {
                    name: "allow-read".to_string(),
                    effect: "allow".to_string(),
                    access_type: "read".to_string(),
                    agent: "*".to_string(),
                    reason: None,
                },
                PolicyRule {
                    name: "deny-all".to_string(),
                    effect: "deny".to_string(),
                    access_type: "*".to_string(),
                    agent: "*".to_string(),
                    reason: Some("blocked".to_string()),
                },
            ],
            from_file: true,
        };

        let read_req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: None,
            operation: "query".to_string(),
        };
        assert_eq!(evaluate_policy(&state, &read_req), PolicyDecision::Allow);

        let write_req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
        };
        assert!(matches!(
            evaluate_policy(&state, &write_req),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_agent_glob_match() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("agent-*", "agent-123"));
        assert!(!glob_match("agent-*", "other-123"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "other"));
    }

    #[test]
    fn test_deny_rule_has_rule_name() {
        let state = PolicyState {
            rules: vec![PolicyRule {
                name: "block-writes".to_string(),
                effect: "deny".to_string(),
                access_type: "write".to_string(),
                agent: "*".to_string(),
                reason: Some("no writes allowed".to_string()),
            }],
            from_file: true,
        };
        let req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
        };
        match evaluate_policy(&state, &req) {
            PolicyDecision::Deny { rule_name, reason } => {
                assert_eq!(rule_name, Some("block-writes".to_string()));
                assert_eq!(reason, "no writes allowed");
            }
            _ => panic!("expected deny"),
        }
    }

    #[test]
    fn test_load_missing_file() {
        let state = load_policy_file(Path::new("/nonexistent/bulwark.toml"));
        assert!(!state.from_file);
        assert_eq!(state.rules.len(), 1);
        assert_eq!(state.rules[0].effect, "allow");
    }

    #[test]
    fn test_load_invalid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bulwark.toml");
        std::fs::write(&path, "not valid { toml [[[").unwrap();
        let state = load_policy_file(&path);
        assert!(!state.from_file);
        assert_eq!(state.rules[0].effect, "deny");
    }

    #[test]
    fn test_load_valid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bulwark.toml");
        std::fs::write(
            &path,
            r#"
[[rules]]
name = "allow-reads"
effect = "allow"
access_type = "read"

[[rules]]
name = "deny-writes"
effect = "deny"
access_type = "write"
reason = "read-only mode"
"#,
        )
        .unwrap();

        let state = load_policy_file(&path);
        assert!(state.from_file);
        assert_eq!(state.rules.len(), 2);
        assert_eq!(state.rules[0].name, "allow-reads");
        assert_eq!(state.rules[1].name, "deny-writes");
    }

    #[test]
    fn test_no_rules_default_deny() {
        let state = PolicyState {
            rules: vec![],
            from_file: true,
        };
        let req = PolicyRequest {
            access_type: AccessType::LlmCall,
            fact_id: None,
            agent_id: Some("agent-1".to_string()),
            operation: "tier3".to_string(),
        };
        assert!(matches!(
            evaluate_policy(&state, &req),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_agent_specific_rule() {
        let state = PolicyState {
            rules: vec![
                PolicyRule {
                    name: "deny-untrusted".to_string(),
                    effect: "deny".to_string(),
                    access_type: "*".to_string(),
                    agent: "untrusted-*".to_string(),
                    reason: Some("untrusted agent".to_string()),
                },
                PolicyRule {
                    name: "allow-all".to_string(),
                    effect: "allow".to_string(),
                    access_type: "*".to_string(),
                    agent: "*".to_string(),
                    reason: None,
                },
            ],
            from_file: true,
        };

        let trusted_req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: Some("trusted-agent".to_string()),
            operation: "query".to_string(),
        };
        assert_eq!(evaluate_policy(&state, &trusted_req), PolicyDecision::Allow);

        let untrusted_req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: Some("untrusted-bot".to_string()),
            operation: "query".to_string(),
        };
        assert!(matches!(
            evaluate_policy(&state, &untrusted_req),
            PolicyDecision::Deny { .. }
        ));
    }
}
