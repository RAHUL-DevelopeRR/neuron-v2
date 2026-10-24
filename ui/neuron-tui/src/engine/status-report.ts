/**
 * What the UI adds to the engine's `/status` report: the Zero-X account (the
 * engine's report has none) and the folder's trust, with how to trust it.
 * Both come from the engine's last `status` event.
 */

import { needsSignIn } from '../state/engine-store'

import type { StatusEvent } from './protocol'

/** What an untrusted folder means, and how to trust it. There is no engine command for it. */
export const UNTRUSTED_FOLDER_EFFECT =
  "this folder's own settings, hooks and MCP servers (.neuron/, .neuron.json) are not loaded"
export const TRUST_FOLDER_HOW =
  'run `neuron --no-tui` in this folder once and press 1 when it asks, or add the folder ' +
  '(or one above it) to "trustedRoots" in ~/.neuron/settings.json; then restart Neuron'

/** The header's line about an untrusted folder; `/status` has the rest. */
export const UNTRUSTED_FOLDER_NOTE =
  "Untrusted folder: this folder's own settings, hooks and MCP servers are not loaded. " +
  '/status says how to trust it.'

const row = (label: string, value: string) => `  ${label.padEnd(17)}${value}`

/** `2026-11-30 14:02 UTC`. */
function formatExpiry(unixSeconds: number): string {
  return `${new Date(unixSeconds * 1000).toISOString().slice(0, 16).replace('T', ' ')} UTC`
}

function signedInRow(status: StatusEvent): string {
  const account = status.account ?? null
  if (status.signed_in && !needsSignIn(status)) {
    return account ? `yes (${account.source})` : 'yes'
  }
  if (needsSignIn(status)) {
    return account ? 'expired or revoked: type /login' : 'no: type /login'
  }
  return 'no (this session uses another provider)'
}

/** The lines `/status` gets after the engine's report, or `null` before any status. */
export function statusAddendum(status: StatusEvent | null): string | null {
  if (!status) return null
  const account = status.account ?? null
  const zeroX = ['Zero-X']
  if (account?.email) zeroX.push(row('Account', account.email))
  if (account?.plan) zeroX.push(row('Plan', account.plan))
  zeroX.push(row('Signed in', signedInRow(status)))
  if (account?.expires_at) zeroX.push(row('Session expires', formatExpiry(account.expires_at)))

  const trust = ['Workspace trust']
  if (status.workspace_trusted) {
    trust.push(row('Folder', 'trusted'))
  } else {
    trust.push(row('Folder', 'not trusted'))
    trust.push(row('Effect', UNTRUSTED_FOLDER_EFFECT))
    trust.push(row('To trust it', TRUST_FOLDER_HOW))
  }
  return [zeroX.join('\n'), trust.join('\n')].join('\n\n')
}
