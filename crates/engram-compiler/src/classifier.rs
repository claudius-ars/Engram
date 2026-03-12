use engram_core::FactType;
use once_cell::sync::Lazy;
use regex::Regex;

/// Minimum confidence from rule-based classification to skip LLM escalation.
pub const RULE_CONFIDENCE_THRESHOLD: f32 = 0.7;

/// Classification method that produced the result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClassificationMethod {
    Rules,
    Llm,
    Default,
}

/// Result of classifying a single fact.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClassificationResult {
    pub fact_type: String, // "durable", "state", "event"
    pub confidence: f32,
    pub method: ClassificationMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classified_at: Option<String>,
}

impl ClassificationResult {
    pub fn to_fact_type(&self) -> FactType {
        match self.fact_type.as_str() {
            "state" => FactType::State,
            "event" => FactType::Event,
            _ => FactType::Durable,
        }
    }
}

// --- Keyword lists ---

pub const STATE_KEYWORDS: &[&str] = &[
    "currently",
    "right now",
    "as of",
    "at present",
    "at this time",
    "is currently",
    "are currently",
    "is now",
    "is set to",
    "is configured",
    "is enabled",
    "is disabled",
    "is running",
    "is deployed",
    "is live",
    "rate limit",
    "rate-limit",
    "quota",
    "threshold",
    "flag is",
    "feature flag",
    "config is",
    "environment is",
];

pub const EVENT_KEYWORDS: &[&str] = &[
    "migrated",
    "migration",
    "deployed",
    "deployment",
    "released",
    "release",
    "upgraded",
    "downgraded",
    "rotated",
    "replaced",
    "switched",
    "changed to",
    "moved to",
    "promoted",
    "merged",
    "rolled back",
    "rollback",
    "incident",
    "postmortem",
    "outage",
    "broke",
    "fixed",
    "patched",
    "launched",
    "deprecated",
    "on january",
    "on february",
    "on march",
    "on april",
    "on may",
    "on june",
    "on july",
    "on august",
    "on september",
    "on october",
    "on november",
    "on december",
];

pub const DURABLE_KEYWORDS: &[&str] = &[
    "always",
    "never",
    "invariant",
    "by design",
    "architectural decision",
    "adr",
    "principle",
    "convention",
    "standard",
    "requirement",
    "must",
    "must not",
    "shall",
    "shall not",
];

// --- Date patterns (compiled once via Lazy) ---

static DATE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"\b\d{4}-\d{2}-\d{2}\b",     // ISO date: 2024-03-15
        r"\bon \w+ \d{1,2}\b",         // "on March 3"
        r"\bin Q[1-4] \d{4}\b",        // "in Q1 2024"
        r"\bin \w+ \d{4}\b",           // "in January 2024"
    ]
    .iter()
    .map(|p| Regex::new(p).expect("date pattern should compile"))
    .collect()
});

/// Check if a keyword matches in the given lowercased text.
/// Single-word keywords require word boundary matching.
/// Multi-word keywords use substring matching.
fn keyword_matches(lower_text: &str, keyword: &str) -> bool {
    if keyword.contains(' ') {
        lower_text.contains(keyword)
    } else {
        lower_text.split_whitespace().any(|word| {
            let word = word.trim_matches(|c: char| c.is_ascii_punctuation());
            word == keyword
        })
    }
}

/// Classify a fact based on its title and body using rule-based heuristics.
pub fn rule_classify(title: &str, body: &str) -> ClassificationResult {
    let combined = format!("{} {}", title, body).to_lowercase();

    let has_state = STATE_KEYWORDS
        .iter()
        .any(|kw| keyword_matches(&combined, kw));
    let has_event = EVENT_KEYWORDS
        .iter()
        .any(|kw| keyword_matches(&combined, kw));
    let has_durable = DURABLE_KEYWORDS
        .iter()
        .any(|kw| keyword_matches(&combined, kw));

    // Check date patterns → Event with high confidence
    let has_date_pattern = DATE_PATTERNS.iter().any(|re| re.is_match(&combined));

    if has_date_pattern {
        return ClassificationResult {
            fact_type: "event".to_string(),
            confidence: 0.90,
            method: ClassificationMethod::Rules,
            classified_at: None,
        };
    }

    // Event beats state when both present (events are more specific)
    if has_event {
        return ClassificationResult {
            fact_type: "event".to_string(),
            confidence: 0.85,
            method: ClassificationMethod::Rules,
            classified_at: None,
        };
    }

    if has_state {
        return ClassificationResult {
            fact_type: "state".to_string(),
            confidence: 0.85,
            method: ClassificationMethod::Rules,
            classified_at: None,
        };
    }

    if has_durable && !has_state && !has_event {
        return ClassificationResult {
            fact_type: "durable".to_string(),
            confidence: 0.95,
            method: ClassificationMethod::Rules,
            classified_at: None,
        };
    }

    // No signals → low confidence durable (below threshold → LLM queue)
    ClassificationResult {
        fact_type: "durable".to_string(),
        confidence: 0.4,
        method: ClassificationMethod::Rules,
        classified_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_keyword_classifies_state() {
        let result = rule_classify("API Rate Limiting", "The API is currently rate-limited to 100 req/s.");
        assert_eq!(result.fact_type, "state");
        assert!(result.confidence >= 0.85);
    }

    #[test]
    fn test_event_keyword_classifies_event() {
        let result = rule_classify("Auth Migration", "We migrated from HMAC to RS256 last quarter.");
        assert_eq!(result.fact_type, "event");
        assert!(result.confidence >= 0.85);
    }

    #[test]
    fn test_date_pattern_classifies_event() {
        let result = rule_classify("Key Rotation", "Keys were rotated on 2024-03-15 during maintenance.");
        assert_eq!(result.fact_type, "event");
        assert!(result.confidence >= 0.90);
    }

    #[test]
    fn test_durable_keyword_classifies_durable() {
        let result = rule_classify("Design Principle", "This is an architectural decision that we always follow.");
        assert_eq!(result.fact_type, "durable");
        assert!(result.confidence >= 0.95);
    }

    #[test]
    fn test_event_beats_state_when_both_present() {
        let result = rule_classify(
            "Deploy Update",
            "The service is currently deployed after we migrated to the new cluster.",
        );
        assert_eq!(result.fact_type, "event", "event should beat state when both present");
    }

    #[test]
    fn test_no_signal_returns_low_confidence() {
        let result = rule_classify("Some Fact", "This is a generic piece of information about the system.");
        assert!(
            result.confidence < RULE_CONFIDENCE_THRESHOLD,
            "no-signal result confidence {} should be below threshold {}",
            result.confidence,
            RULE_CONFIDENCE_THRESHOLD
        );
    }

    #[test]
    fn test_whole_word_matching() {
        // "migrate" should not match "migrated" keyword — but "migrated" IS in the keyword list
        // The point is: single-word matching checks whole words.
        // "migrated" as a standalone word in body DOES match keyword "migrated"
        let result = rule_classify("Title", "We migrated the database.");
        assert_eq!(result.fact_type, "event");

        // "migrate" is NOT in the keyword list, so it should not match
        let result2 = rule_classify("Title", "We plan to migrate the database.");
        // "migrate" is not a keyword — should not match event
        assert_ne!(result2.fact_type, "event", "\"migrate\" is not in EVENT_KEYWORDS");
    }
}
