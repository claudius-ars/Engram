use engram_query::QueryResult;

use crate::EnrichOptions;

pub fn format_context_block(result: &QueryResult, options: &EnrichOptions) -> String {
    let mut out = String::new();

    out.push_str("## Engram Context (Auto-Enriched)\n\n");
    out.push_str("<!-- engram:start -->\n");

    if result.hits.is_empty() {
        out.push_str("_No relevant facts found for this task._\n");
        out.push_str("<!-- engram:end -->");
        return out;
    }

    for (i, hit) in result.hits.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }

        let heading = hit
            .title
            .as_deref()
            .unwrap_or(&hit.id);
        out.push_str(&format!("### {}\n", heading));
        out.push_str(&format!(
            "**Type:** {} | **Confidence:** {:.2}\n",
            hit.fact_type, hit.confidence
        ));

        if !hit.domain_tags.is_empty() {
            out.push_str(&format!("**Tags:** {}\n", hit.domain_tags.join(", ")));
        }

        if !hit.keywords.is_empty() {
            out.push_str(&format!("**Keywords:** {}\n", hit.keywords.join(", ")));
        }

        if !hit.related.is_empty() {
            out.push_str(&format!("**Related:** {}\n", hit.related.join(", ")));
        }

        if !hit.caused_by.is_empty() {
            out.push_str(&format!("**Caused by:** {}\n", hit.caused_by.join(", ")));
        }

        if options.include_metadata {
            out.push_str(&format!(
                "**Score:** {:.3} | **Tier:** {} | **Gen:** {}\n",
                hit.score, result.meta.cache_tier, result.meta.index_generation
            ));
            if hit.maturity < 1.0 {
                out.push_str(&format!("**Maturity:** {:.2}\n", hit.maturity));
            }
            if hit.access_count > 0 {
                out.push_str(&format!("**Accessed:** {} times\n", hit.access_count));
            }
            if hit.update_count > 0 {
                out.push_str(&format!("**Updated:** {} times\n", hit.update_count));
            }
        }

        out.push('\n');
        out.push_str(&format!("_Source: {}_\n", hit.source_path));
    }

    out.push_str("<!-- engram:end -->");

    out
}
