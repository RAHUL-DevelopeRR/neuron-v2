// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: it also
// draws the engine's AskUserQuestion while it waits for the answer typed in
// the input (engine/questions.ts). See ui/neuron-tui/PORTING.md.
import { pluralize } from '@neuron-tui/common/util/string'
import { TextAttributes } from '@opentui/core'
import React from 'react'

import { useTheme } from '../../hooks/use-theme'
import { GLYPHS } from '../../utils/glyphs'
import { BORDER_CHARS } from '../../utils/ui-constants'

import type { AskUserContentBlock } from '../../types/chat'

interface AskUserBranchProps {
  block: AskUserContentBlock
  availableWidth: number
}

export const AskUserBranch = ({ block, availableWidth }: AskUserBranchProps) => {
  const theme = useTheme()

  return (
    <box
      style={{
        flexDirection: 'column',
        gap: 0,
        width: Math.max(10, availableWidth - 2),
        borderStyle: 'single',
        borderColor: theme.secondary,
        customBorderChars: BORDER_CHARS,
        padding: 1,
        marginTop: 1,
        marginBottom: 1,
      }}
    >
      {!block.skipped && !block.answers ? (
        // The engine's question, waiting for its answer in the input.
        <box style={{ flexDirection: 'column', gap: 0 }}>
          <text style={{ fg: theme.secondary, attributes: TextAttributes.BOLD }}>
            Question
          </text>
          {block.questions.map((q, idx) => (
            <box key={idx} style={{ flexDirection: 'column', gap: 0 }}>
              <text style={{ fg: theme.foreground, wrapMode: 'word' }}>{q.question}</text>
              {q.options.map((option, optionIdx) => (
                <text
                  key={optionIdx}
                  style={{ fg: theme.foreground, marginLeft: 2, wrapMode: 'word' }}
                >
                  {`${optionIdx + 1}. ${option.label}`}
                </text>
              ))}
            </box>
          ))}
          <text style={{ fg: theme.muted, wrapMode: 'word' }}>
            {block.hint
              ? `${block.hint} Type it below and press Enter.`
              : 'Type your answer below and press Enter.'}
          </text>
        </box>
      ) : block.skipped ? (
        <box style={{ flexDirection: 'column', gap: 0 }}>
          {block.hint !== undefined &&
            block.questions.map((q, idx) => (
              <text key={idx} style={{ fg: theme.foreground, wrapMode: 'word' }}>
                {q.question}
              </text>
            ))}
          <text style={{ fg: theme.muted, attributes: TextAttributes.ITALIC }}>
            {block.hint !== undefined
              ? 'Not answered.'
              : `You skipped the ${pluralize(block.questions.length, 'question', { includeCount: false })}.`}
          </text>
        </box>
      ) : (
        <box style={{ flexDirection: 'column', gap: 1 }}>
          <text style={{ fg: theme.secondary, attributes: TextAttributes.BOLD }}>
            Your {pluralize(block.questions.length, 'answer', { includeCount: false })}:
          </text>
          {block.questions.map((q, idx) => {
            const answer = block.answers?.find((a) => a.questionIndex === idx)

            // Determine display answer based on answer type
            let displayAnswer: string
            let isCustomAnswer = false

            if (answer?.otherText) {
              // Custom text input
              displayAnswer = `"${answer.otherText}"`
              isCustomAnswer = true
            } else if (answer?.selectedOptions) {
              // Multi-select: join options with commas
              displayAnswer = answer.selectedOptions.join(', ')
            } else if (answer?.selectedOption) {
              // Single-select
              displayAnswer = answer.selectedOption
            } else {
              displayAnswer = 'No answer'
            }

            return (
              <box key={idx} style={{ flexDirection: 'column', gap: 0 }}>
                <text style={{ fg: theme.foreground }}>
                  {idx + 1}. {q.question}
                </text>
                <text style={{
                  fg: theme.primary,
                  marginLeft: 2,
                  attributes: isCustomAnswer ? TextAttributes.ITALIC : undefined,
                }}>
                  {GLYPHS.answer} {displayAnswer}
                </text>
              </box>
            )
          })}
        </box>
      )}
    </box>
  )
}
