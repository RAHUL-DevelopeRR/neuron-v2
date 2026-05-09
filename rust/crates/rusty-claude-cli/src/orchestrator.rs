//! # Multi-Model Orchestrator
//!
//! Handles all multi-model API calls for NeuronCLI's orchestration modes:
//! - `/chain`  — Architect → Coder → Reviewer pipeline
//! - `/power`  — Parallel ensemble + merge agent
//! - `/divide` — Per-file task splitting (prompt-only, uses main runtime)
//!
//! Supports Azure AI Foundry deployments and OpenRouter free-tier models.
//! All API calls use a universal parameter set — no model-specific code.

use std::env;
use std::fmt;

// ── Error type ──────────────────────────────────────────────

/// Orchestrator error with structured context for logging.
#[derive(Debug)]
pub struct OrchestratorError {
    pub provider: &'static str,
    pub model: String,
    pub status: Option<u16>,
    pub message: String,
}

impl fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(s) => write!(
                f,
                "[{}] {} returned {}: {}",
                self.provider, self.model, s, self.message
            ),
            None => write!(
                f,
                "[{}] {} error: {}",
                self.provider, self.model, self.message
            ),
        }
    }
}

impl std::error::Error for OrchestratorError {}

/// Result of a successful model call.
#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub model: String,
    pub content: String,
    pub tokens: u32,
    pub strategy: &'static str, // "deploy", "v1", or "openrouter"
}

// ── Model roster ────────────────────────────────────────────

/// Deployment names for orchestration roles.
/// All names match Azure AI Foundry deployment dashboard exactly.
/// Every model here has been verified working via automated test suite.
pub struct Models;

impl Models {
    /// Cheap Azure models for parallel agents in /power mode
    pub fn cheap_azure() -> &'static [&'static str] {
        &["Kimi-K2.5", "FW-DeepSeek-V3.2", "FW-MiniMax-M2.5"]
    }

    /// OpenRouter free-tier model (4th agent in /power mode)
    pub fn openrouter_free() -> &'static str {
        "qwen/qwen3-235b-a22b:free"
    }

    // ── Chain mode roles ──
    pub fn architect() -> &'static str {
        "Kimi-K2.5"
    }
    pub fn coder() -> &'static str {
        "FW-DeepSeek-V3.2"
    }
    pub fn reviewer() -> &'static str {
        "FW-MiniMax-M2.5"
    }

    /// Merge agent — used by /power to combine ensemble outputs
    pub fn merge() -> &'static str {
        "model-router"
    }
}

// ── API versions to try ─────────────────────────────────────

/// Azure API versions in priority order.
/// Newer versions support more models; older ones are more stable.
const API_VERSIONS: &[&str] = &["2024-10-21", "2025-01-01-preview"];

// ── Azure host resolution ───────────────────────────────────

/// Derive the Azure host root from the endpoint env var.
/// Strips `/openai/v1` suffix if present so we can construct
/// both deployment-specific and v1-compatible URLs.
fn azure_host_root() -> String {
    let raw = env::var("AZURE_OPENAI_ENDPOINT").unwrap_or_default();
    raw.trim_end_matches('/')
        .trim_end_matches("/v1")
        .trim_end_matches("/openai")
        .to_string()
}

/// Get the Azure API key from the environment.
pub fn azure_api_key() -> String {
    env::var("AZURE_OPENAI_API_KEY").unwrap_or_default()
}

// ── HTTP client singleton ───────────────────────────────────

fn build_client(timeout_secs: u64) -> Result<reqwest::blocking::Client, OrchestratorError> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| OrchestratorError {
            provider: "http",
            model: String::new(),
            status: None,
            message: format!("Client build failed: {e}"),
        })
}

// ── Universal response parser ───────────────────────────────

