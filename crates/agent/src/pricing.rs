//! Display-only list-price estimates, verified 2026-11-03. Never used for billing.
//! Sources: https://developers.openai.com/api/docs/pricing
//! https://platform.claude.com/docs/en/about-claude/pricing
//! Codex rates: https://help.openai.com/en/articles/20001415-chatgpt-rate-card-enterprise-token-based-pricing
//! Context: https://platform.claude.com/docs/en/build-with-claude/context-windows
//! Unknown models and mutable aliases deliberately have no inferred price/capacity.
fn canonical(model: &str) -> &str {
    model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .trim_end_matches("[1m]")
}

/// Input excludes cache reads/writes; output includes reasoning tokens.
pub fn estimate(model: &str, input: u64, cached: u64, written: u64, output: u64) -> Option<f64> {
    let model = canonical(model);
    let (i, c, w, o) = match model {
        "gpt-6-sol" => (2., 0.2, 0., 10.),
        "gpt-5.6-sol" => (4., 0.4, 0., 20.),
        "gpt-5.6-terra" => (2., 0.2, 0., 12.),
        "gpt-5.6-luna" => (0.2, 0.02, 0., 1.2),
        "gpt-6-astra" => (10., 1., 12.5, 50.),
        "gpt-6.1-sol" => (2., 0.1, 2.5, 10.),
        "gpt-6.1-terra" => (1., 0.05, 1.25, 5.),
        "gpt-6-luna" => (0.1, 0.01, 0.125, 0.5),
        "gpt-5.2" | "gpt-5.2-codex" | "gpt-5.3-codex" => (1.75, 0.175, 0., 14.),
        "claude-opus-5-5" => (4., 0.2, 5., 20.),
        "claude-sonnet-5-5" => (2., 0.1, 2.5, 10.),
        "claude-haiku-5-5" if input.saturating_add(cached).saturating_add(written) <= 100_000 => {
            (0.1, 0.01, 0.125, 0.5)
        }
        "claude-haiku-5-5" => (0.5, 0.05, 0.625, 2.5),
        "claude-fable-5-1" | "claude-mythos-5-1" => (10., 0.25, 12.5, 50.),
        "claude-opus-5"
        | "claude-opus-4-8"
        | "claude-opus-4-7"
        | "claude-opus-4-6"
        | "claude-opus-4-5"
        | "claude-opus-4-5-20251101" => (5., 0.5, 6.25, 25.),
        "claude-sonnet-5" => (2., 0.2, 2.5, 10.),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" => {
            (3., 0.3, 3.75, 15.)
        }
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => (1., 0.1, 1.25, 5.),
        _ => return None,
    };
    Some(
        (input as f64 * i + cached as f64 * c + written as f64 * w + output as f64 * o)
            / 1_000_000.,
    )
}

pub fn context_window(model: &str) -> Option<u64> {
    match canonical(model) {
        "claude-opus-5-5" | "claude-sonnet-5-5" | "claude-haiku-5-5" | "claude-opus-5"
        | "claude-sonnet-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6"
        | "claude-sonnet-4-6" | "claude-fable-5-1" | "claude-mythos-5-1" => Some(1_000_000),
        "claude-opus-4-5"
        | "claude-opus-4-5-20251101"
        | "claude-sonnet-4-5"
        | "claude-sonnet-4-5-20250929"
        | "claude-haiku-4-5"
        | "claude-haiku-4-5-20251001" => Some(if model.ends_with("[1m]") {
            1_000_000
        } else {
            200_000
        }),
        _ => None,
    }
}

/// Namespace a billable scope by native conversation segment. Hex encoding keeps
/// opaque session IDs from changing the scope grammar.
pub fn cost_scope(session: Option<&str>, scope: String) -> String {
    match session {
        Some(id) => format!(
            "v2:{}:{scope}",
            id.bytes().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        None => scope,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn estimates_cache_separately_and_never_guesses_unknown_models() {
        assert_eq!(
            estimate("gpt-5.3-codex", 1_000_000, 1_000_000, 0, 1_000_000),
            Some(15.925)
        );
        assert_eq!(estimate("default", 100, 0, 0, 10), None);
        assert_eq!(context_window("claude-haiku-5-5"), Some(1_000_000));
        assert_eq!(context_window("opus"), None);
        assert_eq!(estimate("claude-haiku-5-5", 100_000, 0, 0, 0), Some(0.01));
        assert!(estimate("claude-haiku-5-5", 100_001, 0, 0, 0).unwrap() > 0.05);
    }
}
