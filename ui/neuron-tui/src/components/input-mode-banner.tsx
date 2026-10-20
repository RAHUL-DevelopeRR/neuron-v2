// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import React from 'react'

import { PendingAttachmentsBanner } from './pending-attachments-banner'
import { useChatStore } from '../state/chat-store'

/**
 * Registry mapping input modes to their banner components.
 *
 * To add a new banner:
 * 1. Create the banner component using BottomBanner
 * 2. Add an entry here mapping the input mode to a render function
 *
 * Render functions receive context (like showTime) and return the component.
 */
const BANNER_REGISTRY: Record<
  string,
  (ctx: { showTime: number }) => React.ReactNode
> = {
  default: () => <PendingAttachmentsBanner />,
  image: () => <PendingAttachmentsBanner />,
}

/**
 * Banner component that shows contextual information below the input box.
 * Shows mode-specific banners based on the current input mode.
 *
 * Uses a registry pattern for easy extensibility - add new banners by
 * updating BANNER_REGISTRY above.
 */
export const InputModeBanner = () => {
  const inputMode = useChatStore((state) => state.inputMode)

  const renderBanner = BANNER_REGISTRY[inputMode]

  if (!renderBanner) {
    return null
  }

  return <>{renderBanner({ showTime: Date.now() })}</>
}