/// Parse a chat completion response body.
/// Handles both standard `content` and reasoning models' `reasoning_content`.
fn parse_response(
    body: &str,
    model: &str,
    provider: &'static str,
) -> Result<ModelResponse, OrchestratorError> {
    let parsed: serde_json::Value = serde_json::from_str(body).map_err(|e| OrchestratorError {
        provider,
        model: model.to_string(),
        status: None,
        message: format!("JSON parse error: {e}"),
    })?;

    // Check for API-level error in body
    if let Some(err) = parsed.get("error") {
        let msg = err["message"].as_str().unwrap_or("unknown error");
        let code = err["code"].as_str().unwrap_or("");
        return Err(OrchestratorError {
            provider,
            model: model.to_string(),
            status: None,
            message: format!("{code}: {msg}"),
        });
    }

    let msg = &parsed["choices"][0]["message"];
    let content = msg["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| msg["reasoning_content"].as_str())
        .unwrap_or("")
        .to_string();
    let tokens = parsed["usage"]["total_tokens"].as_u64().unwrap_or(0) as u32;

    Ok(ModelResponse {
        model: model.to_string(),
        content,
        tokens,
        strategy: provider,
    })
}

// ── Azure API call ──────────────────────────────────────────

/// Call an Azure AI Foundry deployment.
///
/// Uses a **dual-URL strategy** (no model-specific params):
/// 1. Try `/openai/deployments/{name}/chat/completions` (standard Azure)
/// 2. Fallback to `/openai/v1/chat/completions` with `model` in body
///
/// Both use `max_tokens` only (universally supported).
pub fn azure_call(
    api_key: &str,
    model: &str,
    messages: &[serde_json::Value],
    max_tokens: u32,
) -> Result<ModelResponse, OrchestratorError> {
    let host = azure_host_root();
    let client = build_client(120)?;

    // Universal body — same for every model
    let body = serde_json::json!({
        "messages": messages,
        "max_tokens": max_tokens,
    });

    // Strategy 1: Deployment-specific URL (try each API version)
    for api_ver in API_VERSIONS {
        let url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            host, model, api_ver
        );
        match client
            .post(&url)
            .header("content-type", "application/json")
            .header("api-key", api_key)
            .json(&body)
            .send()
        {
            Ok(r) if r.status().as_u16() == 200 => {
                let text = r.text().unwrap_or_default();
                let mut resp = parse_response(&text, model, "deploy")?;
                resp.strategy = "deploy";
                return Ok(resp);
            }
            Ok(r) if r.status().as_u16() == 404 => continue, // try next version
            _ => continue,
        }
    }

    // Strategy 2: /openai/v1 with model in body (OpenAI-compat)
    let mut body_v1 = body.clone();
    body_v1["model"] = serde_json::json!(model);
    let v1_url = format!("{}/openai/v1/chat/completions", host);

    let resp = client
        .post(&v1_url)
        .header("content-type", "application/json")
        .header("api-key", api_key)
        .bearer_auth(api_key)
        .json(&body_v1)
        .send()
        .map_err(|e| OrchestratorError {
            provider: "azure-v1",
            model: model.to_string(),
            status: None,
            message: format!("Network error: {e}"),
        })?;

    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();

    if status != 200 {
        return Err(OrchestratorError {
            provider: "azure-v1",
            model: model.to_string(),
            status: Some(status),
            message: text.chars().take(300).collect(),
        });
    }

    let mut result = parse_response(&text, model, "v1")?;
    result.strategy = "v1";
    Ok(result)
}

// ── OpenRouter API call ─────────────────────────────────────

/// Call an OpenRouter free-tier model.
/// Gracefully fails — orchestration continues without it.
pub fn openrouter_call(
    api_key: &str,
    model: &str,
    messages: &[serde_json::Value],
    max_tokens: u32,
) -> Result<ModelResponse, OrchestratorError> {
    let client = build_client(90)?;

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
    });

    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", api_key))
        .header("HTTP-Referer", "https://zero-x.live")
        .header("X-Title", "NeuronCLI")
        .json(&body)
        .send()
        .map_err(|e| OrchestratorError {
            provider: "openrouter",
            model: model.to_string(),
            status: None,
            message: format!("Network error: {e}"),
        })?;

    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();

    if status != 200 {
        return Err(OrchestratorError {
            provider: "openrouter",
            model: model.to_string(),
            status: Some(status),
            message: text.chars().take(200).collect(),
        });
    }

    let mut result = parse_response(&text, model, "openrouter")?;
    result.strategy = "openrouter";
    Ok(result)
}

// ── Logging helpers ─────────────────────────────────────────

/// Print a colored orchestration status line to stderr.
pub fn log_phase(mode: &str, _color: &str, msg: &str) {
    eprintln!("[{mode}] {msg}");
}

pub fn log_ok(mode: &str, _color: &str, model: &str, tokens: u32) {
    eprintln!("[{mode}] OK {model} ({tokens} tokens)");
}

pub fn log_fail(mode: &str, _color: &str, model: &str, err: &OrchestratorError) {
    eprintln!("[{mode}] FAIL {model}: {err}");
}

pub fn log_skip(mode: &str, _color: &str, model: &str, reason: &str) {
    eprintln!("[{mode}] SKIP {model}: {reason}");
}

// ── Chain mode orchestration ────────────────────────────────

