import { BottomBanner } from './bottom-banner'
import { FileAttachmentCard } from './file-attachment-card'
import { ImageCard } from './image-card'
import { TextAttachmentCard } from './text-attachment-card'
import { useTheme } from '../hooks/use-theme'
import { useAttachStore } from '../state/attach-store'
import { useChatStore } from '../state/chat-store'
import { GLYPHS } from '../utils/glyphs'

import type {
  PendingFileAttachment,
  PendingImageAttachment,
  PendingTextAttachment,
} from '../types/store'

/**
 * Combined banner for both image and text attachments.
 * Displays all attachments in a single horizontal row.
 */
export const PendingAttachmentsBanner = () => {
  const theme = useTheme()
  const pendingAttachments = useChatStore((state) => state.pendingAttachments)
  const removePendingAttachment = useChatStore(
    (state) => state.removePendingAttachment,
  )
  const attachNotices = useAttachStore((state) => state.notices)
  const clearAttachNotices = useAttachStore((state) => state.clearNotices)

  // Split and categorize attachments
  const pendingImages = pendingAttachments.filter(
    (a): a is PendingImageAttachment => a.kind === 'image',
  )
  const pendingTextAttachments = pendingAttachments.filter(
    (a): a is PendingTextAttachment => a.kind === 'text',
  )
  const pendingFileAttachments = pendingAttachments.filter(
    (a): a is PendingFileAttachment => a.kind === 'file',
  )

  // Separate error messages from actual images
  const errorImages: PendingImageAttachment[] = []
  const validImages: PendingImageAttachment[] = []
  for (const img of pendingImages) {
    if (img.status === 'error') {
      errorImages.push(img)
    } else {
      validImages.push(img)
    }
  }

  const hasValidImages = validImages.length > 0
  const hasTextAttachments = pendingTextAttachments.length > 0
  const hasFileAttachments = pendingFileAttachments.length > 0
  const hasErrorsOnly =
    (errorImages.length > 0 || attachNotices.length > 0) &&
    !hasValidImages &&
    !hasTextAttachments &&
    !hasFileAttachments

  // Files the Attach button could not attach, and why.
  const noticeLines = attachNotices.map((notice) => (
    <text key={`notice-${notice.id}`} style={{ fg: theme.warning, wrapMode: 'word' }}>
      {`${GLYPHS.warning} ${notice.text}`}
    </text>
  ))

  // Nothing to show
  if (
    !hasValidImages &&
    !hasTextAttachments &&
    !hasFileAttachments &&
    errorImages.length === 0 &&
    attachNotices.length === 0
  ) {
    return null
  }

  // If we only have errors (no valid attachments), show just the error messages
  if (hasErrorsOnly) {
    return (
      <BottomBanner
        borderColorKey={errorImages.length > 0 ? 'error' : 'warning'}
        onClose={attachNotices.length > 0 ? clearAttachNotices : undefined}
      >
        {/* One column, clear of the close button. */}
        <box style={{ flexDirection: 'column', flexShrink: 1, marginRight: 2 }}>
          {errorImages.map((image, index) => (
            <text key={`${image.path}-${index}`} style={{ fg: theme.error }}>
              {image.note} ({image.filename})
            </text>
          ))}
          {noticeLines}
        </box>
      </BottomBanner>
    )
  }

  return (
    <BottomBanner
      borderColorKey="imageCardBorder"
      onClose={attachNotices.length > 0 ? clearAttachNotices : undefined}
    >
      {noticeLines}
      {/* Error messages shown above the attachments */}
      {errorImages.map((image, index) => (
        <text key={`error-${image.path}-${index}`} style={{ fg: theme.error }}>
          {image.note} ({image.filename})
        </text>
      ))}

      {/* All attachment cards in a horizontal row */}
      <box
        style={{
          flexDirection: 'row',
          gap: 1,
          flexWrap: 'wrap',
        }}
      >
        {/* Image cards */}
        {validImages.map((image, index) => (
          <ImageCard
            key={`img-${image.path}-${index}`}
            image={image}
            onRemove={() => removePendingAttachment(image.path)}
          />
        ))}

        {/* Text attachment cards */}
        {pendingTextAttachments.map((attachment) => (
          <TextAttachmentCard
            key={attachment.id}
            attachment={attachment}
            onRemove={() => removePendingAttachment(attachment.id)}
          />
        ))}

        {/* File/folder attachment cards */}
        {pendingFileAttachments.map((attachment) => (
          <FileAttachmentCard
            key={attachment.id}
            attachment={attachment}
            onRemove={() => removePendingAttachment(attachment.path)}
          />
        ))}
      </box>
    </BottomBanner>
  )
}
