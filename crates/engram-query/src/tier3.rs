use std::path::Path;

use engram_bulwark::{AccessType, BulwarkHandle, PolicyDecision, PolicyRequest};
use engram_core::Tier3Config;

use crate::result::QueryHit;

/// Cache tier constant for LLM-synthesized results.
pub const CACHE_TIER_LLM: u8 = 3;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;

const TIER3_SYSTEM_PROMPT: &str = "\
You are a knowledge synthesis engine. Given a user query and a set of related facts, \
produce a concise, accurate answer that synthesizes the information from those facts. \
Respond with ONLY the synthesized answer — no preamble, no markdown fences, no citations.";

/// Run Tier 3 LLM pre-fetch. Returns `None` if disabled, denied by policy,
/// no API key, or any error occurs. Never returns `Err`.
pub fn run_tier3(
    root: &Path,
    query_string: &str,
    bm25_hits: &[QueryHit],
    config: &Tier3Config,
    bulwark: &BulwarkHandle,
) -> Option<QueryHit> {
    run_tier3_impl(root, query_string, bm25_hits, config, bulwark, ANTHROPIC_API_URL)
}

/// Internal implementation with configurable API URL (for testing).
fn run_tier3_impl(
    root: &Path,
    query_string: &str,
    bm25_hits: &[QueryHit],
    config: &Tier3Config,
    bulwark: &BulwarkHandle,
    api_url: &str,
) -> Option<QueryHit> {
    // 1. Gate: must be enabled
    if !config.enabled {
        return None;
    }

    // 2. Gate: need hits and best score below threshold
    if bm25_hits.is_empty() {
        return None;
    }
    let best_score = bm25_hits[0].score;
    if best_score >= config.score_threshold {
        return None;
    }

    // 3. Bulwark policy check for LLM calls
    let request = PolicyRequest {
        access_type: AccessType::LlmCall,
        fact_id: None,
        agent_id: None,
        operation: "tier3_llm_synthesis".to_string(),
        domain_tags: vec![],
        fact_types: vec![], // fact type unknown at query time; enforcement requires curate scope
    };
    let t0 = std::time::Instant::now();
    let decision = bulwark.check(&request);
    let duration_ms = t0.elapsed().as_millis() as u64;
    bulwark.audit(&request, &decision, duration_ms);
    if let PolicyDecision::Deny { .. } = decision {
        return None;
    }

    // 4. Read API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        eprintln!("WARN [tier3] no ANTHROPIC_API_KEY, skipping LLM synthesis");
        return None;
    }

    // 5. Read fact bodies from source files
    let top_n = config.top_n.min(bm25_hits.len());
    let top_hits = &bm25_hits[..top_n];
    let bodies = read_fact_bodies(root, top_hits);
    if bodies.is_empty() {
        return None;
    }

    // 6. Call LLM
    match call_llm_tier3(&api_key, api_url, query_string, &bodies) {
        Ok(answer) => Some(make_synthetic_hit(answer)),
        Err(e) => {
            eprintln!("WARN [tier3] LLM call failed: {}", e);
            None
        }
    }
}

/// Read the body text of source .md files for the given hits.
/// Strips YAML frontmatter (between `---` delimiters).
fn read_fact_bodies(root: &Path, hits: &[QueryHit]) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for hit in hits {
        let path = root.join(&hit.source_path);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let body = strip_frontmatter(&content);
            if !body.trim().is_empty() {
                let label = hit.title.clone().unwrap_or_else(|| hit.id.clone());
                result.push((label, body));
            }
        }
    }
    result
}

