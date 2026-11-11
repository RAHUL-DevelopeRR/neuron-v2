/*
 * Ported from Freebuff, common/src/desktop-cloud/project-name.ts, commit 0dc95cb
 * (https://github.com/CodebuffAI/freebuff). Copyright Codebuff, Apache License 2.0.
 * Changed for Neuron: only `projectNameFromPrompt` and the word lists it reads are
 * kept (renamed `copyNameFromPrompt`); the "Untitled" helpers are left out. Its
 * result names a chat's copy of the project: the desktop host slugifies it into
 * the branch `neuron/<slug>` (src-tauri/src/worktree.rs). See ../NOTICE.
 */

const LEAD_INS = [
  /^(?:hi|hey|hello)\b[,!.]?\s*/i,
  /^(?:please|pls)\s+/i,
  /^(?:can|could|would|will) you\s+(?:please\s+)?/i,
  /^i(?:'d| would)? (?:like|want|need)(?: you)? to\s+/i,
  /^(?:help me|let's|lets)\s+/i,
  /^(?:build|make|create|write|code|design|develop|generate|set up|start|add|implement|draft)\s+(?:me\s+|us\s+)?/i,
  /^(?:a|an|the|my|our|some|simple|quick|basic|new)\s+/i,
]

/** Words that end what is being asked for ("a snake game THAT…"). */
const STOP_WORDS = new Set([
  'that', 'which', 'where', 'who', 'with', 'for', 'using', 'in', 'on', 'to', 'so', 'i', 'we', 'you',
  'and', 'from', 'like', 'by', 'about', 'when', 'it', 'please',
])

const SMALL_WORDS = new Set(['a', 'an', 'the', 'of', 'and', 'or', 'to', 'in', 'on', 'for'])

/**
 * A short name for a chat's copy from the first prompt: what is being asked
 * for, title-cased ("fix the login redirect for admins" → "Fix the Login
 * Redirect"). At most four words and 40 characters; `null` when the prompt has
 * no words.
 */
export function copyNameFromPrompt(prompt: string): string | null {
  let text = prompt.split(/[\n.!?]/).find((line) => line.trim())?.trim() ?? ''
  for (let changed = true; changed; ) {
    changed = false
    for (const lead of LEAD_INS) {
      const next = text.replace(lead, '')
      if (next !== text && next.trim()) {
        text = next.trim()
        changed = true
      }
    }
  }
  const words: string[] = []
  for (const raw of text.split(/\s+/)) {
    const word = raw.replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}'-]+$/gu, '')
    if (!word) continue
    if (words.length && STOP_WORDS.has(word.toLowerCase())) break
    words.push(word)
    if (words.length === 4) break
  }
  if (!words.length) return null
  const name = words
    .map((word, index) =>
      index > 0 && SMALL_WORDS.has(word.toLowerCase())
        ? word.toLowerCase()
        : /[A-Z]/.test(word.slice(1))
          ? word
          : word[0]!.toUpperCase() + word.slice(1),
    )
    .join(' ')
  return name.length > 40 ? name.slice(0, 40).trimEnd() : name
}
