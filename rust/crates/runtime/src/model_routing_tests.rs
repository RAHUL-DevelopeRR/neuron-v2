use std::cell::RefCell;
use std::rc::Rc;

use super::ModelRouting;
use crate::conversation::{
    ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, HarnessConfig, RuntimeError,
    RuntimeErrorKind, StaticToolExecutor,
};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, MessageRole, Session};

type Response = Result<Vec<AssistantEvent>, RuntimeError>;

/// What the mock client saw: `Request(model)` per model call (the model it would
/// send to) and `Override(model)` per switch.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Request(String),
    Override(Option<String>),
}

struct RoutingApi {
    responses: Vec<Response>,
    calls: Rc<RefCell<Vec<Call>>>,
    model: String,
    can_switch: bool,
}

impl ApiClient for RoutingApi {
    fn stream(&mut self, _request: ApiRequest) -> Response {
        self.calls
            .borrow_mut()
            .push(Call::Request(self.model.clone()));
        assert!(!self.responses.is_empty(), "unexpected extra model call");
        self.responses.remove(0)
    }

    fn set_model_override(&mut self, model: Option<&str>) -> bool {
        if !self.can_switch {
            return false;
        }
        self.calls
            .borrow_mut()
            .push(Call::Override(model.map(str::to_string)));
        self.model = model.unwrap_or("auto").to_string();
        true
    }
}

fn chat(value: &str) -> Response {
    Ok(vec![
        AssistantEvent::TextDelta(value.to_string()),
        AssistantEvent::MessageStop,
    ])
}

fn bash_call() -> Response {
    Ok(vec![
        AssistantEvent::ToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: r#"{"command":"echo hi > index.html"}"#.to_string(),
        },
        AssistantEvent::MessageStop,
    ])
}

fn request(model: &str) -> Call {
    Call::Request(model.to_string())
}

fn routing() -> ModelRouting {
    ModelRouting {
        agent_mode: true,
        tool_retry_models: vec!["nemotron-3-ultra".to_string()],
        fallbacks: vec!["backup-model".to_string()],
    }
}

type Runtime = ConversationRuntime<RoutingApi, StaticToolExecutor>;

fn runtime(
    responses: Vec<Response>,
    mode: PermissionMode,
    routing: ModelRouting,
    can_switch: bool,
) -> (Runtime, Rc<RefCell<Vec<Call>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let api = RoutingApi {
        responses,
        calls: Rc::clone(&calls),
        model: "auto".to_string(),
        can_switch,
    };
    let tools = StaticToolExecutor::new().register("bash", |_| Ok("ok".to_string()));
    let harness = HarnessConfig {
        action_nudge: true,
        ..HarnessConfig::default()
    }
    .with_model("auto");
    let runtime = ConversationRuntime::new(
        Session::new(),
        api,
        tools,
        PermissionPolicy::new(mode),
        vec!["system".to_string()],
    )
    .with_harness_config(harness)
    .with_model_routing(routing);
    (runtime, calls)
}

/// The user-role texts after the prompt (the notes the runtime added).
fn notes(session: &Session) -> Vec<String> {
    session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .skip(1)
        .collect()
}

#[test]
fn a_chat_only_answer_to_a_build_request_is_nudged_then_retried_on_a_tool_model() {
    let (mut runtime, calls) = runtime(
        vec![
            chat("No tool needed. Here is the HTML:\n<html>...</html>"),
            chat("As shown above, paste it into index.html."),
            bash_call(),
            chat("Created index.html."),
        ],
        PermissionMode::WorkspaceWrite,
        routing(),
        true,
    );

    let summary = runtime
        .run_turn("build me a landing page for my bakery", None)
        .expect("turn completes");

    assert_eq!(summary.iterations, 4);
    assert_eq!(
        *calls.borrow(),
        vec![
            request("auto"),
            request("auto"),
            Call::Override(Some("nemotron-3-ultra".to_string())),
            request("nemotron-3-ultra"),
            request("nemotron-3-ultra"),
            Call::Override(None),
        ],
        "nudged on the session model, retried once on the tool model, then back"
    );
    let notes = notes(runtime.session());
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(
        notes[0].contains("made no tool call, so nothing was changed"),
        "{notes:?}"
    );
    assert!(
        notes[1].contains("now runs on nemotron-3-ultra"),
        "{notes:?}"
    );
}

#[test]
fn a_nudge_that_works_keeps_the_session_model() {
    let (mut runtime, calls) = runtime(
        vec![chat("Sure, here it is: ..."), bash_call(), chat("Done.")],
        PermissionMode::WorkspaceWrite,
        routing(),
        true,
    );
    let summary = runtime
        .run_turn("please fix the failing test in parser.rs", None)
        .expect("turn completes");
    assert_eq!(summary.iterations, 3);
    assert!(calls
        .borrow()
        .iter()
        .all(|call| matches!(call, Call::Request(model) if model == "auto")));
}

