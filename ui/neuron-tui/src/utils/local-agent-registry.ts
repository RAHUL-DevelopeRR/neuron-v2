// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
//
// Freebuff loaded Codebuff agent definitions here: the agents bundled into the
// binary, the Freebuff catalog's root agent, and repository `.agents/`
// directories (executed on import, with their mcp.json servers). Neuron's
// engine owns agents, so this registry is empty until the engine reports them.

export interface LocalAgentInfo {
  id: string
  displayName: string
  filePath: string
  /** True if this is a bundled agent (not user-created) */
  isBundled?: boolean
}

/** Agents for the '@' menu. */
export const loadLocalAgents = (): LocalAgentInfo[] => []

export const getLoadedAgentsData = (): {
  agents: LocalAgentInfo[]
  agentsDir: string
} | null => null
