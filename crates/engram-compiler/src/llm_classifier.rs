use crate::classifier::{rule_classify, ClassificationMethod, ClassificationResult};

/// System prompt for the LLM fact classifier.
pub const CLASSIFY_SYSTEM_PROMPT: &str = "\
You are a fact classifier for a knowledge graph. Classify each fact as one of three types:

- \"durable\": Permanent truths, architectural decisions, invariants, conventions.
- \"state\": Current conditions that may change: feature flags, config values, deployment status.
- \"event\": Things that happened at a specific time: migrations, incidents, deployments, releases.

Respond ONLY with a JSON array. No prose, no markdown fences, no explanation.
Each element must have exactly this schema: {\"fact_id\": string, \"fact_type\": string, \"confidence\": number}
where fact_type is one of \"durable\", \"state\", or \"event\" and confidence is a number between 0.0 and 1.0.";

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_BODY_CHARS: usize = 200;
const MAX_TOKENS: u32 = 1024;
const TOKENS_PER_FACT_ESTIMATE: u32 = 50;

/// Classify a batch of facts via the Anthropic API.
///
/// Always returns results — never fails. On any error (HTTP, parse, missing key),
/// falls back to the rule-based classifier. Respects the token budget: skips the
/// API call when the remaining budget is insufficient for the estimated cost.
pub async fn classify_batch(
    facts: &[(&str, &str)], // (fact_id, body_text)
    api_key: &str,
    model: &str,
    token_budget: &mut u32,
) -> Vec<ClassificationResult> {
    classify_batch_impl(facts, api_key, model, token_budget, ANTHROPIC_API_URL).await
}

/// Internal implementation with configurable API URL (for testing).
async fn classify_batch_impl(
    facts: &[(&str, &str)],
    api_key: &str,
    model: &str,
    token_budget: &mut u32,
    api_url: &str,
) -> Vec<ClassificationResult> {
    if facts.is_empty() {
        return vec![];
    }

    // Empty API key → fallback
    if api_key.is_empty() {
        eprintln!("WARN [classifier] no API key, using rule-based fallback");
        return fallback_all(facts);
    }

    // Budget check before call
    let estimated_cost = facts.len() as u32 * TOKENS_PER_FACT_ESTIMATE;
    if *token_budget < estimated_cost {
        eprintln!("WARN [classifier] token budget exhausted");
        return fallback_all(facts);
    }

    // Build user message with truncated bodies
    let user_message: String = facts
        .iter()
        .enumerate()
        .map(|(i, (id, body))| {
            let truncated = if body.len() > MAX_BODY_CHARS {
                &body[..MAX_BODY_CHARS]
            } else {
                body
            };
            format!("{}. [{}]: {}", i + 1, id, truncated)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let request_body = serde_json::json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": CLASSIFY_SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": user_message
        }]
    });

    let client = reqwest::Client::new();

    // First attempt
    let result = call_api(&client, api_key, api_url, &request_body).await;

    // On 429, retry once after 1 second
    let result = match result {
        Err(ApiError::RateLimit) => {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            call_api(&client, api_key, api_url, &request_body).await
        }
        other => other,
    };

    match result {
        Ok(response) => {
            // Decrement token budget by input tokens used
            *token_budget = token_budget.saturating_sub(response.input_tokens);
            parse_response(&response.content, facts)
        }
        Err(e) => {
            eprintln!("WARN [classifier] API error: {}, using rule-based fallback", e);
            fallback_all(facts)
        }
    }
}

#[derive(Debug)]
enum ApiError {
    RateLimit,
    Http(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::RateLimit => write!(f, "rate limited (429)"),
            ApiError::Http(msg) => write!(f, "{}", msg),
        }
    }
}

struct ApiResponse {
    content: String,
    input_tokens: u32,
}

async fn call_api(
    client: &reqwest::Client,
    api_key: &str,
    api_url: &str,
    request_body: &serde_json::Value,
) -> Result<ApiResponse, ApiError> {
    let resp = client
        .post(api_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(request_body)
        .send()
        .await
        .map_err(|e| ApiError::Http(e.to_string()))?;

    let status = resp.status();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ApiError::RateLimit);
    }

    if !status.is_success() {
        return Err(ApiError::Http(format!("HTTP {}", status.as_u16())));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Http(format!("response parse error: {}", e)))?;

    let content = body["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("")
        .to_string();

    let input_tokens = body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;

    Ok(ApiResponse {
        content,
        input_tokens,
    })
}

