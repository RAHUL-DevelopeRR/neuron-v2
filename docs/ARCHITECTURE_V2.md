# Neuron Architecture V2 — Production AI Coding Agent

## Executive Summary

**Current state**: Token usage explodes exponentially (~17K merge tokens), terminal freezes during subprocess execution, streaming is unbuffered causing rerender storms, full repository injected into every prompt.

**Target state**: Claude-Code-level latency (<2s first token, 60+ tok/s), aggressive context pruning (<8K tokens total prompt), buffered streaming at 30-50ms intervals, async non-blocking tool execution.

## Root Cause Analysis

### Token Explosion (O(n²) growth)
- Full conversation replay: every turn resends entire history verbatim
- Tool output recursion: previous tool results embedded as text in subsequent prompts
- File duplication: `write_file` outputs full file content back into context
- No summarization: old conversation turns never get compressed
- Repository dump: `glob_search` + `read_file` inject everything into system prompt
- Terminal log spam: every shell command + stdout is preserved in context

### Terminal Latency (Blocking I/O)
- `subprocess.run()` blocks event loop
- `reqwest::blocking` inside `orchestrator.rs` spawns OS threads per API call
- No async streaming: waits for full response before rendering
- Rustyline prompt re-renders on every token (unbuffered)

### Streaming Stutters (Render Storms)
- React/xterm.js re-renders on every `onmessage` token event
- No virtualized scrollback: DOM grows unbounded
- No batching: 30-60 FPS token stream overwhelms renderer

## Reference Benchmarks

| Metric | Claude Code | Cursor | Current Neuron | Target |
|--------|-------------|--------|----------------|--------|
| First token latency | ~800ms | ~1.2s | ~8-15s | <2s |
| Tokens/sec | 45-80 | 30-60 | 5-15 | 50+ |
| Context tokens/turn | ~4-8K | ~6-10K | ~20-40K | <8K |
| Tool exec latency | <500ms | <1s | 3-10s | <500ms |
| Terminal FPS | 60 | 60 | 10-15 | 60 |

## Architectural Redesign

### High-Level Modules

```
┌─────────────────────────────────────────────────────────────┐
│                         UI Layer                             │
│  (React/Tauri/xterm.js or Terminal CLI)                    │
│  - Batched render loop (30-50ms)                             │
│  - Virtualized scrollback (max 10K lines)                    │
│  - Incremental diffs only                                    │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼ WebSocket / IPC
┌─────────────────────────────────────────────────────────────┐
│                    Event Bus (asyncio / tokio)               │
│  - Typed events: TokenBatch, ToolStart, ToolDone             │
│  - Backpressure: drop old frames if renderer behind          │
└─────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
┌──────────────┐  ┌──────────────────┐  ┌──────────────────┐
│  Planner     │  │  Executor        │  │  Terminal Agent  │
│  Agent       │  │  Agent           │  │                  │
│              │  │                  │  │  - Shell Pty     │
│  - Query     │  │  - Tool dispatch │  │  - Async exec    │
│  analysis    │  │  - Parallel tools│  │  - Output        │
│  - Context   │  │  - Result buffer │  │    streaming     │
│  budget      │  │                  │  │                  │
└──────────────┘  └──────────────────┘  └──────────────────┘
        │                     │                     │
        └─────────────────────┼─────────────────────┘
                              ▼
┌─────────────────────────────────────────────────────────────┐
│              Context Retrieval & Memory Engine               │
│                                                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐         │
│  │  Repo Map   │  │  Semantic   │  │  Working    │         │
│  │  (AST)      │  │  Search     │  │  Memory     │         │
│  │  tree-sitter│  │  (embed)    │  │  (SQLite+   │         │
│  │  deps graph │  │  chunking   │  │  summaries) │         │
│  └─────────────┘  └─────────────┘  └─────────────┘         │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│              LLM Router & Inference Layer                    │
│                                                              │
│  - Streaming-first (SSE/WebSocket)                           │
│  - KV-cache aware prompt templates                           │
│  - Automatic model fallback                                  │
│  - Token budget enforcement (hard cap)                       │
└─────────────────────────────────────────────────────────────┘
```

## Token Optimization Strategy

### Prompt Structure (KV-Cache Aware)