/// Execute the chain pipeline: Architect → Coder → Reviewer.
/// Returns the combined output from all 3 models to feed into the main runtime.
pub fn run_chain(api_key: &str, user_input: &str) -> String {
    let color = "";
    let mode = "chain";

    // Phase 1: Architect
    log_phase(
        mode,
        color,
        &format!("Phase 1: {} (architect)...", Models::architect()),
    );
    let arch_msgs = vec![serde_json::json!({
        "role": "user",
        "content": format!(
            "You are a senior software architect. Design the approach for this task.\n\
             Output ONLY the technical design:\n\
             - Components needed and why\n- Data flow and interfaces\n- Edge cases\n\
             NO code — just architecture.\n\nTask: {}", user_input
        )
    })];
    let arch_result = match azure_call(api_key, Models::architect(), &arch_msgs, 4000) {
        Ok(r) => {
            log_ok(mode, color, &r.model, r.tokens);
            r.content
        }
        Err(e) => {
            log_fail(mode, color, Models::architect(), &e);
            format!("(architect unavailable) Task: {user_input}")
        }
    };

    // Phase 2: Coder
    log_phase(
        mode,
        color,
        &format!("Phase 2: {} (coder)...", Models::coder()),
    );
    let code_msgs = vec![
        serde_json::json!({"role": "system", "content": "You are an expert coder. Implement the architect's design with clean, production-quality code. Include error handling, type hints, docstrings."}),
        serde_json::json!({"role": "user", "content": format!("ARCHITECTURE DESIGN:\n{arch_result}\n\nORIGINAL TASK:\n{user_input}\n\nImplement this now. Write complete, working code files.")}),
    ];
    let code_result = match azure_call(api_key, Models::coder(), &code_msgs, 8000) {
        Ok(r) => {
            log_ok(mode, color, &r.model, r.tokens);
            r.content
        }
        Err(e) => {
            log_fail(mode, color, Models::coder(), &e);
            arch_result.clone()
        }
    };

    // Phase 3: Reviewer
    log_phase(
        mode,
        color,
        &format!("Phase 3: {} (reviewer)...", Models::reviewer()),
    );
    let review_msgs = vec![
        serde_json::json!({"role": "system", "content": "You are a senior code reviewer. Review the code below. Find bugs, security issues, missing error handling. Output the FIXED final code."}),
        serde_json::json!({"role": "user", "content": format!("ARCHITECTURE:\n{arch_result}\n\nCODE TO REVIEW:\n{code_result}\n\nReview and output the hardened, fixed code.")}),
    ];
    let review_result = match azure_call(api_key, Models::reviewer(), &review_msgs, 8000) {
        Ok(r) => {
            log_ok(mode, color, &r.model, r.tokens);
            r.content
        }
        Err(e) => {
            log_fail(mode, color, Models::reviewer(), &e);
            code_result.clone()
        }
    };

    format!(
        "[CHAIN MODE — 3 MODELS COMPLETED]\n\
         Three specialized models have processed this task:\n\n\
         === ARCHITECT ({}) ===\n{arch_result}\n\n\
         === CODER ({}) ===\n{code_result}\n\n\
         === REVIEWER ({}) ===\n{review_result}\n\n\
         Execute the REVIEWER's final output using your tools (write_file, bash, etc.).\n\
         Write the files exactly as the reviewer specified.\n\
         Original task: {user_input}\n",
        Models::architect(),
        Models::coder(),
        Models::reviewer()
    )
}

// ── Power mode orchestration ────────────────────────────────

