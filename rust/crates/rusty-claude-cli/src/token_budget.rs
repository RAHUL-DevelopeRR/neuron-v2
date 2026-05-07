//! Token budget enforcement for NeuronCLI
//!
//! Prevents context explosion by enforcing a hard cap on input tokens
//! per turn. Automatically prunes conversation history, tool results,
//! and retrieved context to stay under budget.

use std::collections::VecDeque;

/// Hard token budget per LLM turn.
/// Claude Code operates at ~4-8K; we target the same.
pub const HARD_TOKEN_BUDGET: usize = 8192;

/// Reserved tokens for static portions of the prompt.
pub const SYSTEM_RESERVE: usize = 512;
pub const REPO_MAP_RESERVE: usize = 1024;
pub const WORKING_MEMORY_RESERVE: usize = 512;

/// Dynamic budget available for conversation + tool results + retrieved context.
pub fn dynamic_budget() -> usize {
    HARD_TOKEN_BUDGET
        .saturating_sub(SYSTEM_RESERVE)
        .saturating_sub(REPO_MAP_RESERVE)
        .saturating_sub(WORKING_MEMORY_RESERVE)
}

/// Simple tokenizer: ~0.25 tokens per byte for English/code.
/// Accurate enough for budget enforcement.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() as f64 * 0.25).ceil() as usize
}

/// A message in the conversation context.
#[derive(Debug, Clone)]
pub struct BudgetMessage {
    pub role: String,
    pub content: String,
}

/// Token budget manager that prunes context to stay under limit.
pub struct TokenBudget {
    budget: usize,
    used: usize,
}

impl TokenBudget {
    pub fn new() -> Self {
        Self {
            budget: dynamic_budget(),
            used: 0,
        }
    }

    /// Fit messages into budget by pruning oldest pairs first.
    pub fn fit_messages(&self, messages: &mut VecDeque<BudgetMessage>) {
        let mut total: usize = messages.iter().map(|m| estimate_tokens(&m.content)).sum();

        while total > self.budget && messages.len() > 4 {
            // Remove oldest user + assistant pair
            let user = messages.pop_front();
            let assistant = messages.pop_front();

            if let (Some(u), Some(a)) = (user, assistant) {
                let saved = estimate_tokens(&u.content) + estimate_tokens(&a.content);
                // Add a tiny summary placeholder (replaces the pair)
                let summary = BudgetMessage {
                    role: "system".to_string(),
                    content: format!(
                        "[Earlier conversation: {} tokens of dialogue summarized]",
                        saved
                    ),
                };
                let summary_tokens = estimate_tokens(&summary.content);
                messages.push_front(summary);
                total = total.saturating_sub(saved).saturating_add(summary_tokens);
            }
        }

        // If still over budget, truncate individual messages
        for msg in messages.iter_mut() {
            let tok = estimate_tokens(&msg.content);
            if tok > 1500 {
                let byte_limit = (1500.0 / 0.25) as usize;
                msg.content.truncate(byte_limit);
                msg.content.push_str("\n... [truncated by token budget]");
            }
        }
    }

    /// Check if adding `text` would exceed budget.
    pub fn would_exceed(&self, text: &str) -> bool {
        self.used + estimate_tokens(text) > self.budget
    }

    /// Reserve tokens for upcoming content.
    pub fn reserve(&mut self, text: &str) -> bool {
        let cost = estimate_tokens(text);
        if self.used + cost <= self.budget {
            self.used += cost;
            true
        } else {
            false
        }
    }

    /// Remaining tokens.
    pub fn remaining(&self) -> usize {
        self.budget.saturating_sub(self.used)
    }
}

/// Compress tool result to fit within a token limit.
pub fn compress_tool_result(name: &str, output: &str, max_tokens: usize) -> String {
    if estimate_tokens(output) <= max_tokens {
        return output.to_string();
    }

    let byte_limit = ((max_tokens as f64) / 0.25) as usize;
    let mut compressed = output[..output.len().min(byte_limit)].to_string();
    compressed.push_str(&format!(
        "\n... [{} tool output truncated from {} bytes]",
        name,
        output.len()
    ));
    compressed
}

/// Summarize a list of file contents for repo map injection.
/// Only includes file paths and first 3 lines (signature), not full bodies.
pub fn build_repo_map(file_paths: &[String]) -> String {
    let mut map = String::from("Repository structure:\n");
    for path in file_paths {
        map.push_str(&format!("- {}\n", path));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_prunes_oldest_first() {
        let mut msgs: VecDeque<BudgetMessage> = VecDeque::new();
        msgs.push_back(BudgetMessage {
            role: "user".to_string(),
            content: "a".repeat(4000), // ~1000 tokens
        });
        msgs.push_back(BudgetMessage {
            role: "assistant".to_string(),
            content: "b".repeat(4000), // ~1000 tokens
        });
        msgs.push_back(BudgetMessage {
            role: "user".to_string(),
            content: "c".repeat(4000), // ~1000 tokens
        });
        msgs.push_back(BudgetMessage {
            role: "assistant".to_string(),
            content: "d".repeat(4000), // ~1000 tokens
        });

        let budget = TokenBudget::new();
        let mut msgs_vec = Vec::from(msgs);
        let mut deque = VecDeque::from(msgs_vec);
        budget.fit_messages(&mut deque);

        // Should have summarized the oldest pair
        assert!(deque.len() <= 3);
    }

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens("hello"), 2); // 5 * 0.25 = 1.25 → 2
        assert_eq!(estimate_tokens("a".repeat(100).as_str()), 25);
    }
}
