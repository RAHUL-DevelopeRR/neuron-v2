// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import type { PendingAttachment } from '../store'
import type { AgentMode } from '../../utils/constants'
import type { ChatMessage } from '../chat'

export type PostUserMessageFn = (prev: ChatMessage[]) => ChatMessage[]

export type SendMessageFn = (params: {
  content: string
  agentMode: AgentMode
  postUserMessage?: PostUserMessageFn
  attachments?: PendingAttachment[]
}) => Promise<void>
