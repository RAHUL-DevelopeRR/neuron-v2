// Derived from Codebuff/Freebuff common/src/tools/params/tool/render-ui.ts
// (Apache-2.0); modified for Neuron: only the resolved button widget the UI
// renders is kept. The model-facing tool description and the gravity_index
// (ad network) link references are gone. See ui/neuron-tui/PORTING.md.
import z from 'zod/v4'

const buttonLinkSchema = z
  .string()
  .url()
  .refine(
    (value) => {
      try {
        const url = new URL(value)
        return url.protocol === 'https:' || url.protocol === 'http:'
      } catch {
        return false
      }
    },
    { message: 'Button links must use http:// or https://' },
  )

const buttonWidgetBaseSchema = z.object({
  type: z
    .literal('button')
    .describe('Widget type. Currently, the only supported widget is button.'),
  text: z
    .string()
    .min(1)
    .max(80)
    .describe('Short button label shown to the user.'),
  variant: z
    .enum(['primary', 'secondary'])
    .optional()
    .default('primary')
    .describe(
      'Theme-aware color treatment. Use primary for the main action and secondary for lower-emphasis actions.',
    ),
})

const resolvedButtonWidgetSchema = buttonWidgetBaseSchema.extend({
  link: buttonLinkSchema.describe(
    'The http:// or https:// URL to open when the user clicks the button.',
  ),
})

export type RenderUIButtonWidget = z.infer<typeof resolvedButtonWidgetSchema>

/**
 * Validates the resolved widget shape consumed by UI surfaces. Keeping this
 * beside the tool schema gives CLI, Chat, Web, and Cloud one runtime contract.
 */
export function parseRenderUIButtonWidget(
  widget: unknown,
): RenderUIButtonWidget | null {
  const rawLink =
    widget && typeof widget === 'object' && !Array.isArray(widget)
      ? (widget as { link?: unknown }).link
      : undefined
  if (typeof rawLink !== 'string') return null

  const result = resolvedButtonWidgetSchema.safeParse(widget)
  if (!result.success || result.data.link !== rawLink) return null

  const text = result.data.text.trim()
  return text ? { ...result.data, text } : null
}
