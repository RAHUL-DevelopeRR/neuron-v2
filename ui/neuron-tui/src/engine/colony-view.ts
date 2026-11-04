/**
 * A Colony run's live rows (`/colony <task>`): one message in the transcript
 * that the `colony_*` events keep current while the run goes, with a row per
 * agent (planner, workers, reviewers, synthesis), its status and its tokens of
 * its budget, and the run's total against its cap. The run's questions come as
 * notices like the agent's own; the full report follows as a notice when it
 * ends.
 */

import { getSystemMessage } from '../utils/message-history'

import type { ColonyAgentInfo, UiEvent } from './protocol'
import type { ChatMessage } from '../types/chat'

/** What the rows show. */
export interface ColonyView {
  runId: string
  workers: number | null
  tokenCap: number | null
  used: number
  tasks: number | null
  agents: ColonyAgentInfo[]
  /** Set when the run ended. */
  status: string | null
  merged: boolean
  branch: string | null
  conflict: string | null
}

const emptyView = (runId: string): ColonyView => ({
  runId,
  workers: null,
  tokenCap: null,
  used: 0,
  tasks: null,
  agents: [],
  status: null,
  merged: false,
  branch: null,
  conflict: null,
})

/** The view after `event`; events that are not a Colony run's leave it as it is. */
export function applyColonyEvent(view: ColonyView | null, event: UiEvent): ColonyView | null {
  switch (event.type) {
    case 'colony_started':
      return {
        ...(view ?? emptyView(event.run_id)),
        runId: event.run_id || view?.runId || '',
        workers: event.workers,
        tokenCap: event.token_cap,
        tasks: event.tasks.length,
      }
    case 'colony_agent': {
      const base = view ?? emptyView(event.run_id)
      const index = base.agents.findIndex((agent) => agent.id === event.agent.id)
      const agents =
        index < 0
          ? [...base.agents, event.agent]
          : base.agents.map((agent, position) => (position === index ? event.agent : agent))
      const used = agents.reduce((total, agent) => total + agent.tokens_used, 0)
      return { ...base, runId: base.runId || event.run_id, agents, used: Math.max(base.used, used) }
    }
    case 'colony_budget': {
      const base = view ?? emptyView(event.run_id)
      return { ...base, used: event.used, tokenCap: event.limit }
    }
    case 'colony_finished': {
      const base = view ?? emptyView(event.run_id)
      return {
        ...base,
        status: event.status,
        used: event.tokens_used,
        tokenCap: event.token_cap,
        merged: event.merged,
        branch: event.branch,
        conflict: event.conflict,
      }
    }
    default:
      return view
  }
}

const count = (value: number): string => value.toLocaleString('en-US')

/** The rows as text, one line per agent under a heading with the totals. */
export function renderColonyView(view: ColonyView): string {
  const heading = [
    view.status ? `Colony run ${view.status}` : 'Colony run',
    view.tasks !== null ? `${view.tasks} subtask${view.tasks === 1 ? '' : 's'}` : null,
    view.workers !== null ? `${view.workers} at a time` : null,
    view.tokenCap !== null
      ? `${count(view.used)} / ${count(view.tokenCap)} tokens`
      : `${count(view.used)} tokens`,
  ]
    .filter(Boolean)
    .join(' · ')
  const rows = view.agents.map((agent) => {
    const tokens =
      agent.token_budget !== null
        ? `${count(agent.tokens_used)} / ${count(agent.token_budget)} tokens`
        : `${count(agent.tokens_used)} tokens`
    const parts = [agent.role, agent.title, agent.status, tokens]
    if (agent.model) parts.push(agent.model)
    if (agent.branch) parts.push(agent.branch)
    if (agent.note) parts.push(agent.note)
    return `- ${parts.join(' · ')}`
  })
  const footer: string[] = []
  if (view.conflict) footer.push(`Merging stopped at a conflict in ${view.conflict}.`)
  if (view.merged) footer.push('The changes are in your working tree, not committed.')
  else if (view.branch && view.status) footer.push(`The changes are on branch ${view.branch}.`)
  return [heading, ...rows, ...footer].join('\n')
}

type SetMessages = (update: (previous: ChatMessage[]) => ChatMessage[]) => void

/**
 * An event handler for the `/colony` exchange: the first Colony event adds the
 * rows' message, later ones update it in place.
 */
export function colonyEventHandler(setMessages: SetMessages): (event: UiEvent) => void {
  let view: ColonyView | null = null
  let messageId: string | null = null
  return (event) => {
    const next = applyColonyEvent(view, event)
    if (next === view || next === null) return
    view = next
    const content = renderColonyView(next)
    if (messageId === null) {
      const message = getSystemMessage(content)
      messageId = `colony-${next.runId || message.id}`
      const added = { ...message, id: messageId }
      setMessages((previous) => [...previous, added])
      return
    }
    const id = messageId
    setMessages((previous) =>
      previous.map((message) => (message.id === id ? { ...message, content } : message)),
    )
  }
}