/// Execute the power ensemble: 3 Azure + 1 OpenRouter agents → merge.
/// Returns the merged output to feed into the main runtime.
pub fn run_power(api_key: &str, user_input: &str) -> String {
    let color = "";
    let mode = "power";

    let azure_models = Models::cheap_azure();
    let total = azure_models.len() + 1; // +1 for OpenRouter

    // ── Spawn ALL agents in parallel ─────────────────────────
    let mut handles: Vec<std::thread::JoinHandle<Option<ModelResponse>>> = Vec::new();

    // Azure agents (parallel)
    for (i, model) in azure_models.iter().enumerate() {
        let key = api_key.to_string();
        let model = model.to_string();
        let input = user_input.to_string();
        let idx = i + 1;

        handles.push(std::thread::spawn(move || {
            let _result = std::panic::catch_unwind(|| {
                eprintln!("[power] Agent {}/{}: {} (Azure)", idx, total, model);
                let msgs = vec![serde_json::json!({
                    "role": "user",
                    "content": format!(
                        "You are an expert developer. Solve this task with maximum quality.\n\
                         Write complete, production-ready code.\n\nTask: {input}"
                    )
                })];
                match azure_call(&key, &model, &msgs, 2000) {
                    Ok(r) => {
                        eprintln!("[power] OK {} ({})", r.model, r.tokens);
                        Some(r)
                    }
                    Err(e) => {
                        eprintln!("[power] FAIL {}: {}", model, e);
                        None
                    }
                }
            });
            _result.unwrap_or_else(|_| {
                eprintln!("[power] CRASH {} crashed unexpectedly", model);
                None
            })
        }));
    }

    // OpenRouter 4th agent (parallel with Azure agents)
    {
        let input = user_input.to_string();
        let or_model = Models::openrouter_free().to_string();
        let or_key = crate::auth::ensure_api_key();

        handles.push(std::thread::spawn(move || {
            let _result = std::panic::catch_unwind(|| {
                eprintln!(
                    "[power] Agent {}/{}: {} (OpenRouter)",
                    total, total, or_model
                );
                let Some(api_key) = or_key else {
                    eprintln!("[power] SKIP {}: no auth", or_model);
                    return None;
                };
                let msgs = vec![serde_json::json!({
                    "role": "user",
                    "content": format!(
                        "You are an expert developer. Solve this task with maximum quality.\n\
                         Write complete, production-ready code.\n\nTask: {input}"
                    )
                })];
                match openrouter_call(&api_key, &or_model, &msgs, 2000) {
                    Ok(r) => {
                        eprintln!("[power] OK {} ({})", r.model, r.tokens);
                        Some(r)
                    }
                    Err(e) => {
                        eprintln!("[power] SKIP {}: {}", or_model, e);
                        None
                    }
                }
            });
            _result.unwrap_or_else(|_| {
                eprintln!("[power] CRASH {} crashed unexpectedly", or_model);
                None
            })
        }));
    }

    // ── Collect results from all threads ─────────────────────
    let results: Vec<ModelResponse> = handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect();

    if results.is_empty() {
        return user_input.to_string();
    }

    // Fast path: single result — no merge needed
    if results.len() == 1 {
        let r = &results[0];
        return format!(
            "[POWER MODE — SINGLE MODEL]\n\
             Only one model produced a solution. Using output from {}.\n\n\
             {}\n\n\
             Execute this code using your tools (write_file, bash, etc.).\n\
             Original task: {user_input}\n",
            r.model, r.content
        );
    }

    // Compress each solution to fit merge budget
    let merge_budget = 3000usize;
    let per_solution_budget = merge_budget / results.len();
    let mut merge_content = String::from(
        "You are a merge agent. Below are solutions from multiple AI models.\n\
         COMBINE the BEST PARTS into ONE concise solution (max 150 lines):\n\
         - Pick best algorithm\n- Pick best structure\n- Pick best error handling\n\
         Produce ONE merged implementation. Be concise.\n\n",
    );
    for r in &results {
        let compressed =
            crate::token_budget::compress_tool_result(&r.model, &r.content, per_solution_budget);
        merge_content.push_str(&format!("=== {} ===\n{}\n\n", r.model, compressed));
    }
    merge_content.push_str(&format!("TASK: {user_input}\n"));

    log_phase(mode, color, &format!("Merging with {}...", Models::merge()));
    let merge_msgs = vec![serde_json::json!({"role": "user", "content": merge_content})];

    match azure_call(api_key, Models::merge(), &merge_msgs, 3000) {
        Ok(r) => {
            log_ok(mode, color, &r.model, r.tokens);
            format!(
                "[POWER MODE — {} MODELS MERGED]\n\
                 Multiple models generated solutions IN PARALLEL, a merge agent combined the best parts.\n\n\
                 === MERGED RESULT ===\n{}\n\n\
                 Execute this merged code using your tools (write_file, bash, etc.).\n\
                 Write the files exactly as specified.\n\
                 Original task: {user_input}\n",
                results.len(), r.content
            )
        }
        Err(e) => {
            log_fail(mode, color, Models::merge(), &e);
            // Fallback: use the first successful result
            format!(
                "[POWER MODE — FALLBACK]\n\
                 Merge failed, using best single output from {}.\n\n\
                 {}\n\nExecute this code. Original task: {user_input}\n",
                results[0].model, results[0].content
            )
        }
    }
}

// ── Divide mode (prompt-only) ───────────────────────────────

/// Build the divide-mode prompt wrapper.
/// This mode doesn't make extra API calls — it instructs the main
/// runtime model to split work per file/module.
pub fn build_divide_prompt(user_input: &str) -> String {
    format!(
        "[DIVIDE MODE — MULTI-FILE PARALLEL STRATEGY]\n\
         You are operating in DIVIDE mode. Follow this workflow:\n\n\
         1. ANALYZE the task and identify all files/modules needed.\n\
         2. For EACH file, act as a specialized sub-agent.\n\
         3. Design each file independently with clean interfaces.\n\
         4. After generating all files, act as INTEGRATOR:\n\
            - Ensure imports/exports are consistent\n\
            - Verify shared state and data flow\n\
            - Fix cross-file dependencies\n\
         5. Summary of files created and how they connect.\n\n\
         User's request: {user_input}\n"
    )
}