/// Strip YAML frontmatter delimited by `---`.
fn strip_frontmatter(content: &str) -> String {
    if !content.starts_with("---") {
        return content.to_string();
    }
    // Find closing ---
    let after_open = &content[3..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    if let Some(close_pos) = find_closing_separator(after_open) {
        let after_close = &after_open[close_pos + 3..];
        let after_close = after_close.strip_prefix('\n').unwrap_or(after_close);
        after_close.to_string()
    } else {
        content.to_string()
    }
}

/// Find `---` at the start of a line.
fn find_closing_separator(content: &str) -> Option<usize> {
    if content.starts_with("---") && (content.len() == 3 || content.as_bytes().get(3) == Some(&b'\n')) {
        return Some(0);
    }
    let mut search_from = 0;
    while let Some(pos) = content[search_from..].find("\n---") {
        let abs_pos = search_from + pos + 1;
        let after = abs_pos + 3;
        if after >= content.len() || content.as_bytes()[after] == b'\n' {
            return Some(abs_pos);
        }
        search_from = after;
    }
    None
}

#[derive(Debug)]
enum Tier3Error {
    Http(String),
    RateLimit,
}

impl std::fmt::Display for Tier3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tier3Error::Http(msg) => write!(f, "{}", msg),
            Tier3Error::RateLimit => write!(f, "rate limited (429)"),
        }
    }
}

/// Call the Anthropic API synchronously (blocking) for Tier 3 synthesis.
fn call_llm_tier3(
    api_key: &str,
    api_url: &str,
    query_string: &str,
    bodies: &[(String, String)],
) -> Result<String, Tier3Error> {
    let context: String = bodies
        .iter()
        .enumerate()
        .map(|(i, (label, body))| {
            let truncated = if body.len() > 500 { &body[..500] } else { body.as_str() };
            format!("{}. [{}]: {}", i + 1, label, truncated)
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let user_message = format!(
        "Query: {}\n\nContext facts:\n{}",
        query_string, context
    );

    let request_body = serde_json::json!({
        "model": "claude-haiku-4-5-20251001",
        "max_tokens": MAX_TOKENS,
        "system": TIER3_SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": user_message
        }]
    });

    let client = reqwest::blocking::Client::new();

    // First attempt
    let result = do_blocking_call(&client, api_key, api_url, &request_body);

    // On 429, retry once after 1 second
    match result {
        Err(Tier3Error::RateLimit) => {
            std::thread::sleep(std::time::Duration::from_secs(1));
            do_blocking_call(&client, api_key, api_url, &request_body)
        }
        other => other,
    }
}

