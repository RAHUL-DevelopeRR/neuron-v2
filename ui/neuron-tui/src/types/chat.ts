// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// sponsored-proposal block, inline-ad marker and per-message credits are
// gone; a tool block can carry its input while the model writes it and a
// sub-agent's progress, a `git-view` block holds a diff or a rewind's result,
// and a message can be marked as an answer to the agent's question.
// See ui/neuron-tui/PORTING.md.
import type { ChatTheme } from './theme-system'
import type { ToolName } from './agent-run'
import type { GitView } from './git-view'
import type { ReactNode } from 'react'

/**
 * isCollapsed/userOpened are duplicated across block types intentionally - each UI
 * element tracks collapse state independently for different defaults and to persist
 * user intent vs programmatic state.
 */

export type ChatVariant = 'ai' | 'user' | 'agent' | 'error'

export type ThinkingCollapseState = 'expanded' | 'preview' | 'hidden'

export type TextContentBlock = {
  type: 'text'
  content: string
  color?: string
  marginTop?: number
  marginBottom?: number
  status?: 'running' | 'complete'
  textType?: 'reasoning' | 'text'
  isCollapsed?: boolean
  thinkingId?: string
  userOpened?: boolean
  /** True if this is a reasoning block from a <think> tag that hasn't been closed yet */
  thinkingOpen?: boolean
  thinkingCollapseState?: ThinkingCollapseState
}
/** Renders dynamic React content. NOT serializable - don't use for persistent data. */
export type HtmlContentBlock = {
  type: 'html'
  marginTop?: number
  marginBottom?: number
  render: (context: { textColor: string; theme: ChatTheme }) => ReactNode
}
export type ToolContentBlock = {
  type: 'tool'
  toolCallId: string
  toolName: ToolName
  input: any
  output?: string
  outputRaw?: unknown
  /**
   * The engine marked the result an error (`is_error`): a refused or denied
   * call, a failed tool, a non-zero exit. The renderers show it as a failure.
   */
  isError?: boolean
  /** A command's exit code, when it ran to the end and failed. */
  exitCode?: number
  /** Why a command stopped otherwise: "timed out", "killed", "interrupted", "blocked: …". */
  stopReason?: string
  /**
   * The model is still writing this call's input (`tool_input_progress`):
   * `input` is empty until the complete one arrives and replaces this.
   */
  liveInput?: ToolInputSoFar
  /** An `Agent` call's sub-agent, as the engine reports it. */
  subagent?: SubagentState
  agentId?: string
  includeToolCall?: boolean
  isCollapsed?: boolean
  userOpened?: boolean
}
/** How far the model got writing a tool call's input. */
export type ToolInputSoFar = {
  bytes: number
  /** Lines of the call's main text: a file's content, an edit's new text, a command. */
  lines: number
  /** A path, command, pattern or URL, once written. */
  target: string | null
  /** The last lines of the main text so far. */
  preview: string | null
}
/** A sub-agent an `Agent` call runs. */
export type SubagentState = {
  description: string
  subagentType: string | null
  /** `running` until it ends: `completed`, `failed`, or `running` in the background. */
  status: string
  finished: boolean
  isError: boolean
  /** Its final text, when it finished with one. */
  result: string | null
  /** Why it stopped early, or that it went on in the background. */
  note: string | null
}
export type AgentContentBlock = {
  type: 'agent'
  agentId: string
  agentName: string
  agentType: string
  content: string
  status: 'running' | 'complete' | 'failed' | 'cancelled'
  blocks?: ContentBlock[]
  initialPrompt?: string
  params?: Record<string, any>
  isCollapsed?: boolean
  userOpened?: boolean
  /** The spawn_agents tool call ID that created this block, used to match results */
  spawnToolCallId?: string
  /** The index within the spawn_agents call, used to match the correct result */
  spawnIndex?: number
}
export type AgentListContentBlock = {
  type: 'agent-list'
  id: string
  agents: Array<{ id: string; displayName: string }>
  agentsDir: string
  isCollapsed?: boolean
  userOpened?: boolean
}
export type ModeDividerContentBlock = {
  type: 'mode-divider'
  mode: string
}

export type PlanContentBlock = {
  type: 'plan'
  content: string
}

export type AskUserContentBlock = {
  type: 'ask-user'
  toolCallId: string
  questions: Array<{
    question: string
    header?: string
    options: Array<{
      label: string
      description?: string
    }>
    multiSelect?: boolean
    validation?: {
      maxLength?: number
      minLength?: number
      pattern?: string
      patternError?: string
    }
  }>
  answers?: Array<{
    questionIndex: number
    selectedOption?: string
    selectedOptions?: string[]
    otherText?: string
  }>
  skipped?: boolean
  /**
   * Set (possibly empty) on the engine's AskUserQuestion, which is answered
   * in the input: how to answer ("Answer with 1-2 or in your own words.").
   */
  hint?: string
}

export type ImageContentBlock = {
  type: 'image'
  image: string // base64 encoded
  mediaType: string
  filename?: string
  size?: number
  width?: number
  height?: number
  isCollapsed?: boolean
  userOpened?: boolean
}

/** A `/diff` or a rewind's result, drawn by components/git-view-cell.tsx. */
export type GitViewContentBlock = {
  type: 'git-view'
  view: GitView
}

export type ImageAttachment = {
  filename: string
  path: string
  size?: number
}

export type TextAttachment = {
  id: string
  content: string
  preview: string
  charCount: number
}

export type FileAttachment = {
  path: string
  filename: string
  isDirectory: boolean
  note?: string
}

export type ContentBlock =
  | AgentContentBlock
  | AgentListContentBlock
  | AskUserContentBlock
  | GitViewContentBlock
  | HtmlContentBlock
  | ImageContentBlock
  | ModeDividerContentBlock
  | TextContentBlock
  | ToolContentBlock
  | PlanContentBlock

export type AgentMessage = {
  agentName: string
  agentType: string
  responseCount: number
  subAgentCount?: number
}

export type ChatMessageMetadata = {
  /** Working directory where a bash command was executed */
  bashCwd?: string
  /** Whether this message/agent is collapsed in the UI */
  isCollapsed?: boolean
  /** Whether the user manually opened this collapsed item */
  userOpened?: boolean
  /** RunState stored after completion */
  runState?: unknown
  /**
   * A user message that answered the agent's question rather than starting a
   * turn, so it has no checkpoint of its own.
   */
  answersQuestion?: boolean
}

export type ChatMessage = {
  id: string
  variant: ChatVariant
  content: string
  blocks?: ContentBlock[]
  timestamp: string
  parentId?: string
  agent?: AgentMessage
  isCompletion?: boolean
  completionTime?: string
  isComplete?: boolean
  metadata?: ChatMessageMetadata
  validationErrors?: Array<{ id: string; message: string }>
  /**
   * UI-only runtime error displayed in UserErrorBanner (not sent to LLM).
   * Set by setError() when an error occurs during message streaming.
   * Can be cleared by clearUserError() when starting a new successful interaction.
   */
  userError?: string
  /**
   * The engine's refusal line, when the model declined to continue this
   * turn's request (a `refusal` notice). Drawn apart from the answer in
   * RefusalBanner; the answer keeps what the model wrote before it.
   */
  refusal?: string
  attachments?: ImageAttachment[]
  textAttachments?: TextAttachment[]
  fileAttachments?: FileAttachment[]
}

// Type guard functions for safe type narrowing
export function isTextBlock(block: ContentBlock): block is TextContentBlock {
  return block.type === 'text'
}

export function isImageBlock(block: ContentBlock): block is ImageContentBlock {
  return block.type === 'image'
}
