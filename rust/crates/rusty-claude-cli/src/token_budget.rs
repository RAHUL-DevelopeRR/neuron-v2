//! Token estimation helpers for prompt assembly.
//!
//! There is deliberately no fixed per-turn token ceiling here. Input budgets
//! are model-aware and enforced by `runtime::context_manager` before every
//! model call: the context window from the shared model registry minus the
//! output reserve, the tool schemas and the system prompt.

use runtime::context_manager::{estimate_text_tokens, truncate_head_tail, ASCII_CHARS_PER_TOKEN};

/// Estimates the token count of `text` (UTF-8 aware; see
/// [`runtime::context_manager::estimate_text_tokens`]).
pub fn estimate_tokens(text: &str) -> usize {
    estimate_text_tokens(text)
}

/// Shortens a tool result to roughly `max_tokens`, keeping its head and tail.
/// Never splits a multi-byte character.
pub fn compress_tool_result(name: &str, output: &str, max_tokens: usize) -> String {
    if estimate_tokens(output) <= max_tokens {
        return output.to_string();
    }
    // ASCII_CHARS_PER_TOKEN characters per token; non-ASCII text is truncated further
    // below if the first cut is still over budget.
    let mut compressed =
        truncate_head_tail(output, max_tokens.saturating_mul(ASCII_CHARS_PER_TOKEN));
    if estimate_tokens(&compressed) > max_tokens {
        compressed = truncate_head_tail(output, max_tokens);
    }
    compressed.push_str(&format!(
        "\n... [{name} output shortened from {} chars]",
        output.chars().count()
    ));
    compressed
}

/// Summarize a list of file contents for repo map injection.
/// Only includes file paths, not bodies.
pub fn build_repo_map(file_paths: &[String]) -> String {
    let mut map = String::from("Repository structure:\n");
    for path in file_paths {
        map.push_str(&format!("- {path}\n"));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_counts_ascii_and_unicode() {
        assert_eq!(estimate_tokens("hello"), 2);
        assert_eq!(estimate_tokens("a".repeat(99).as_str()), 33);
        assert_eq!(estimate_tokens("日本"), 2);
    }

    #[test]
    fn compress_tool_result_is_char_boundary_safe() {
        // A byte-based cut through these multi-byte characters used to panic.
        let output = "é🙂".repeat(10_000);
        let compressed = compress_tool_result("read_file", &output, 100);
        assert!(compressed.contains("chars omitted"));
        assert!(compressed.contains("read_file output shortened from 20000 chars"));
        assert!(estimate_tokens(&compressed) < 200);

        let short = "fits";
        assert_eq!(compress_tool_result("bash", short, 100), short);
    }

    #[test]
    fn has_no_fixed_input_ceiling() {
        // A large tool result within its own budget is returned untouched.
        let output = "x".repeat(100_000 * ASCII_CHARS_PER_TOKEN);
        assert_eq!(compress_tool_result("bash", &output, 100_000), output);
    }
}
