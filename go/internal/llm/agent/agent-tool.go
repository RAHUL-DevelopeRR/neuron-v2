package agent

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/opencode-ai/opencode/internal/config"
	"github.com/opencode-ai/opencode/internal/llm/tools"
	"github.com/opencode-ai/opencode/internal/lsp"
	"github.com/opencode-ai/opencode/internal/message"
	"github.com/opencode-ai/opencode/internal/session"
)

type agentTool struct {
	sessions   session.Service
	messages   message.Service
	lspClients map[string]*lsp.Client
}

const (
	AgentToolName = "agent"
)

type AgentParams struct {
	Prompt       string `json:"prompt"`
	Description  string `json:"description,omitempty"`
	SubagentType string `json:"subagent_type,omitempty"`
}

func (b *agentTool) Info() tools.ToolInfo {
	return tools.ToolInfo{
		Name:        AgentToolName,
		Description: "Launch a focused background sub-agent for a specific piece of work. The sub-agent has read/search tools only: GlobTool, GrepTool, LS, Sourcegraph, and View. Use it to investigate files, map a subsystem, compare approaches, or verify a change without flooding the main conversation context.\n\nUsage notes:\n1. Pick a clear subagent_type such as explore, plan, verify, review, or docs.\n2. The prompt must be detailed and self-contained because each sub-agent is stateless.\n3. Spawn multiple sub-agents in one assistant turn when the work can be split by subsystem or question.\n4. The sub-agent returns one final report. Summarize useful findings to the user yourself.\n5. Sub-agents cannot edit files or run Bash; use normal tools for mutations.",
		Parameters: map[string]any{
			"prompt": map[string]any{
				"type":        "string",
				"description": "The task for the agent to perform",
			},
			"description": map[string]any{
				"type":        "string",
				"description": "Short human-readable label for the delegated work",
			},
			"subagent_type": map[string]any{
				"type":        "string",
				"description": "Specialization hint such as explore, plan, verify, review, or docs",
			},
		},
		Required: []string{"prompt"},
	}
}

func (b *agentTool) Run(ctx context.Context, call tools.ToolCall) (tools.ToolResponse, error) {
	var params AgentParams
	if err := json.Unmarshal([]byte(call.Input), &params); err != nil {
		return tools.NewTextErrorResponse(fmt.Sprintf("error parsing parameters: %s", err)), nil
	}
	if params.Prompt == "" {
		return tools.NewTextErrorResponse("prompt is required"), nil
	}
	if params.SubagentType == "" {
		params.SubagentType = "explore"
	}
	if params.Description == "" {
		params.Description = fmt.Sprintf("%s sub-agent", params.SubagentType)
	}

	sessionID, messageID := tools.GetContextValues(ctx)
	if sessionID == "" || messageID == "" {
		return tools.ToolResponse{}, fmt.Errorf("session_id and message_id are required")
	}

	agent, err := NewAgent(config.AgentTask, b.sessions, b.messages, TaskAgentTools(b.lspClients))
	if err != nil {
		return tools.ToolResponse{}, fmt.Errorf("error creating agent: %s", err)
	}

	sessionTitle := fmt.Sprintf("%s: %s", params.SubagentType, params.Description)
	session, err := b.sessions.CreateTaskSession(ctx, call.ID, sessionID, sessionTitle)
	if err != nil {
		return tools.ToolResponse{}, fmt.Errorf("error creating session: %s", err)
	}

	prompt := fmt.Sprintf("You are a %s sub-agent.\n\nTask: %s\n\n%s", params.SubagentType, params.Description, params.Prompt)
	done, err := agent.Run(ctx, session.ID, prompt)
	if err != nil {
		return tools.ToolResponse{}, fmt.Errorf("error generating agent: %s", err)
	}
	result := <-done
	if result.Error != nil {
		return tools.ToolResponse{}, fmt.Errorf("error generating agent: %s", result.Error)
	}

	response := result.Message
	if response.Role != message.Assistant {
		return tools.NewTextErrorResponse("no response"), nil
	}

	updatedSession, err := b.sessions.Get(ctx, session.ID)
	if err != nil {
		return tools.ToolResponse{}, fmt.Errorf("error getting session: %s", err)
	}
	parentSession, err := b.sessions.Get(ctx, sessionID)
	if err != nil {
		return tools.ToolResponse{}, fmt.Errorf("error getting parent session: %s", err)
	}

	parentSession.Cost += updatedSession.Cost

	_, err = b.sessions.Save(ctx, parentSession)
	if err != nil {
		return tools.ToolResponse{}, fmt.Errorf("error saving parent session: %s", err)
	}
	return tools.NewTextResponse(fmt.Sprintf("[%s sub-agent] %s", params.SubagentType, response.Content().String())), nil
}

func NewAgentTool(
	Sessions session.Service,
	Messages message.Service,
	LspClients map[string]*lsp.Client,
) tools.BaseTool {
	return &agentTool{
		sessions:   Sessions,
		messages:   Messages,
		lspClients: LspClients,
	}
}
