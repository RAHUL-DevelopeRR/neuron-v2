/**
 * `/update`: whether a newer Neuron release exists, and the command that
 * installs it (utils/update-check.ts).
 *
 * The command and a "Checking for updates..." line appear at once; the report
 * takes that line's place when the check ends, which can take a few seconds on
 * a network that drops packets. A message sent meanwhile stays below them.
 */

import { refreshUpdateCheck } from '../state/update-store'
import { copyTextToClipboard } from '../utils/clipboard'
import { getSystemMessage, getUserMessage } from '../utils/message-history'
import { formatUpdateReport, type UpdateCheckResult } from '../utils/update-check'

import type { ChatMessage } from '../types/chat'

export const UPDATE_CHECKING_TEXT = 'Checking for updates...'

export interface UpdateCommandDeps {
  setMessages: (value: ChatMessage[] | ((prev: ChatMessage[]) => ChatMessage[])) => void
  /** Reads the manifest again (`refreshUpdateCheck`). */
  refresh?: () => Promise<UpdateCheckResult>
  /** Copies the install command; `true` when it is surely on the clipboard. */
  copy?: (text: string) => Promise<boolean>
}

let pendingCount = 0

async function copyCommand(text: string): Promise<boolean> {
  return (await copyTextToClipboard(text, { suppressGlobalMessage: true })) !== 'unverified'
}

/** Runs `/update` for the typed `shown` text. Never throws. */
export async function runUpdateCommand(
  shown: string,
  { setMessages, refresh = refreshUpdateCheck, copy = copyCommand }: UpdateCommandDeps,
): Promise<void> {
  // Date.now() alone repeats within a millisecond; the counter keeps it unique.
  const pendingId = `sys-update-${Date.now()}-${++pendingCount}`
  setMessages((prev) => [
    ...prev,
    getUserMessage(shown),
    { ...getSystemMessage(UPDATE_CHECKING_TEXT), id: pendingId },
  ])

  let report: string
  try {
    const result = await refresh()
    let copied = false
    if (result.status === 'available') {
      try {
        copied = await copy(result.command)
      } catch {
        copied = false
      }
    }
    report = formatUpdateReport(result, copied)
  } catch {
    report = 'Could not check for updates: the check could not start.'
  }

  // The chat was cleared or replaced meanwhile: the report has no place left.
  setMessages((prev) =>
    prev.map((message) =>
      message.id === pendingId ? { ...getSystemMessage(report), id: pendingId } : message,
    ),
  )
}
