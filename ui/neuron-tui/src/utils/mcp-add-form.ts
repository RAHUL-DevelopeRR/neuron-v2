/**
 * The MCP panel's Add tab as state and keys, without a renderer: search the
 * connector catalog, pick a connector, fill its variables (secret ones
 * masked), name it and pick where it is saved, then add it.
 *
 *   search:  type to filter · ↑/↓ move · Enter pick · Tab Installed · Esc close
 *   form:    ↑/↓ or Tab field · type or paste · ←/→ scope · Enter add · Esc back
 *
 * The catalog is asked for once (all connectors) and filtered here, every word
 * of the query against the id, name, publisher and description, as the
 * engine's own search does.
 */

import { CONNECTOR_SCOPES, type ConnectorForm, type ConnectorScope } from '../engine/mcp-panel'

import type { ConnectorInfo, ConnectorVariable } from '../engine/protocol'

export interface AddTabState {
  stage: 'search' | 'form'
  query: string
  /** The focused connector among the search's matches. */
  focus: number
  /** The connector the form is for. */
  connectorId: string | null
  /** The focused field of the form: its variables, then the name, then the scope. */
  field: number
  form: ConnectorForm
}

export const ADD_TAB_START: AddTabState = {
  stage: 'search',
  query: '',
  focus: 0,
  connectorId: null,
  field: 0,
  form: { name: '', scope: 'user', values: {} },
}

/** A key as the panel gets it (opentui's KeyEvent, the parts used here). */
export interface PanelKey {
  name?: string
  sequence?: string
  ctrl?: boolean
  meta?: boolean
  shift?: boolean
}

/** What the Add tab asks the panel to do after a key. */
export type AddTabEffect =
  | { type: 'none' }
  | { type: 'close' }
  | { type: 'installed' }
  | { type: 'add'; connector: ConnectorInfo; form: ConnectorForm }

/** The catalog's connectors matching every word of `query`. */
export function filterConnectors(connectors: ConnectorInfo[], query: string): ConnectorInfo[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (words.length === 0) return connectors
  return connectors.filter((connector) => {
    const haystack =
      `${connector.id} ${connector.name} ${connector.publisher} ${connector.description}`.toLowerCase()
    return words.every((word) => haystack.includes(word))
  })
}

/** One field of the form, as drawn. */
export type FormField =
  | { kind: 'variable'; variable: ConnectorVariable }
  | { kind: 'name' }
  | { kind: 'scope' }

export function formFields(connector: ConnectorInfo): FormField[] {
  return [
    ...connector.variables.map((variable) => ({ kind: 'variable' as const, variable })),
    { kind: 'name' as const },
    { kind: 'scope' as const },
  ]
}

/** A secret's value as the form shows it: one mask character per character, at most 32. */
export function maskSecret(value: string, mask = '•'): string {
  const length = [...value].length
  if (length === 0) return ''
  return length > 32 ? `${mask.repeat(32)}…` : mask.repeat(length)
}

/** The text a key types: a printable character, or `null` for any other key. */
function typedText(key: PanelKey): string | null {
  if (key.ctrl || key.meta) return null
  if (key.name === 'space') return ' '
  const sequence = key.sequence ?? ''
  // One printable character (not an escape sequence or a control character).
  if ([...sequence].length !== 1) return null
  return /[\p{L}\p{N}\p{P}\p{S} ]/u.test(sequence) ? sequence : null
}

const isEnter = (key: PanelKey) => key.name === 'return' || key.name === 'enter'

function withField(state: AddTabState, field: FormField | undefined, edit: (text: string) => string): AddTabState {
  if (!field || field.kind === 'scope') return state
  if (field.kind === 'name') return { ...state, form: { ...state.form, name: edit(state.form.name) } }
  const name = field.variable.name
  return {
    ...state,
    form: {
      ...state.form,
      values: { ...state.form.values, [name]: edit(state.form.values[name] ?? '') },
    },
  }
}