```
[SYSTEM: 512 tokens]          ← Cached by provider (prefix caching)
  Static instructions, tool schema, formatting rules

[REPO_MAP: 1024 tokens]     ← Cached if repo unchanged
  Dependency graph, file list, symbol index (NO full files)

[WORKING_MEMORY: 512 tokens]  ← Updated each turn
  Key facts, decisions, open tasks

[RETRIEVED_CONTEXT: 2048 tokens] ← Dynamic per query
  Relevant file chunks from semantic search

[TOOL_RESULTS: 1024 tokens]   ← Compressed, last 3 only
  Summarized tool outputs, not raw stdout

[CONVERSATION: 2048 tokens]   ← Sliding window
  Last N messages (N ≈ 6-8), older ones summarized

[TURN: variable]              ← User input + reasoning
  Current query, planner reasoning
```

**Total budget: 8192 tokens max.**

### Context Pruning Rules

| Content Type | Retention Policy | Compression |
|--------------|------------------|-------------|
| System prompt | Keep forever | Static |
| Repo map | Rebuild on file change | AST symbols only |
| Working memory | Summarize every 5 turns | Bullet list |
| Tool results | Keep last 3, summarize older | Diff format |
| File reads | Semantic relevance < 0.7 → drop | Chunk + summary |
| Conversation | Sliding window 6-8 turns | Summary beyond window |
| Terminal logs | Keep last command only | Exit code + last 10 lines |

### Conversation Summarization

```python
class ConversationPruner:
    def prune(self, messages: list[Message], budget: int) -> list[Message]:
        current = self.tokenizer.count(messages)
        if current <= budget:
            return messages

        # Summarize oldest pairs (user + assistant)
        while current > budget and len(messages) > 4:
            oldest_user = messages.pop(0)
            oldest_assistant = messages.pop(0)
            summary = self.summarize_pair(oldest_user, oldest_assistant)
            messages.insert(0, Message(role="system", content=f"[Earlier: {summary}]"))
            current = self.tokenizer.count(messages)

        # Truncate tool results if still over budget
        for msg in messages:
            if msg.role == "tool" and len(msg.content) > 500:
                msg.content = msg.content[:500] + "\n... [truncated]"

        return messages
```

## Terminal Engine Redesign

### Async Shell Execution (Pty)

Replace `subprocess.run()` with `asyncio.create_subprocess_exec()` + `os.openpty()`:

```python
class AsyncShell:
    async def execute(self, cmd: str, cwd: str) -> ShellResult:
        master, slave = os.openpty()
        proc = await asyncio.create_subprocess_exec(
            "bash", "-c", cmd,
            stdin=slave, stdout=slave, stderr=slave,
            cwd=cwd,
        )
        os.close(slave)

        output = []
        loop = asyncio.get_event_loop()
        reader = asyncio.StreamReader()
        transport, _ = await loop.connect_read_pipe(
            lambda: asyncio.StreamReaderProtocol(reader), os.fdopen(master)
        )

        async for line in reader:
            output.append(line.decode())
            self.emit("shell_output", line.decode())

        await proc.wait()
        return ShellResult(
            exit_code=proc.returncode,
            output="".join(output[-500:]),  # Keep last 500 lines only
        )
```

### Buffered Token Streaming (Rust)

```rust
use tokio::time::{interval, Duration};
use tokio::sync::mpsc;

pub struct BufferedRenderer {
    buffer: String,
    rx: mpsc::Receiver<String>,
    frame_interval: Duration,
}

impl BufferedRenderer {
    pub async fn run(mut self, mut stdout: impl Write) {
        let mut tick = interval(self.frame_interval);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if !self.buffer.is_empty() {
                        stdout.write_all(self.buffer.as_bytes()).ok();
                        stdout.flush().ok();
                        self.buffer.clear();
                    }
                }
                Some(token) = self.rx.recv() => {
                    self.buffer.push_str(&token);
                    if self.buffer.len() > 1024 {
                        stdout.write_all(self.buffer.as_bytes()).ok();
                        stdout.flush().ok();
                        self.buffer.clear();
                    }
                }
                else => break,
            }
        }
    }
}
```

## Memory Architecture

### Three-Tier Memory

