# Opencode Architecture Analysis & Neuron Integration Plan

## Repository Created
**https://github.com/RAHUL-DevelopeRR/neuron-v2**

---

## 1. Opencode Architecture Deep Dive

### 1.1 Overall Structure
```
opencode/
├── cmd/opencode/           # CLI entry point (main.go)
├── internal/
│   ├── tui/                # Bubble Tea TUI (components, layout, theme)
│   ├── pubsub/             # Typed generic event broker
│   ├── db/                 # SQLite + sqlc code generation
│   ├── config/             # Viper-based config with agents, providers, MCP
│   ├── llm/
│   │   ├── agent/          # Agent orchestration (coder, summarizer, task, title)
│   │   ├── models/         # Model definitions & aliases
│   │   ├── prompt/         # Prompt templates & management
│   │   ├── provider/       # Provider abstractions (OpenAI, Anthropic, etc.)
│   │   └── tools/          # Tool system (bash, view, edit, grep, glob, ls, write, patch)
│   ├── permission/         # Permission service for tool execution
│   ├── session/            # Session management
│   └── lsp/                # LSP client integration
├── go.mod                  # Go module definition
└── install                 # Install script
```

### 1.2 Key Features to Integrate

#### A. **PubSub Event Broker** (`internal/pubsub/broker.go`)
- **Generic typed broker**: `Broker[T any]` with `Subscribe(ctx) <-chan Event[T]` and `Publish(t EventType, payload T)`
- **Context-aware subscriptions**: Auto-cleanup when context cancelled
- **Non-blocking publish**: `select { case sub <- event: default: }` prevents backpressure
- **Graceful shutdown**: `Shutdown()` closes all subscriber channels
- **Integration**: Replace Neuron's direct stdout writes with event-driven architecture

#### B. **TUI Architecture** (`internal/tui/`)
- **Bubble Tea framework**: Elm architecture (Model-Update-View)
- **Component-based**: 22 components in `components/`
- **Layout system**: `layout/` with responsive design
- **Theme system**: 12 theme files with lipgloss styling
- **Pages**: 3 page types (chat, config, etc.)
- **Integration**: Add Bubble Tea as optional TUI mode alongside current rustyline REPL

#### C. **Tool System** (`internal/llm/tools/`)
- **Rich tool descriptions**: Each tool has detailed markdown description with WHEN/HOW/LIMITATIONS/TIPS
- **Permission-based execution**: `permission.Service` gates dangerous operations
- **Command banning**: `bannedCommands` list (curl, wget, nc, etc.)
- **Safe read-only whitelist**: `safeReadOnlyCommands` for git status, ls, etc.
- **Output truncation**: `MaxOutputLength = 30000` characters
- **Timeout handling**: Default 1min, max 10min
- **Integration**: Adopt opencode's tool description format and permission system

#### D. **Database Layer** (`internal/db/`)
- **sqlc code generation**: Type-safe SQL queries from `.sql` files
- **Tables**: sessions, messages, files with CRUD operations
- **Session persistence**: Full conversation history
- **File tracking**: Track modified files per session
- **Integration**: Replace JSONL sessions with SQLite + sqlc

#### E. **Configuration System** (`internal/config/`)
- **Viper-based**: Multi-source config (env, file, flags)
- **Agent definitions**: Named agents (coder, summarizer, task, title) with model + maxTokens
- **Provider config**: API keys with disable toggle
- **MCP server support**: Model Control Protocol for external tools
- **LSP integration**: Language Server Protocol config
- **Auto-compact**: Summarize at 95% context window
- **Integration**: Adopt structured agent config and auto-compact feature

#### F. **Agent System** (`internal/llm/agent/`)
- **Multi-agent roles**: Coder, Summarizer, Task, Title agents
- **Agent-tool integration**: `agent-tool.go` for tool calling
- **MCP tools**: External tool server integration
- **Integration**: Replace monolithic agent with role-based agents

#### G. **Model System** (`internal/llm/models/`)
- **Model aliases**: Friendly names map to provider-specific IDs
- **Provider abstractions**: OpenAI, Anthropic, Google, AWS, Groq, Azure, OpenRouter
- **Integration**: Adopt alias system for multi-provider support

