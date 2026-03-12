use std::collections::HashMap;

use crate::classification_cache::ClassificationCache;
use crate::classifier::{ClassificationMethod, ClassificationResult};

/// Classify a batch of facts via LLM.
/// Returns a map of content_hash → ClassificationResult.
/// Never returns Err — on any failure, logs WARN and returns empty map.
/// Respects the token budget: stops sending batches when budget is exhausted.
///
/// In Phase 2, the actual HTTP call is stubbed. The interface is ready for
/// Phase 3 to wire in real Anthropic API calls.
pub fn classify_batch(
    facts: &[(String, String)], // (content_hash, body)
    _api_key: &str,
    max_tokens: u32,
    cache: &mut ClassificationCache,
) -> HashMap<String, ClassificationResult> {
    let mut results = HashMap::new();
    let mut tokens_used: u32 = 0;

    for (hash, body) in facts {
        // Estimate input tokens as body.len() / 4
        let estimated_tokens = (body.len() as u32) / 4;
        if tokens_used + estimated_tokens > max_tokens {
            eprintln!(
                "WARN: LLM token budget exhausted ({}/{} tokens), {} facts skipped",
                tokens_used,
                max_tokens,
                facts.len() - results.len()
            );
            break;
        }
        tokens_used += estimated_tokens;

        // Phase 2: use stub classification instead of real API call
        let result = classify_stub(body);
        cache.insert(hash.clone(), result.clone());
        results.insert(hash.clone(), result);
    }

    results
}

/// Stub classifier for testing. Returns:
/// - Event for bodies containing "migrated"
/// - State for bodies containing "currently"
/// - Durable otherwise
fn classify_stub(body: &str) -> ClassificationResult {
    let lower = body.to_lowercase();
    let now = chrono::Utc::now().to_rfc3339();

    if lower.contains("migrated") {
        ClassificationResult {
            fact_type: "event".to_string(),
            confidence: 0.92,
            method: ClassificationMethod::Llm,
            classified_at: Some(now),
        }
    } else if lower.contains("currently") {
        ClassificationResult {
            fact_type: "state".to_string(),
            confidence: 0.90,
            method: ClassificationMethod::Llm,
            classified_at: Some(now),
        }
    } else {
        ClassificationResult {
            fact_type: "durable".to_string(),
            confidence: 0.80,
            method: ClassificationMethod::Llm,
            classified_at: Some(now),
        }
    }
}

/// Public test stub — same logic as classify_stub but accessible for tests.
#[cfg(test)]
pub fn classify_batch_stub(
    facts: &[(String, String)],
) -> HashMap<String, ClassificationResult> {
    facts
        .iter()
        .map(|(hash, body)| (hash.clone(), classify_stub(body)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification_cache::ClassificationCache;

    #[test]
    fn test_stub_classifies_correctly() {
        let facts = vec![
            ("hash1".to_string(), "We migrated to the new cluster.".to_string()),
            ("hash2".to_string(), "The API is currently rate-limited.".to_string()),
            ("hash3".to_string(), "Generic system architecture info.".to_string()),
        ];

        let results = classify_batch_stub(&facts);

        assert_eq!(results["hash1"].fact_type, "event");
        assert_eq!(results["hash2"].fact_type, "state");
        assert_eq!(results["hash3"].fact_type, "durable");
    }

    #[test]
    fn test_token_budget_respected() {
        let mut cache = ClassificationCache::new();

        // Each body is 100 chars → ~25 tokens each. Budget of 60 tokens → 2 facts max.
        let facts = vec![
            (
                "h1".to_string(),
                "A".repeat(100), // 25 tokens
            ),
            (
                "h2".to_string(),
                "B".repeat(100), // 25 tokens → cumulative 50
            ),
            (
                "h3".to_string(),
                "C".repeat(100), // 25 tokens → cumulative 75, exceeds 60
            ),
        ];

        let results = classify_batch(&facts, "fake-key", 60, &mut cache);

        assert_eq!(
            results.len(),
            2,
            "only 2 facts should fit within 60-token budget"
        );
        assert!(results.contains_key("h1"));
        assert!(results.contains_key("h2"));
        assert!(
            !results.contains_key("h3"),
            "third fact should be skipped due to budget"
        );
    }
}
