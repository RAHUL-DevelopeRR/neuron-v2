/**
 * The host's pairing tokens and the files they are kept in (`bun run host`).
 *
 * There are two, one for each role (see `HostOptions`): the owner's token, which the desktop app reads, and an optional
 * peer token for a bridge such as `neuron engine --share`, whose phones reach the host from 127.0.0.1. The peer token
 * exists only when a file is named for it, so nothing lies around that no bridge needs.
 */

import { randomBytes } from 'crypto'
import fs from 'fs'
import path from 'path'

/** A new pairing token: 24 random bytes, base64url. */
export const newToken = (): string => randomBytes(24).toString('base64url')

export interface HostTokens {
  /** The owner's token and the file it was written to. */
  token: string
  tokenFile: string
  /** The peer token and its file, when a file was named for one. */
  peerToken?: string
  peerTokenFile?: string
}

const samePath = (a: string, b: string): boolean => (process.platform === 'win32' ? a.toLowerCase() === b.toLowerCase() : a === b)

/**
 * Makes the host's tokens and writes each to its own file, readable only by the user. The two files must differ: in
 * one file, somebody would be handed the token of the other role.
 */
export function writeHostTokens(options: { tokenFile: string; peerTokenFile?: string | null }): HostTokens {
  const tokenFile = path.resolve(options.tokenFile)
  const peerTokenFile = options.peerTokenFile ? path.resolve(options.peerTokenFile) : undefined
  if (peerTokenFile !== undefined && samePath(tokenFile, peerTokenFile)) {
    throw new Error('The peer token file must be a different file from the owner token file.')
  }
  const token = newToken()
  fs.writeFileSync(tokenFile, token, { mode: 0o600 })
  if (peerTokenFile === undefined) return { token, tokenFile }
  const peerToken = newToken()
  fs.writeFileSync(peerTokenFile, peerToken, { mode: 0o600 })
  return { token, tokenFile, peerToken, peerTokenFile }
}

/** Deletes the token files `writeHostTokens` made. */
export function removeHostTokens(tokens: HostTokens): void {
  fs.rmSync(tokens.tokenFile, { force: true })
  if (tokens.peerTokenFile) fs.rmSync(tokens.peerTokenFile, { force: true })
}