#[test]
fn a_second_chat_only_answer_after_the_retry_ends_the_turn() {
    let (mut runtime, _) = runtime(
        vec![chat("Here."), chat("Here again."), chat("Still chat.")],
        PermissionMode::WorkspaceWrite,
        routing(),
        true,
    );
    let summary = runtime
        .run_turn("build me a todo app", None)
        .expect("turn completes");
    assert_eq!(summary.iterations, 3, "one nudge, one retry, no more");
}

#[test]
fn questions_read_only_sessions_and_chat_mode_are_left_alone() {
    for (prompt, mode, agent_mode) in [
        (
            "what does the parser do?",
            PermissionMode::WorkspaceWrite,
            true,
        ),
        ("build me a todo app", PermissionMode::ReadOnly, true),
        ("build me a todo app", PermissionMode::WorkspaceWrite, false),
    ] {
        let (mut runtime, _) = runtime(
            vec![chat("Here is an answer.")],
            mode,
            ModelRouting {
                agent_mode,
                ..routing()
            },
            true,
        );
        let summary = runtime.run_turn(prompt, None).expect("turn completes");
        assert_eq!(summary.iterations, 1, "{prompt} / {mode:?} / {agent_mode}");
    }
}

#[test]
fn a_turn_with_tool_calls_is_not_nudged() {
    let (mut runtime, _) = runtime(
        vec![bash_call(), chat("Built it.")],
        PermissionMode::DangerFullAccess,
        routing(),
        true,
    );
    let summary = runtime
        .run_turn("build me a todo app", None)
        .expect("turn completes");
    assert_eq!(summary.iterations, 2);
}

#[test]
fn without_a_switchable_client_the_turn_ends_after_the_nudge() {
    let (mut runtime, _) = runtime(
        vec![chat("Here is the code."), chat("Still here.")],
        PermissionMode::WorkspaceWrite,
        routing(),
        false,
    );
    let summary = runtime
        .run_turn("build me a todo app", None)
        .expect("turn completes");
    assert_eq!(summary.iterations, 2);
}

#[test]
fn the_action_nudge_flag_turns_the_chat_only_nudge_off() {
    let (runtime, _) = runtime(
        vec![chat("Here is the code.")],
        PermissionMode::WorkspaceWrite,
        routing(),
        true,
    );
    let mut runtime = runtime.with_harness_config(HarnessConfig::default());
    let summary = runtime
        .run_turn("build me a todo app", None)
        .expect("turn completes");
    assert_eq!(summary.iterations, 1);
}

#[test]
fn an_unavailable_provider_falls_over_to_the_next_model_for_the_turn() {
    let (mut runtime, calls) = runtime(
        vec![
            Err(RuntimeError::with_kind(
                RuntimeErrorKind::ProviderUnavailable,
                "503 capacity_exhausted",
            )),
            chat("Hello from the backup."),
        ],
        PermissionMode::WorkspaceWrite,
        routing(),
        true,
    );
    let summary = runtime.run_turn("hello", None).expect("turn completes");
    assert_eq!(summary.iterations, 2);
    assert_eq!(
        *calls.borrow(),
        vec![
            request("auto"),
            Call::Override(Some("backup-model".to_string())),
            request("backup-model"),
            Call::Override(None),
        ]
    );
}

#[test]
fn final_errors_and_an_exhausted_chain_are_reported() {
    let (mut runtime_one, calls) = runtime(
        vec![Err(RuntimeError::new("402 insufficient_credits"))],
        PermissionMode::WorkspaceWrite,
        routing(),
        true,
    );
    assert!(runtime_one.run_turn("hello", None).is_err());
    assert_eq!(*calls.borrow(), vec![request("auto")]);

    let unavailable = || {
        Err(RuntimeError::with_kind(
            RuntimeErrorKind::Timeout,
            "no first token",
        ))
    };
    let (mut runtime_two, calls) = runtime(
        vec![unavailable(), unavailable()],
        PermissionMode::WorkspaceWrite,
        routing(),
        true,
    );
    let error = runtime_two
        .run_turn("hello", None)
        .expect_err("chain exhausted");
    assert_eq!(error.kind(), RuntimeErrorKind::Timeout);
    assert_eq!(
        *calls.borrow(),
        vec![
            request("auto"),
            Call::Override(Some("backup-model".to_string())),
            request("backup-model"),
            Call::Override(None),
        ]
    );
}
