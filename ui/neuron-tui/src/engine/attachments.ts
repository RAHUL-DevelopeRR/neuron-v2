/**
 * What a prompt's attachments send to the engine as `Submit.attachments`.
 *
 * Files, folders and pasted text are still inlined into the prompt text, as
 * Freebuff did: the model gets their contents, where the engine would only
 * list their paths. Images are not text, so they go as attachments; the
 * engine lists them for the model until it can send images.
 */

import path from 'path'

import type { Attachment } from './protocol'
import type { PendingAttachment } from '../types/store'

const IMAGE_TYPES: Record<string, string> = {
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.bmp': 'image/bmp',
}

export function engineAttachments(attachments: PendingAttachment[] | undefined): Attachment[] {
  const sent: Attachment[] = []
  for (const attachment of attachments ?? []) {
    if (attachment.kind !== 'image' || attachment.status !== 'ready') continue
    sent.push({
      kind: 'image',
      path: attachment.path,
      media_type:
        attachment.processedImage?.mediaType ??
        IMAGE_TYPES[path.extname(attachment.path).toLowerCase()] ??
        'application/octet-stream',
    })
  }
  return sent
}
