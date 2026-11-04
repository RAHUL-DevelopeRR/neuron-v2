//! Running several read-only tool calls of one model response at once.
//!
//! A response often asks for a handful of reads (files, searches, fetches) before the
//! model can go on. The runtime runs a run of such calls together when every one of
//! them is safe to overlap; see `ConversationRuntime`'s batch rule:
//!
//! - the tool is one of [`CONCURRENCY_SAFE_TOOLS`], and the permission policy needs
//!   only read-only access for this very input (so a read of a credential file, a
//!   search outside the workspace or a fetch from an unapproved domain is out);
//! - the policy allows it without asking anyone, no `PreToolUse` hook is configured
//!   (a hook may rewrite the input or ask), and the loop guard has nothing to say;
//! - the executor can run calls concurrently ([`crate::ToolExecutor::supports_concurrent_calls`]).
//!
//! Calls are taken in the response's order: each maximal run of consecutive eligible
//! calls (two or more) runs together, at most [`MAX_CONCURRENT_TOOL_CALLS`] at a time,
//! and its results are recorded in the original order. Every other call (writes,
//! shells, sub-agents, anything that needs approval) runs alone, in order, as before;
//! a read after a write therefore still sees the write.
//!
//! The safe list is the intersection of what the tool metadata says (read-only) and
//! what the tool does: several tools are marked read-only but run processes or talk
//! to the user (`bash`, `PowerShell`, `AskUserQuestion`, `Sleep`, ...), so the
//! metadata alone cannot decide.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// How many calls of a batch run at the same time, at most.
pub const MAX_CONCURRENT_TOOL_CALLS: usize = 4;

/// Tools that only read and may overlap with each other: they keep no state between
/// calls that another call of the batch could see half-done.
pub const CONCURRENCY_SAFE_TOOLS: &[&str] = &[
    "read_file",
    "glob_search",
    "grep_search",
    "WebFetch",
    "WebSearch",
    "ToolSearch",
];

/// Whether calls of `tool_name` may run together with other such calls.
#[must_use]
pub fn is_concurrency_safe_tool(tool_name: &str) -> bool {
    CONCURRENCY_SAFE_TOOLS.contains(&tool_name)
}

/// One call of a batch, as the executor gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcurrentToolCall {
    pub tool_use_id: String,
    pub tool_name: String,
    /// The call's input as the model wrote it (JSON text).
    pub input: String,
}

/// Runs `run` over `items` on up to `limit` scoped threads and returns the results in
/// the order of `items`. With one item or a limit of one it runs on this thread.
pub fn run_concurrently<T, R>(items: &[T], limit: usize, run: impl Fn(&T) -> R + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let workers = limit.min(items.len());
    if workers <= 1 {
        return items.iter().map(run).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::SeqCst);
                let Some(item) = items.get(index) else {
                    break;
                };
                let result = run(item);
                results
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)[index] = Some(result);
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .into_iter()
        .map(|result| result.expect("every item ran"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{is_concurrency_safe_tool, run_concurrently, MAX_CONCURRENT_TOOL_CALLS};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn results_keep_the_order_of_the_items_and_the_limit_holds() {
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let items: Vec<u64> = (0..10).collect();
        let results = run_concurrently(&items, MAX_CONCURRENT_TOOL_CALLS, |item| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            // Later items finish first.
            std::thread::sleep(Duration::from_millis(40 - item * 3));
            running.fetch_sub(1, Ordering::SeqCst);
            item * 10
        });
        assert_eq!(results, (0..10).map(|item| item * 10).collect::<Vec<_>>());
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak > 1, "ran one at a time");
        assert!(peak <= MAX_CONCURRENT_TOOL_CALLS, "peak {peak}");
    }

    #[test]
    fn one_item_runs_on_this_thread() {
        let here = std::thread::current().id();
        let ran_on = run_concurrently(&[1], 4, |_| std::thread::current().id());
        assert_eq!(ran_on, vec![here]);
    }

    #[test]
    fn only_pure_reads_are_safe() {
        for tool in [
            "read_file",
            "grep_search",
            "glob_search",
            "WebFetch",
            "ToolSearch",
        ] {
            assert!(is_concurrency_safe_tool(tool), "{tool}");
        }
        for tool in [
            "bash",
            "PowerShell",
            "write_file",
            "edit_file",
            "Agent",
            "AskUserQuestion",
            "TodoWrite",
            "Sleep",
        ] {
            assert!(!is_concurrency_safe_tool(tool), "{tool}");
        }
    }
}
