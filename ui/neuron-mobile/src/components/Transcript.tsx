import { memo, useState } from 'react'
import { Pressable, ScrollView, Text, View } from 'react-native'

import type { TranscriptItem } from '@neuron/client'

import { mono, useTheme, type Palette } from '../theme'
import { DiffView } from './DiffView'
import { Markdown } from './Markdown'
import { useStyles } from './ui'

const TOOL_STATE: Record<string, string> = {
  running: 'running',
  awaiting_permission: 'needs you',
  done: 'done',
  error: 'failed',
  denied: 'denied',
  cut_off: 'cut off',
}

const makeStyles = (colors: Palette) => ({
  message: { borderLeftWidth: 3, paddingLeft: 10, marginVertical: 6 },
  text: { color: colors.text, fontSize: 15, lineHeight: 22 },
  dim: { color: colors.textDim, fontSize: 13 },
  reasoning: { marginVertical: 4 },
  reasoningHead: { color: colors.textDim, fontSize: 13, fontStyle: 'italic' as const },
  reasoningText: { color: colors.textMuted, fontSize: 13, lineHeight: 19, fontStyle: 'italic' as const, marginTop: 4, paddingLeft: 10, borderLeftWidth: 2, borderLeftColor: colors.border },
  tool: { backgroundColor: colors.panel, borderRadius: 8, borderWidth: 1, borderColor: colors.border, padding: 10, marginVertical: 4, gap: 6 },
  toolHead: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 8 },
  toolTitle: { flex: 1, color: colors.text },
  toolVerb: { fontWeight: '600' as const, color: colors.text },
  toolTarget: { fontFamily: mono, color: colors.textMuted, fontSize: 13 },
  toolState: { color: colors.textDim, fontSize: 12 },
  output: { backgroundColor: colors.sunken, borderRadius: 6, maxHeight: 260 },
  outputText: { fontFamily: mono, fontSize: 12, lineHeight: 18, color: colors.textMuted, padding: 8 },
  question: { borderWidth: 1, borderColor: colors.warning, borderRadius: 8, padding: 10, marginVertical: 6, gap: 6 },
  questionLabel: { color: colors.warning, fontSize: 12, fontWeight: '600' as const },
  options: { flexDirection: 'row' as const, flexWrap: 'wrap' as const, gap: 8 },
  option: { borderWidth: 1, borderColor: colors.border, borderRadius: 8, paddingHorizontal: 12, paddingVertical: 8 },
  notice: { color: colors.textDim, fontFamily: mono, fontSize: 12, marginVertical: 4 },
  error: { color: colors.danger, marginVertical: 4 },
  todo: { color: colors.text, fontSize: 13 },
})

function Reasoning({ item }: { item: Extract<TranscriptItem, { kind: 'reasoning' }> }) {
  const styles = useStyles(makeStyles)
  const [open, setOpen] = useState(false)
  const label = item.streaming ? 'Thinking…' : item.redacted ? 'Reasoned (hidden by the model)' : 'Reasoned'
  const canOpen = !item.redacted && item.text.length > 0
  return (
    <Pressable style={styles.reasoning} accessibilityRole="button" accessibilityState={{ expanded: open }} disabled={!canOpen} onPress={() => setOpen(!open)}>
      <Text style={styles.reasoningHead}>
        {canOpen ? (open ? '▾ ' : '▸ ') : ''}
        {label}
      </Text>
      {open ? <Text style={styles.reasoningText}>{item.text}</Text> : null}
    </Pressable>
  )
}

function Tool({ item }: { item: Extract<TranscriptItem, { kind: 'tool' }> }) {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  const diff = item.view.diff
  // The view's output is the readable part (a file's content, stdout); the raw result is the fallback.
  const output = item.subagent?.result ?? item.view.output ?? item.output ?? null
  // Edits open on their diff; other calls on their output once it is short.
  const [open, setOpen] = useState(item.state === 'awaiting_permission')
  const subagent = item.subagent
  const stateColor = item.state === 'awaiting_permission' ? colors.warning : item.state === 'error' || item.state === 'denied' ? colors.danger : colors.textDim
  const expandable = !!diff || !!output || !!item.view.todos?.length
  return (
    <View style={styles.tool}>
      <Pressable style={styles.toolHead} accessibilityRole="button" accessibilityState={{ expanded: open }} disabled={!expandable} onPress={() => setOpen(!open)}>
        <Text style={styles.toolTitle} numberOfLines={open ? 4 : 2}>
          <Text style={styles.toolVerb}>{subagent ? `Sub-agent${subagent.type ? ` (${subagent.type})` : ''} ` : `${item.view.verb} `}</Text>
          <Text style={styles.toolTarget}>{subagent?.description ?? item.view.target}</Text>
        </Text>
        <Text style={[styles.toolState, { color: stateColor }]}>
          {subagent?.running ? 'working' : TOOL_STATE[item.state]}
          {diff ? `  +${diff.added} −${diff.removed}` : ''}
          {expandable ? (open ? '  ▾' : '  ▸') : ''}
        </Text>
      </Pressable>
      {open && diff ? <DiffView diff={diff} /> : null}
      {open && item.view.todos?.length
        ? item.view.todos.map((todo, index) => (
            <Text key={index} style={styles.todo}>
              {todo.status === 'completed' ? '✓' : todo.status === 'in_progress' ? '▸' : '○'} {todo.text}
            </Text>
          ))
        : null}
      {open && output && !diff ? (
        <ScrollView style={styles.output} nestedScrollEnabled>
          <Text style={styles.outputText} selectable>
            {output}
          </Text>
        </ScrollView>
      ) : null}
    </View>
  )
}

/** One transcript item: messages in Markdown, reasoning that folds, tool calls that open on their diff or output. */
export const TranscriptRow = memo(function TranscriptRow({ item, onAnswer }: { item: TranscriptItem; onAnswer: (id: number, answer: string) => void }) {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  switch (item.kind) {
    case 'user':
      return (
        <View style={[styles.message, { borderLeftColor: colors.user }]}>
          <Text style={styles.text} selectable>
            {item.text}
          </Text>
        </View>
      )
    case 'assistant':
      return (
        <View style={[styles.message, { borderLeftColor: colors.assistant }]}>
          <Markdown text={item.text} />
        </View>
      )
    case 'reasoning':
      return <Reasoning item={item} />
    case 'tool':
      return <Tool item={item} />
    case 'question':
      return (
        <View style={styles.question}>
          <Text style={styles.questionLabel}>{item.answered ? 'Neuron asked' : 'Neuron asks'}</Text>
          <Text style={styles.text}>{item.text}</Text>
          {!item.answered && item.id !== undefined && (item.options?.length ?? 0) > 0 ? (
            <View style={styles.options}>
              {item.options!.map((option) => (
                <Pressable key={option} accessibilityRole="button" style={styles.option} onPress={() => onAnswer(item.id!, option)}>
                  <Text style={styles.text}>{option}</Text>
                </Pressable>
              ))}
            </View>
          ) : null}
        </View>
      )
    case 'notice':
      return <Text style={styles.notice}>{item.text}</Text>
    case 'error':
      return <Text style={styles.error}>{item.text}</Text>
  }
})