```
┌─────────────────────────────────────────┐
│  Tier 1: Working Memory (in-process)    │
│  - Current task context                 │
│  - Open files, pending edits            │
│  - Last 3 tool results                  │
│  - TTL: 1 turn                          │
└─────────────────────────────────────────┘
                    │
                    ▼ summarize
┌─────────────────────────────────────────┐
│  Tier 2: Session Memory (SQLite)        │
│  - Conversation summaries               │
│  - Key decisions & facts                │
│  - File edit history (diffs)            │
│  - TTL: session lifetime                │
└─────────────────────────────────────────┘
                    │
                    ▼ embed
┌─────────────────────────────────────────┐
│  Tier 3: Long-term Memory (Qdrant)      │
│  - Code embeddings (file chunks)        │
│  - Conversation embeddings              │
│  - Error pattern embeddings             │
│  - TTL: persistent                      │
└─────────────────────────────────────────┘
```

### SQLite Schema

```sql
CREATE TABLE conversation_summary (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    summary TEXT NOT NULL,
    key_facts TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE file_edit_history (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    diff TEXT NOT NULL,
    edit_reason TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE tool_result_cache (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    params_hash TEXT NOT NULL,
    result_summary TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

## Async Orchestration Flow

```
User Input
    │
    ▼
[Planner Agent] ──► Query analysis ──► Context budget: 1024 tokens
    │                                      Retrieve relevant files
    ▼                                      Build repo map
Plan: [tool1, tool2, tool3]
    │
    ▼
[Executor Agent] ──► Parallel tool dispatch (async)
    │                    ├─► read_file("src/main.rs")
    │                    ├─► glob_search("*.py")
    │                    └─► bash("cargo test")
    │
    ▼ (collect results)
[Context Compressor] ──► Summarize tool outputs ──► Budget: 512 tokens
    │
    ▼
[LLM Turn] ──► Streaming generation ──► TokenBatch events
    │
    ▼
[Buffered Renderer] ──► 30ms frame output
    │
    ▼
User sees smooth streaming
```

## Observability & Benchmarking

### Metrics to Track

| Metric | Type | Target | Alert Threshold |
|--------|------|--------|----------------|
| `llm_first_token_latency` | Histogram | <2s | >5s |
| `llm_tokens_per_second` | Gauge | >50 | <20 |
| `context_input_tokens` | Counter | <8K | >12K |
| `tool_execution_latency` | Histogram | <500ms | >3s |
| `render_frame_time` | Histogram | <16ms | >33ms |
| `websocket_msg_per_sec` | Gauge | <30 | >100 |

### OpenTelemetry Integration

```rust
use tracing::{info_span, Instrument};

async fn run_turn(query: &str) -> Result<TurnResult> {
    let span = info_span!("turn", query = query);
    async {
        let planner_span = info_span!("planner");
        let plan = plan_query(query).instrument(planner_span).await?;

        let exec_span = info_span!("executor", tools = plan.tools.len());
        let results = execute_tools_parallel(plan.tools)
            .instrument(exec_span)
            .await;

        let llm_span = info_span!("llm_stream", model = &plan.model);
        let response = stream_response(&plan, &results)
            .instrument(llm_span)
            .await?;

        Ok(TurnResult { response })
    }
    .instrument(span)
    .await
}
```

## Implementation Roadmap

### Phase 1: Critical Path (Week 1)
- [ ] Replace `reqwest::blocking` with async streaming in Rust CLI
- [ ] Implement token budget enforcer (hard 8K cap)
- [ ] Add conversation pruner with summarization
- [ ] Buffered renderer (30ms frame interval)
- [ ] Async shell execution (Pty + create_subprocess_exec)

### Phase 2: Retrieval (Week 2)
- [ ] Tree-sitter repo map builder
- [ ] Embedding pipeline (384-dim, local model)
- [ ] Qdrant/LanceDB integration
- [ ] Semantic search endpoint
- [ ] Replace full-repo injection with retrieved chunks

### Phase 3: Memory (Week 3)
- [ ] SQLite session store
- [ ] Conversation summarization agent
- [ ] Working memory manager
- [ ] Tool result cache with hash-based dedup

### Phase 4: Polish (Week 4)
- [ ] OpenTelemetry tracing
- [ ] Benchmark harness
- [ ] WebSocket batching (Tauri/React)
- [ ] Virtualized terminal scrollback
