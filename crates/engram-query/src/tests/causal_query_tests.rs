use crate::causal_query::{is_causal_query, CACHE_TIER_CAUSAL};

// ─── Trigger detection ──────────────────────────────────────────────────

#[test]
fn test_is_causal_all_signals() {
    let signals = [
        "caused by",
        "depends on",
        "enables",
        "led to",
        "because",
        "therefore",
        "chain",
        "upstream",
        "downstream",
        "root cause",
        "consequence of",
    ];
    for signal in &signals {
        let query = format!("what {} the outage", signal);
        assert!(
            is_causal_query(&query),
            "should trigger on signal: {:?}",
            signal
        );
    }
}

#[test]
fn test_is_causal_negative_queries() {
    assert!(!is_causal_query("what is the retry policy"));
    assert!(!is_causal_query("explain authentication flow"));
    assert!(!is_causal_query("how does the cache work"));
    assert!(!is_causal_query("what are the deployment steps"));
}

#[test]
fn test_is_causal_no_false_positive_led_the_team() {
    // "led to" is the signal, not "led"
    assert!(!is_causal_query("I led the team"));
}

#[test]
fn test_is_causal_no_false_positive_chain_partial() {
    // "chain" as substring should match
    assert!(is_causal_query("show the chain of events"));
    // "blockchain" contains "chain"
    assert!(is_causal_query("blockchain consensus"));
}

#[test]
fn test_cache_tier_causal_value() {
    assert_eq!(CACHE_TIER_CAUSAL, 24);
}