---

## 2. Neuron → Opencode Feature Integration Map

| Opencode Feature | Neuron Current | Integration Strategy | Priority |
|------------------|----------------|----------------------|----------|
| **PubSub Broker** | Direct stdout writes | Add `events.rs` module with typed channels | HIGH |
| **Bubble Tea TUI** | rustyline REPL | Add optional `--tui` flag, keep REPL as default | MEDIUM |
| **Tool Descriptions** | Basic tool info | Adopt rich markdown descriptions with examples | HIGH |
| **Permission System** | Simple mode enum | Add per-tool permission granularity | HIGH |
| **Command Banning** | None | Add `banned_commands` list + safe whitelist | HIGH |
| **SQLite Sessions** | JSONL files | Migrate to SQLite + sqlx (Rust equivalent) | MEDIUM |
| **Auto-Compact** | None | Summarize at 80% token budget (already implemented) | DONE |
| **Agent Roles** | Single agent | Split into Coder/Executor/Planner agents | MEDIUM |
| **MCP Support** | None | Add MCP client for external tool servers | LOW |
| **LSP Integration** | None | Add LSP client for code intelligence | LOW |
| **Theme System** | Basic ANSI | Add lipgloss-like styling with themes | LOW |

---

## 3. Implementation Plan

### Phase 1: Event-Driven Architecture (Week 1)
```rust
// New: src/events.rs
pub enum NeuronEvent {
    TokenDelta(String),
    ToolCall { name: String, input: String },
    ToolResult { name: String, output: String },
    SessionStart,
    SessionEnd,
    Error(String),
}

pub struct EventBroker {
    subscribers: Vec<mpsc::Sender<NeuronEvent>>,
}
```
- Replace direct stdout with event publishing
- Add subscribers: terminal renderer, session logger, metrics collector

### Phase 2: Enhanced Tool System (Week 1-2)
- Add rich markdown descriptions to all tools
- Implement `permission.rs` with per-tool granularity
- Add banned commands list + safe command whitelist
- Truncate tool output at 30K chars (already in token_budget.rs)

### Phase 3: Multi-Agent Roles (Week 2)
```rust
pub enum AgentRole {
    Planner,      // Architect: plans approach
    Coder,        // Writer: generates code
    Executor,     // Runner: executes tools
    Reviewer,     // Critic: reviews output
    Summarizer,   // Memory: compresses history
}
```
- Split current monolithic agent into roles
- Each role has different system prompt and model config

### Phase 4: SQLite Migration (Week 3)
- Replace JSONL with SQLite + sqlx
- Sessions table: id, created_at, updated_at, summary
- Messages table: id, session_id, role, content, tokens, timestamp
- Files table: id, session_id, path, content_hash, modified_at

### Phase 5: TUI Mode (Week 4)
- Add `--tui` flag for Bubble Tea mode
- Keep rustyline as `--repl` default
- Use same event broker for both UIs

---

## 4. Code Push to neuron-v2

The following will be pushed to `https://github.com/RAHUL-DevelopeRR/neuron-v2`:
- Full Rust codebase (`rust/`)
- Python shim (`neuron_cli/`)
- Build artifacts (`dist/`)
- Documentation (`docs/`)
- Integration plan (`docs/OPENCODE_INTEGRATION.md`)

---

## 5. Immediate Actions for Power Mode Fix

Based on opencode's approach:
1. **Timeout enforcement**: Add 60s timeout per agent (opencode uses 1-10min)
2. **Output truncation**: Already implemented (30K chars / 2K tokens)
3. **Permission gating**: Add before executing dangerous commands
4. **Event streaming**: Replace blocking loop with event-driven streaming
5. **Auto-compact**: Already implemented in token_budget.rs

---

## 6. Performance Targets (Post-Integration)

| Metric | Current | Target (Opencode-level) |
|--------|---------|------------------------|
| First token latency | 5+ min | <2s |
| Streaming FPS | ~10 | 60+ |
| Token budget | 17K+ | 8K |
| Tool output truncation | None | 30K chars |
| Session persistence | JSONL | SQLite |
| UI mode | REPL only | REPL + TUI |