/// Strip markdown code fences from the response.
fn strip_markdown_fences(s: &str) -> String {
    let trimmed = s.trim();
    let trimmed = trimmed.strip_prefix("```json").unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix("```").unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed);
    trimmed.trim().to_string()
}

/// Parse the LLM JSON response into ClassificationResults, matched by fact_id.
fn parse_response(content: &str, facts: &[(&str, &str)]) -> Vec<ClassificationResult> {
    let stripped = strip_markdown_fences(content);

    #[derive(serde::Deserialize)]
    struct LlmClassification {
        fact_id: String,
        fact_type: String,
        confidence: f64,
    }

    let classifications: Vec<LlmClassification> = match serde_json::from_str(&stripped) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("WARN [classifier] JSON parse failed, using fallback");
            return fallback_all(facts);
        }
    };

    // Build a lookup map from fact_id → classification
    let lookup: std::collections::HashMap<&str, &LlmClassification> = classifications
        .iter()
        .map(|c| (c.fact_id.as_str(), c))
        .collect();

    let now = chrono::Utc::now().to_rfc3339();

    facts
        .iter()
        .map(|(id, body)| {
            if let Some(c) = lookup.get(id) {
                let fact_type = match c.fact_type.as_str() {
                    "durable" | "state" | "event" => c.fact_type.clone(),
                    unknown => {
                        eprintln!(
                            "WARN [classifier] unknown fact_type '{}' for {}, defaulting to durable",
                            unknown, id
                        );
                        "durable".to_string()
                    }
                };
                let confidence = c.confidence.clamp(0.0, 1.0) as f32;
                ClassificationResult {
                    fact_type,
                    confidence,
                    method: ClassificationMethod::Llm,
                    classified_at: Some(now.clone()),
                }
            } else {
                // Fact not in LLM response → fallback
                rule_classify("", body)
            }
        })
        .collect()
}

