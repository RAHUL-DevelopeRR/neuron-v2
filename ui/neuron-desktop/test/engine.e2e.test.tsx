/**
 * The desktop UI against the real `neuron engine --stdio` and the mock
 * Anthropic service, through a Neuron host: what `bun run dev:mock` shows in a
 * browser. Needs `cargo build -p rusty-claude-cli -p mock-anthropic-service`
 * in rust/; skipped without it.
 *
 * Waits poll CSS selectors: testing-library's role queries are too slow on the
 * whole app's DOM to poll.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'
import { cleanup, fireEvent, render, within } from '@testing-library/react'

import type { SocketLike } from '@neuron/client'
import { NeuronHost } from '../../neuron-client/src/node/host'
import { haveEngineBinaries, makeMockHome, neuronBinary, startMockService, type MockHome, type MockService } from '../../neuron-client/src/node/mock'
import { StdioTransport } from '../../neuron-client/src/node/stdio-transport'
import { App } from '../src/App'
import { hostPlatform } from '../src/platform/host'

const BunWebSocket = (globalThis as unknown as { BunWebSocket: new (url: string) => SocketLike }).BunWebSocket
const TOKEN = 'desktop-e2e-pairing-token'
let mock: MockService | null = null
let home: MockHome | null = null
let host: NeuronHost | null = null

async function until(label: string, predicate: () => boolean, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
}

async function element(selector: string): Promise<HTMLElement> {
  await until(selector, () => document.querySelector(selector) !== null)
  return document.querySelector<HTMLElement>(selector)!
}

const text = (selector: string) => document.querySelector(selector)?.textContent ?? ''

async function startApp() {
  mock = await startMockService()
  home = makeMockHome(mock.baseUrl, 'neuron-desktop-e2e')
  const transport = new StdioTransport({ binary: neuronBinary, cwd: home.workspace, model: 'sonnet', permissionMode: 'workspace-write', env: home.env })
  host = new NeuronHost({ transport, token: TOKEN, root: home.workspace, name: 'e2e' })
  await host.start()
  render(<App platform={hostPlatform({ url: host.url, token: TOKEN, connect: (url) => new BunWebSocket(url) })} />)
  await element('textarea[aria-label="Message Neuron"]')
  // A fresh home: the first-run steps, then the folder trust gate, as a new user sees them.
  fireEvent.click(await button('.first-run button', 'Get started'))
  fireEvent.click(await button('.first-run button', 'Use workspace'))
  fireEvent.click(await button('.trust-dialog button', 'Allow Neuron in this folder'))
  await until('the folder allowed', () => !document.querySelector('.trust-dialog'))
  await until('the message box on', () => !document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Neuron"]')!.disabled)
  return home
}

/** The first `selector` whose text starts with `label`, once there. */
async function button(selector: string, label: string): Promise<HTMLElement> {
  const find = () => [...document.querySelectorAll<HTMLElement>(selector)].find((el) => el.textContent?.trim().startsWith(label))
  await until(label, () => find() !== undefined)
  return find()!
}

function send(prompt: string) {
  const box = document.querySelector('textarea[aria-label="Message Neuron"]')!
  fireEvent.change(box, { target: { value: prompt } })
  fireEvent.keyDown(box, { key: 'Enter' })
}

afterEach(async () => {
  cleanup()
  localStorage.clear()
  await host?.stop()
  mock?.stop()
  home?.remove()
  host = null
  mock = null
  home = null
})

describe.skipIf(!haveEngineBinaries)('Neuron Desktop on the real engine', () => {
  test(
    'a turn that needs approval: the dialog, the approval, the result',
    async () => {
      const { workspace } = await startApp()
      expect((await element('.connection.ready')).textContent).toMatch(/^engine \d/)
      send('PARITY_SCENARIO:bash_permission_prompt_approved')
      const dialog = await element('[role="dialog"]')
      expect(within(dialog).getByText("$ printf 'approved via prompt' | tee approved.txt")).toBeTruthy()
      fireEvent.click(within(dialog).getByRole('button', { name: /allow once/i }))
      await until('the reply', () => text('[data-testid="assistant-message"]').includes('bash approved and executed'))
      await until('the turn to end', () => document.querySelector('.composer button.stop') === null)
      expect(text('[data-testid="tool-card"] .tool-state')).toBe('done')
      expect(document.querySelector('[role="dialog"]')).toBeNull()
      expect(fs.readFileSync(path.join(workspace, 'approved.txt'), 'utf8')).toBe('approved via prompt')
    },
    90_000,
  )

  test(
    'a file write: the diff in its card and the side panel, the file in the tree',
    async () => {
      await startApp()
      send('PARITY_SCENARIO:write_file_allowed')
      await until('the reply', () => text('[data-testid="assistant-message"]').includes('write_file succeeded'))
      expect(text('[data-testid="tool-card"] .tool-target')).toBe('generated/output.txt')
      expect(text('[data-testid="tool-card"] tr[data-kind="add"]')).toContain('created by mock service')
      // The card's diff in the side panel.
      fireEvent.click(await element('[data-testid="tool-card"] .link-button'))
      await until('the diff in the panel', () => text('.side-pane tr[data-kind="add"]').includes('created by mock service'))
      // The new file in the Files tab.
      fireEvent.click([...document.querySelectorAll<HTMLElement>('.tabs button')].find((tab) => tab.textContent === 'Files')!)
      await until('the folder in the tree', () =>
        [...document.querySelectorAll('.file-tree .tree-row')].some((row) => row.textContent?.includes('generated')),
      )
      fireEvent.click([...document.querySelectorAll<HTMLElement>('.file-tree .tree-row')].find((row) => row.textContent?.includes('generated'))!)
      await until('the file in the tree', () =>
        [...document.querySelectorAll('.file-tree .tree-row')].some((row) => row.textContent?.includes('output.txt')),
      )
      fireEvent.click([...document.querySelectorAll<HTMLElement>('.file-tree .tree-row')].find((row) => row.textContent?.includes('output.txt'))!)
      await until('the file', () => text('[data-testid="file-view"]') === 'created by mock service\n')
    },
    90_000,
  )

  test(
    'session history: a new session, the list, and back to the first one',
    async () => {
      await startApp()
      send('PARITY_SCENARIO:write_file_allowed')
      await until('the reply', () => text('[data-testid="assistant-message"]').includes('write_file succeeded'))
      await until('the session in the list', () => text('.session-list').includes('PARITY_SCENARIO:write_file_allowed'))
      fireEvent.click(await button('.nav-item .nav-label', 'New session'))
      await until('an empty session', () => document.querySelector('[data-testid="assistant-message"]') === null)
      await until('the old session in the list, not current', () =>
        [...document.querySelectorAll('.session-row:not(.current)')].some((row) => row.textContent?.includes('write_file_allowed')),
      )
      fireEvent.click([...document.querySelectorAll<HTMLElement>('.session-row:not(.current)')].find((row) => row.textContent?.includes('write_file_allowed'))!)
      await until('the old conversation', () => text('[data-testid="assistant-message"]').includes('write_file succeeded'))
      expect(text('[data-testid="tool-card"] .tool-state')).toBe('done')
      expect(text('.chat-header-title')).toBe('PARITY_SCENARIO:write_file_allowed')
    },
    90_000,
  )
})