fn do_blocking_call(
    client: &reqwest::blocking::Client,
    api_key: &str,
    api_url: &str,
    request_body: &serde_json::Value,
) -> Result<String, Tier3Error> {
    let resp = client
        .post(api_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(request_body)
        .send()
        .map_err(|e| Tier3Error::Http(e.to_string()))?;

    let status = resp.status();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(Tier3Error::RateLimit);
    }

    if !status.is_success() {
        return Err(Tier3Error::Http(format!("HTTP {}", status.as_u16())));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| Tier3Error::Http(format!("response parse error: {}", e)))?;

    let content = body["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("")
        .to_string();

    if content.is_empty() {
        return Err(Tier3Error::Http("empty response content".to_string()));
    }

    Ok(content)
}

/// Build a synthetic QueryHit from the LLM answer.
fn make_synthetic_hit(answer: String) -> QueryHit {
    QueryHit {
        id: "llm-synthesized".to_string(),
        title: Some("LLM Synthesis".to_string()),
        source_path: "<llm:tier3>".to_string(),
        score: 1.0,
        fact_type: "durable".to_string(),
        confidence: 1.0,
        keywords: vec!["llm-synthesized".to_string()],
        answer: Some(answer),
        ..QueryHit::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_bulwark::BulwarkHandle;
    use engram_core::Tier3Config;

    #[test]
    fn test_strip_frontmatter_basic() {
        let content = "---\ntitle: Hello\n---\nBody text here.";
        assert_eq!(strip_frontmatter(content), "Body text here.");
    }

    #[test]
    fn test_strip_frontmatter_no_frontmatter() {
        let content = "Just plain text.";
        assert_eq!(strip_frontmatter(content), "Just plain text.");
    }

    #[test]
    fn test_strip_frontmatter_empty_body() {
        let content = "---\ntitle: Hello\n---\n";
        assert_eq!(strip_frontmatter(content), "");
    }

    #[test]
    fn test_make_synthetic_hit() {
        let hit = make_synthetic_hit("The answer is 42.".to_string());
        assert_eq!(hit.id, "llm-synthesized");
        assert_eq!(hit.score, 1.0);
        assert_eq!(hit.answer, Some("The answer is 42.".to_string()));
        assert!(hit.tags.is_empty());
    }

    #[test]
    fn test_tier3_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Tier3Config {
            enabled: false,
            ..Tier3Config::default()
        };
        let bulwark = BulwarkHandle::new_stub();
        let result = run_tier3(tmp.path(), "test query", &[], &config, &bulwark);
        assert!(result.is_none());
    }

    #[test]
    fn test_tier3_score_above_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Tier3Config {
            enabled: true,
            score_threshold: 0.75,
            top_n: 5,
        };
        let bulwark = BulwarkHandle::new_stub();
        let hits = vec![QueryHit {
            id: "f1".to_string(),
            score: 0.90, // above threshold
            ..QueryHit::default()
        }];
        let result = run_tier3(tmp.path(), "test query", &hits, &config, &bulwark);
        assert!(result.is_none());
    }

    #[test]
    fn test_tier3_policy_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Tier3Config {
            enabled: true,
            score_threshold: 0.75,
            top_n: 5,
        };
        let bulwark = BulwarkHandle::new_denying();
        let hits = vec![QueryHit {
            id: "f1".to_string(),
            score: 0.50,
            ..QueryHit::default()
        }];
        let result = run_tier3(tmp.path(), "test query", &hits, &config, &bulwark);
        assert!(result.is_none());
    }

    #[test]
    fn test_tier3_no_api_key() {
        // Ensure ANTHROPIC_API_KEY is not set for this test
        std::env::remove_var("ANTHROPIC_API_KEY");
        let tmp = tempfile::tempdir().unwrap();
        let config = Tier3Config {
            enabled: true,
            score_threshold: 0.75,
            top_n: 5,
        };
        let bulwark = BulwarkHandle::new_stub();
        let hits = vec![QueryHit {
            id: "f1".to_string(),
            score: 0.50,
            ..QueryHit::default()
        }];
        let result = run_tier3(tmp.path(), "test query", &hits, &config, &bulwark);
        assert!(result.is_none());
    }

    #[test]
    fn test_call_llm_tier3_success() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": "Capybaras are the largest living rodents."}],
                    "usage": {"input_tokens": 100, "output_tokens": 20}
                })
                .to_string(),
            )
            .create();

        let bodies = vec![
            ("Capybara Facts".to_string(), "The capybara is the largest living rodent.".to_string()),
        ];

        let result = call_llm_tier3("test-key", &server.url(), "capybara facts", &bodies);
        mock.assert();
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Capybaras"));
    }

    #[test]
    fn test_call_llm_tier3_http_error() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .with_status(500)
            .create();

        let bodies = vec![("Fact".to_string(), "Body".to_string())];
        let result = call_llm_tier3("test-key", &server.url(), "query", &bodies);
        mock.assert();
        assert!(result.is_err());
    }

    #[test]
    fn test_call_llm_tier3_429_retry() {
        let mut server = mockito::Server::new();

        // First call returns 429, second returns 200
        let _mock_429 = server
            .mock("POST", "/")
            .with_status(429)
            .expect(1)
            .create();

        let _mock_200 = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": "Synthesized answer."}],
                    "usage": {"input_tokens": 50, "output_tokens": 10}
                })
                .to_string(),
            )
            .expect(1)
            .create();

        let bodies = vec![("Fact".to_string(), "Body text".to_string())];
        let result = call_llm_tier3("test-key", &server.url(), "query", &bodies);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Synthesized answer.");
    }

    #[test]
    fn test_read_fact_bodies() {
        let tmp = tempfile::tempdir().unwrap();
        let fact_path = tmp.path().join("facts");
        std::fs::create_dir_all(&fact_path).unwrap();
        std::fs::write(
            fact_path.join("test.md"),
            "---\ntitle: Test\n---\nThis is the body.",
        ).unwrap();

        let hits = vec![QueryHit {
            id: "test".to_string(),
            title: Some("Test".to_string()),
            source_path: "facts/test.md".to_string(),
            ..QueryHit::default()
        }];

        let bodies = read_fact_bodies(tmp.path(), &hits);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].0, "Test");
        assert_eq!(bodies[0].1, "This is the body.");
    }
}