function cycleScope(scope: ConnectorScope, delta: number): ConnectorScope {
  const index = CONNECTOR_SCOPES.indexOf(scope)
  const next = (index + delta + CONNECTOR_SCOPES.length) % CONNECTOR_SCOPES.length
  return CONNECTOR_SCOPES[next]!
}

/** The form for `connector`, its defaults filled in. */
export function openForm(state: AddTabState, connector: ConnectorInfo): AddTabState {
  const values: Record<string, string> = {}
  for (const variable of connector.variables) {
    if (variable.default && !variable.secret) values[variable.name] = variable.default
  }
  const scope = (CONNECTOR_SCOPES as readonly string[]).includes(connector.default_scope)
    ? (connector.default_scope as ConnectorScope)
    : 'user'
  return {
    ...state,
    stage: 'form',
    connectorId: connector.id,
    field: 0,
    form: { name: '', scope, values },
  }
}

/** Text pasted into the focused field (a token, usually); newlines are dropped. */
export function pasteIntoAddTab(state: AddTabState, connectors: ConnectorInfo[], text: string): AddTabState {
  const clean = text.replace(/[\r\n]+/g, '').trim()
  if (!clean) return state
  if (state.stage === 'search') return { ...state, query: state.query + clean, focus: 0 }
  const connector = connectors.find((entry) => entry.id === state.connectorId)
  if (!connector) return state
  return withField(state, formFields(connector)[state.field], (value) => value + clean)
}

/** The next state for `key`, and what the panel should do. */
export function addTabKey(
  state: AddTabState,
  key: PanelKey,
  connectors: ConnectorInfo[],
): { state: AddTabState; effect: AddTabEffect } {
  const none = (next: AddTabState) => ({ state: next, effect: { type: 'none' } as AddTabEffect })
  if (key.ctrl && !key.meta && key.name === 'c') return { state, effect: { type: 'close' } }

  if (state.stage === 'search') {
    const matches = filterConnectors(connectors, state.query)
    if (key.name === 'escape') {
      return state.query
        ? none({ ...state, query: '', focus: 0 })
        : { state, effect: { type: 'close' } }
    }
    if (key.name === 'tab') return { state, effect: { type: 'installed' } }
    if (key.name === 'up') return none({ ...state, focus: Math.max(0, state.focus - 1) })
    if (key.name === 'down') {
      return none({ ...state, focus: Math.max(0, Math.min(matches.length - 1, state.focus + 1)) })
    }
    if (isEnter(key)) {
      const connector = matches[state.focus]
      return connector ? none(openForm(state, connector)) : none(state)
    }
    if (key.name === 'backspace') {
      return none({ ...state, query: [...state.query].slice(0, -1).join(''), focus: 0 })
    }
    const text = typedText(key)
    return text === null ? none(state) : none({ ...state, query: state.query + text, focus: 0 })
  }

  const connector = connectors.find((entry) => entry.id === state.connectorId)
  if (!connector) return none({ ...state, stage: 'search' })
  const fields = formFields(connector)
  const field = fields[state.field]
  if (key.name === 'escape') return none({ ...state, stage: 'search', connectorId: null })
  if (key.name === 'up' || (key.name === 'tab' && key.shift)) {
    return none({ ...state, field: (state.field - 1 + fields.length) % fields.length })
  }
  if (key.name === 'down' || key.name === 'tab') {
    return none({ ...state, field: (state.field + 1) % fields.length })
  }
  if (isEnter(key)) {
    return { state, effect: { type: 'add', connector, form: state.form } }
  }
  if (field?.kind === 'scope') {
    if (key.name === 'left') return none({ ...state, form: { ...state.form, scope: cycleScope(state.form.scope, -1) } })
    if (key.name === 'right' || key.name === 'space') {
      return none({ ...state, form: { ...state.form, scope: cycleScope(state.form.scope, 1) } })
    }
    return none(state)
  }
  if (key.name === 'backspace') {
    return none(withField(state, field, (value) => [...value].slice(0, -1).join('')))
  }
  const text = typedText(key)
  return text === null ? none(state) : none(withField(state, field, (value) => value + text))
}