/// Return rule-based fallback results for all facts.
fn fallback_all(facts: &[(&str, &str)]) -> Vec<ClassificationResult> {
    facts.iter().map(|(_, body)| rule_classify("", body)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_valid_json_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": r#"[{"fact_id":"f1","fact_type":"event","confidence":0.95},{"fact_id":"f2","fact_type":"state","confidence":0.88}]"#}],
                    "usage": {"input_tokens": 100, "output_tokens": 50}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let facts = vec![
            ("f1", "We migrated the database"),
            ("f2", "API is currently limited"),
        ];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results = classify_batch_impl(
            &facts,
            "test-key",
            "claude-haiku-4-5-20251001",
            &mut budget,
            &url,
        )
        .await;

        mock.assert_async().await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact_type, "event");
        assert!((results[0].confidence - 0.95).abs() < 0.01);
        assert_eq!(results[1].fact_type, "state");
        assert!((results[1].confidence - 0.88).abs() < 0.01);
        assert_eq!(results[0].method, ClassificationMethod::Llm);
        assert_eq!(results[1].method, ClassificationMethod::Llm);
        // Budget should be decremented by input_tokens
        assert_eq!(budget, 9_900);
    }

    #[tokio::test]
    async fn test_malformed_json_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": "this is not json at all"}],
                    "usage": {"input_tokens": 50, "output_tokens": 10}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let facts = vec![("f1", "Some fact body")];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results =
            classify_batch_impl(&facts, "test-key", "test-model", &mut budget, &url).await;

        mock.assert_async().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].method, ClassificationMethod::Rules);
    }

    #[tokio::test]
    async fn test_http_500_fallback() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(500)
            .create_async()
            .await;

        let facts = vec![("f1", "Some fact")];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results =
            classify_batch_impl(&facts, "test-key", "test-model", &mut budget, &url).await;

        mock.assert_async().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].method, ClassificationMethod::Rules);
    }

    #[tokio::test]
    async fn test_http_429_retry_success() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut server = mockito::Server::new_async().await;

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let count_for_status = call_count.clone();
        let count_for_body = call_count.clone();

        let success_body = serde_json::json!({
            "content": [{"type": "text", "text": r#"[{"fact_id":"f1","fact_type":"durable","confidence":0.9}]"#}],
            "usage": {"input_tokens": 80, "output_tokens": 30}
        })
        .to_string();

        // First call → 429, second call → 200 with valid JSON
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status_code_from_request(move |_| {
                let n = count_for_status.fetch_add(1, Ordering::SeqCst);
                if n == 0 { 429 } else { 200 }
            })
            .with_body_from_request(move |_| {
                let n = count_for_body.load(Ordering::SeqCst);
                // call_count is already incremented by status callback, so n >= 1 on retry
                if n >= 2 { success_body.clone().into_bytes() } else { vec![] }
            })
            .expect(2)
            .create_async()
            .await;

        let facts = vec![("f1", "An architectural principle")];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results =
            classify_batch_impl(&facts, "test-key", "test-model", &mut budget, &url).await;

        // Confirms exactly 2 hits: initial 429 + successful retry
        mock.assert_async().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_type, "durable");
        assert_eq!(results[0].method, ClassificationMethod::Llm);
        assert!((results[0].confidence - 0.9).abs() < 0.01);
        // Budget decremented by input_tokens from the successful retry
        assert_eq!(budget, 10_000 - 80);
    }

    #[tokio::test]
    async fn test_http_429_retry_exhaustion() {
        let mut server = mockito::Server::new_async().await;

        // Both calls return 429 — mock expects exactly 2 hits (initial + retry)
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(429)
            .expect(2)
            .create_async()
            .await;

        let facts = vec![("f1", "An architectural principle")];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results =
            classify_batch_impl(&facts, "test-key", "test-model", &mut budget, &url).await;

        // Confirms exactly 2 hits: initial call + one retry
        mock.assert_async().await;
        assert_eq!(results.len(), 1);
        // After retry also fails, falls back to rule-based
        assert_eq!(results[0].method, ClassificationMethod::Rules);
        // Budget unchanged — no successful response to decrement from
        assert_eq!(budget, 10_000);
    }

    #[tokio::test]
    async fn test_empty_api_key_fallback() {
        let facts = vec![("f1", "Some fact")];
        let mut budget = 10_000u32;
        let results = classify_batch(&facts, "", "test-model", &mut budget).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].method, ClassificationMethod::Rules);
    }

    #[tokio::test]
    async fn test_budget_exhaustion_fallback() {
        let facts = vec![("f1", "Some fact")];
        let mut budget = 0u32;
        let results = classify_batch(&facts, "test-key", "test-model", &mut budget).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].method, ClassificationMethod::Rules);
    }

    #[tokio::test]
    async fn test_unknown_fact_type_defaults_to_durable() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": r#"[{"fact_id":"f1","fact_type":"unknown_type","confidence":0.8}]"#}],
                    "usage": {"input_tokens": 50, "output_tokens": 20}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let facts = vec![("f1", "Some fact")];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results =
            classify_batch_impl(&facts, "test-key", "test-model", &mut budget, &url).await;

        mock.assert_async().await;
        assert_eq!(results[0].fact_type, "durable");
        assert_eq!(results[0].method, ClassificationMethod::Llm);
    }

    #[tokio::test]
    async fn test_confidence_clamping() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": r#"[{"fact_id":"f1","fact_type":"event","confidence":1.5}]"#}],
                    "usage": {"input_tokens": 50, "output_tokens": 20}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let facts = vec![("f1", "Some event")];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results =
            classify_batch_impl(&facts, "test-key", "test-model", &mut budget, &url).await;

        mock.assert_async().await;
        assert!(
            (results[0].confidence - 1.0).abs() < f32::EPSILON,
            "confidence should be clamped to 1.0, got: {}",
            results[0].confidence
        );
    }

    #[tokio::test]
    async fn test_markdown_fence_stripping() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": "```json\n[{\"fact_id\":\"f1\",\"fact_type\":\"state\",\"confidence\":0.9}]\n```"}],
                    "usage": {"input_tokens": 60, "output_tokens": 25}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let facts = vec![("f1", "API is rate limited")];
        let mut budget = 10_000u32;
        let url = format!("{}/v1/messages", server.url());
        let results =
            classify_batch_impl(&facts, "test-key", "test-model", &mut budget, &url).await;

        mock.assert_async().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_type, "state");
        assert_eq!(results[0].method, ClassificationMethod::Llm);
    }
}
