import { describe, expect, test } from 'bun:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { engineQuestionBlock, questionAnswer } from '../../engine/questions'
import { initializeThemeStore } from '../../hooks/use-theme'
import { AskUserBranch } from '../blocks/ask-user-branch'

initializeThemeStore()

const QUESTION =
  'Question: May I run the command?\n  1. Yes\n  2. No\n  Answer with 1-2 or in your own words.'

describe("AskUserBranch with the engine's question", () => {
  test('while it waits: the question, its options and how to answer', () => {
    const markup = renderToStaticMarkup(
      <AskUserBranch block={engineQuestionBlock(QUESTION, 'toolu_q')} availableWidth={80} />,
    )
    expect(markup).toContain('May I run the command?')
    expect(markup).toContain('1. Yes')
    expect(markup).toContain('2. No')
    expect(markup).toContain('Answer with 1-2 or in your own words. Type it below and press Enter.')
  })

  test('once answered: the answer under the question', () => {
    const block = engineQuestionBlock(QUESTION, 'toolu_q')
    const markup = renderToStaticMarkup(
      <AskUserBranch
        block={{ ...block, answers: [questionAnswer(block, '2')] }}
        availableWidth={80}
      />,
    )
    expect(markup).toContain('Your answer:')
    expect(markup).toContain('May I run the command?')
    expect(markup).toContain('↳ No')
  })

  test('not answered when the turn ended', () => {
    const block = engineQuestionBlock(QUESTION, 'toolu_q')
    const markup = renderToStaticMarkup(
      <AskUserBranch block={{ ...block, skipped: true }} availableWidth={80} />,
    )
    expect(markup).toContain('May I run the command?')
    expect(markup).toContain('Not answered.')
  })
})
