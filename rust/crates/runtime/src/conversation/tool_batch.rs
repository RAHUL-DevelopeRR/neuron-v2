//! Read-only tool calls of one response that run at the same time. The rule for
//! which calls qualify is in [`crate::parallel_tools`].

use super::{
    merge_hook_feedback, tool_input_object, watch_tool_result, ApiClient, ConversationMessage,
    ConversationRuntime, LoopGuard, LoopVerdict, RuntimeError, ToolError, ToolExecutor,
    TurnProgress,
};
use crate::parallel_tools::{is_concurrency_safe_tool, ConcurrentToolCall};
use crate::permissions::{PermissionMode, PermissionOutcome};

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Runs the leading calls of `calls` together when two or more of them in a row are
    /// read-only calls nobody has to approve, and records their results in order.
    /// Returns how many calls it ran: 0 when the first call has to run alone, the
    /// usual way.
    pub(super) fn run_read_only_batch(
        &mut self,
        calls: &[(String, String, String)],
        loop_guard: &mut LoopGuard,
        progress: &mut TurnProgress,
    ) -> Result<usize, RuntimeError> {
        if calls.len() < 2
            || !self.tool_executor.supports_concurrent_calls()
            || self.hook_runner.has_pre_tool_use_hooks()
            || self.cancel.is_cancelled()
        {
            return Ok(0);
        }
        // The loop guard sees the calls in order, as when they run one by one; a call
        // it would warn about or stop runs alone.
        let mut trial = loop_guard.clone();
        let count = calls
            .iter()
            .take_while(|(_, name, input)| {
                self.runs_unattended_read_only(name, input)
                    && trial.observe(name, input) == LoopVerdict::Proceed
            })
            .count();
        if count < 2 {
            return Ok(0);
        }
        let batch = calls[..count]
            .iter()
            .map(|(id, name, input)| {
                loop_guard.observe(name, input);
                ConcurrentToolCall {
                    tool_use_id: id.clone(),
                    tool_name: name.clone(),
                    input: input.clone(),
                }
            })
            .collect::<Vec<_>>();

        let iteration = progress.iterations;
        for call in &batch {
            self.record_tool_started(iteration, &call.tool_name);
            self.tool_executor
                .tool_started(&call.tool_use_id, &call.tool_name, &call.input);
        }
        let mut results = self
            .tool_executor
            .execute_concurrently(&batch, &self.cancel)
            .into_iter();
        for call in &batch {
            let result = results.next().unwrap_or_else(|| {
                Err(ToolError::new(
                    "the tool executor returned no result for this call",
                ))
            });
            let (output, is_error) =
                self.after_tool_ran(&call.tool_use_id, &call.tool_name, &call.input, &[], result);
            let mut message = ConversationMessage::tool_result(
                call.tool_use_id.clone(),
                call.tool_name.clone(),
                self.context_manager.cap_tool_result(output),
                is_error,
            );
            // The whole batch already ran: a pause takes effect after it.
            watch_tool_result(
                &mut progress.watch,
                &call.tool_name,
                &call.input,
                &mut message,
            );
            self.record_tool_result(iteration, message, progress)?;
        }
        Ok(count)
    }

    /// Whether a call may join a batch: a concurrency-safe tool, valid arguments, only
    /// read access needed for this input, and allowed by the policy without a prompt.
    fn runs_unattended_read_only(&self, tool_name: &str, input: &str) -> bool {
        is_concurrency_safe_tool(tool_name)
            && tool_input_object(input).is_ok()
            && self
                .permission_policy
                .required_mode_for_input(tool_name, input)
                == PermissionMode::ReadOnly
            && self.permission_policy.authorize(tool_name, input, None) == PermissionOutcome::Allow
    }

    /// A call that ran: its result with the pre-hook's feedback, then the post-hook
    /// (which may block it and add its own).
    pub(super) fn after_tool_ran(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        input: &str,
        pre_hook_messages: &[String],
        result: Result<String, ToolError>,
    ) -> (String, bool) {
        let (output, mut is_error) = match result {
            Ok(output) => (output, false),
            Err(error) => (error.to_string(), true),
        };
        let output = merge_hook_feedback(pre_hook_messages, output, false);
        let post_hook_result = if is_error {
            self.run_post_tool_use_failure_hook(tool_use_id, tool_name, input, &output)
        } else {
            self.run_post_tool_use_hook(tool_use_id, tool_name, input, &output, false)
        };
        let post_hook_blocked = post_hook_result.is_denied()
            || post_hook_result.is_failed()
            || post_hook_result.is_cancelled();
        if post_hook_blocked {
            is_error = true;
        }
        (
            merge_hook_feedback(post_hook_result.messages(), output, post_hook_blocked),
            is_error,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError, ToolError,
        ToolExecutor,
    };
    use crate::cancel::CancellationToken;
    use crate::parallel_tools::{run_concurrently, ConcurrentToolCall, MAX_CONCURRENT_TOOL_CALLS};
    use crate::permissions::{PermissionMode, PermissionPolicy};
    use crate::session::{ContentBlock, Session};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// One response with `calls`, then a closing answer.
    struct TwoResponses {
        calls: Vec<(String, String, String)>,
        sent: usize,
    }

    impl ApiClient for TwoResponses {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.sent += 1;
            if self.sent == 1 {
                let mut events = self
                    .calls
                    .iter()
                    .map(|(id, name, input)| AssistantEvent::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    })
                    .collect::<Vec<_>>();
                events.push(AssistantEvent::MessageStop);
                Ok(events)
            } else {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }
    }

    type Log = Arc<Mutex<Vec<String>>>;

    /// Runs every call by sleeping a little (later calls of a batch less, so they end
    /// first) and logs what happens, in order.
    struct MockTools {
        concurrent: bool,
        log: Log,
        batches: Vec<Vec<String>>,
        running: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl MockTools {
        fn new(concurrent: bool) -> Self {
            Self {
                concurrent,
                log: Log::default(),
                batches: Vec::new(),
                running: Arc::default(),
                peak: Arc::default(),
            }
        }
    }

    fn run_one(
        log: &Log,
        running: &AtomicUsize,
        peak: &AtomicUsize,
        id: &str,
        delay_ms: u64,
    ) -> Result<String, ToolError> {
        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
        log.lock().unwrap().push(format!("start {id}"));
        std::thread::sleep(Duration::from_millis(delay_ms));
        log.lock().unwrap().push(format!("end {id}"));
        running.fetch_sub(1, Ordering::SeqCst);
        Ok(format!("result {id}"))
    }

    fn id_of(input: &str) -> String {
        serde_json::from_str::<serde_json::Value>(input).unwrap()["path"]
            .as_str()
            .unwrap()
            .to_string()
    }

    impl ToolExecutor for MockTools {
        fn execute(&mut self, _tool_name: &str, input: &str) -> Result<String, ToolError> {
            run_one(&self.log, &self.running, &self.peak, &id_of(input), 5)
        }

        fn tool_started(&mut self, id: &str, _name: &str, _input: &str) {
            self.log.lock().unwrap().push(format!("started {id}"));
        }

        fn tool_call_finished(&mut self, id: &str, _name: &str, _output: &str, _is_error: bool) {
            self.log.lock().unwrap().push(format!("finished {id}"));
        }

        fn supports_concurrent_calls(&self) -> bool {
            self.concurrent
        }

        fn execute_concurrently(
            &mut self,
            calls: &[ConcurrentToolCall],
            _cancel: &CancellationToken,
        ) -> Vec<Result<String, ToolError>> {
            self.batches
                .push(calls.iter().map(|call| id_of(&call.input)).collect());
            let (log, running, peak) = (&self.log, &*self.running, &*self.peak);
            let count = calls.len() as u64;
            let indexed = calls.iter().enumerate().collect::<Vec<_>>();
            run_concurrently(&indexed, MAX_CONCURRENT_TOOL_CALLS, |(index, call)| {
                let delay = 20 * (count - *index as u64);
                run_one(log, running, peak, &id_of(&call.input), delay)
            })
        }
    }

    fn call(id: &str, name: &str) -> (String, String, String) {
        (
            id.to_string(),
            name.to_string(),
            format!(r#"{{"path":"{id}"}}"#),
        )
    }

    fn run(
        tools: MockTools,
        calls: Vec<(String, String, String)>,
    ) -> ConversationRuntime<TwoResponses, MockTools> {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly)
            .with_tool_requirement("grep_search", PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite);
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            TwoResponses { calls, sent: 0 },
            tools,
            policy,
            vec!["system".to_string()],
        );
        runtime.run_turn("go", None).expect("the turn runs");
        runtime
    }

    fn result_ids(runtime: &ConversationRuntime<TwoResponses, MockTools>) -> Vec<String> {
        runtime
            .session()
            .messages
            .iter()
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    ..
                } => Some(format!("{tool_use_id}={output}")),
                _ => None,
            })
            .collect()
    }

    fn position(log: &[String], entry: &str) -> usize {
        log.iter()
            .position(|line| line == entry)
            .unwrap_or_else(|| panic!("{entry} missing from {log:?}"))
    }

    /// Reads in a row run at the same time; their results go to the transcript in the
    /// order the model asked, and each call is started before it runs and finished
    /// after, as one by one.
    #[test]
    fn reads_run_together_and_results_keep_their_order() {
        let runtime = run(
            MockTools::new(true),
            vec![
                call("a", "read_file"),
                call("b", "grep_search"),
                call("c", "read_file"),
            ],
        );
        assert_eq!(
            result_ids(&runtime),
            vec!["a=result a", "b=result b", "c=result c"]
        );
        let tools = &runtime.tool_executor;
        assert_eq!(tools.batches, vec![vec!["a", "b", "c"]]);
        assert!(tools.peak.load(Ordering::SeqCst) > 1, "ran one at a time");
        let log = tools.log.lock().unwrap().clone();
        // The last call ends first, yet results are finished in order.
        assert!(position(&log, "end c") < position(&log, "end a"), "{log:?}");
        for id in ["a", "b", "c"] {
            assert!(
                position(&log, &format!("started {id}")) < position(&log, &format!("start {id}"))
            );
            assert!(
                position(&log, &format!("end {id}")) < position(&log, &format!("finished {id}"))
            );
        }
        assert!(position(&log, "finished a") < position(&log, "finished b"));
        assert!(position(&log, "finished b") < position(&log, "finished c"));
    }

    /// A write splits the reads around it: the reads before it run together and end
    /// before it starts, it runs alone, and the reads after it start once it ended.
    #[test]
    fn a_write_runs_alone_between_the_batches() {
        let runtime = run(
            MockTools::new(true),
            vec![
                call("a", "read_file"),
                call("b", "read_file"),
                call("w", "write_file"),
                call("c", "read_file"),
                call("d", "grep_search"),
            ],
        );
        assert_eq!(
            result_ids(&runtime),
            vec![
                "a=result a",
                "b=result b",
                "w=result w",
                "c=result c",
                "d=result d"
            ]
        );
        let tools = &runtime.tool_executor;
        assert_eq!(tools.batches, vec![vec!["a", "b"], vec!["c", "d"]]);
        let log = tools.log.lock().unwrap().clone();
        assert!(position(&log, "end a") < position(&log, "start w"));
        assert!(position(&log, "end b") < position(&log, "start w"));
        assert!(position(&log, "end w") < position(&log, "start c"));
        assert!(position(&log, "end w") < position(&log, "start d"));
    }

    /// Calls that need approval (a credential file), a lone read, and executors that
    /// cannot run calls together all take the one-by-one path.
    #[test]
    fn approval_lone_reads_and_plain_executors_stay_sequential() {
        let runtime = run(
            MockTools::new(true),
            vec![
                call("a", "read_file"),
                call(".env", "read_file"),
                call("c", "read_file"),
            ],
        );
        assert_eq!(
            runtime.tool_executor.batches,
            [] as [std::vec::Vec<std::string::String>; 0]
        );
        assert_eq!(runtime.tool_executor.peak.load(Ordering::SeqCst), 1);

        let runtime = run(
            MockTools::new(true),
            vec![call("a", "read_file"), call("w", "write_file")],
        );
        assert_eq!(
            runtime.tool_executor.batches,
            [] as [std::vec::Vec<std::string::String>; 0]
        );

        let runtime = run(
            MockTools::new(false),
            vec![call("a", "read_file"), call("b", "read_file")],
        );
        assert_eq!(
            runtime.tool_executor.batches,
            [] as [std::vec::Vec<std::string::String>; 0]
        );
        assert_eq!(runtime.tool_executor.peak.load(Ordering::SeqCst), 1);
        assert_eq!(result_ids(&runtime), vec!["a=result a", "b=result b"]);
    }
}
