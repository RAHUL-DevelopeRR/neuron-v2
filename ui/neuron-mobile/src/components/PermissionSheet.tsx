import { useMemo, useState } from 'react'
import { Pressable, ScrollView, Text, TextInput, View } from 'react-native'

import { toolView, type Diff, type PendingPermission, type PermissionDecision } from '@neuron/client'

import { mono, useTheme, type Palette } from '../theme'
import { DiffView } from './DiffView'
import { useStyles } from './ui'

const makeStyles = (colors: Palette) => ({
  sheet: { backgroundColor: colors.panel, borderTopWidth: 2, borderTopColor: colors.warning, padding: 14, gap: 8 },
  title: { color: colors.text, fontSize: 17, fontWeight: '600' as const },
  dim: { color: colors.textDim, fontSize: 13 },
  detail: { maxHeight: 220 },
  code: { fontFamily: mono, color: colors.link, fontSize: 13, backgroundColor: colors.sunken, borderRadius: 6, padding: 8 },
  buttons: { flexDirection: 'row' as const, flexWrap: 'wrap' as const, gap: 8 },
  button: { borderWidth: 1, borderColor: colors.border, borderRadius: 8, paddingHorizontal: 12, paddingVertical: 10 },
  allow: { backgroundColor: colors.accent, borderColor: colors.accent },
  deny: { borderColor: colors.danger },
  allowText: { color: '#ffffff', fontWeight: '600' as const },
  text: { color: colors.text },
  denyText: { color: colors.danger, fontWeight: '600' as const },
  note: { backgroundColor: colors.sunken, color: colors.text, borderRadius: 8, borderWidth: 1, borderColor: colors.border, padding: 10, minHeight: 64, fontSize: 15, textAlignVertical: 'top' as const },
})

/** What the call will do, in a line: the command, or the path. */
function summary(input: string): string {
  try {
    const value = JSON.parse(input) as Record<string, unknown>
    if (typeof value.command === 'string') return `$ ${value.command}`
    if (typeof value.path === 'string') return value.path
    if (typeof value.file_path === 'string') return value.file_path
    return JSON.stringify(value, null, 2)
  } catch {
    return input
  }
}

/**
 * An approval the agent waits for: what it wants to run or change (with the
 * diff for edits), then Allow once, Allow for the session, Deny, or Deny with
 * a note the agent reads.
 */
export function PermissionSheet({
  pending,
  diff,
  cwd,
  onAnswer,
}: {
  pending: PendingPermission
  /** The call's diff, when the transcript has the call. */
  diff?: Diff
  cwd?: string | null
  onAnswer: (decision: PermissionDecision) => void
}) {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  const { request } = pending
  const [noting, setNoting] = useState(false)
  const [note, setNote] = useState('')
  // An edit's diff from its input, when the transcript does not have the call yet.
  const preview = useMemo(() => diff ?? toolView(request.tool, request.input, cwd).diff, [diff, request.tool, request.input, cwd])
  const view = useMemo(() => toolView(request.tool, request.input, cwd), [request.tool, request.input, cwd])
  return (
    <View style={styles.sheet} accessibilityRole="alert" accessibilityLabel={`Neuron asks to ${view.verb.toLowerCase()} ${view.target}`}>
      <Text style={styles.title}>
        {view.verb} {view.kind === 'edit' || view.kind === 'write' ? view.target : request.tool}?
      </Text>
      {request.reason ? <Text style={styles.dim}>{request.reason}</Text> : null}
      <ScrollView style={styles.detail} nestedScrollEnabled>
        {preview && preview.lines.length > 0 ? <DiffView diff={preview} collapsedLines={12} /> : <Text style={styles.code}>{summary(request.input)}</Text>}
      </ScrollView>
      {noting ? (
        <>
          <TextInput
            accessibilityLabel="A note for Neuron"
            style={styles.note}
            value={note}
            onChangeText={setNote}
            placeholder="Tell Neuron what to do instead…"
            placeholderTextColor={colors.textFaint}
            multiline
            autoFocus
          />
          <View style={styles.buttons}>
            <Pressable accessibilityRole="button" style={[styles.button, styles.deny]} onPress={() => onAnswer({ kind: 'deny', note: note.trim() || null })}>
              <Text style={styles.denyText}>Deny with this note</Text>
            </Pressable>
            <Pressable accessibilityRole="button" style={styles.button} onPress={() => setNoting(false)}>
              <Text style={styles.text}>Back</Text>
            </Pressable>
          </View>
        </>
      ) : (
        <View style={styles.buttons}>
          <Pressable accessibilityRole="button" style={[styles.button, styles.allow]} onPress={() => onAnswer({ kind: 'allow_once' })}>
            <Text style={styles.allowText}>Allow once</Text>
          </Pressable>
          {request.session_prefixes.length > 0 ? (
            <Pressable accessibilityRole="button" style={styles.button} onPress={() => onAnswer({ kind: 'allow_for_session' })}>
              <Text style={styles.text}>Allow {request.session_prefixes.join(', ')} this session</Text>
            </Pressable>
          ) : null}
          <Pressable accessibilityRole="button" style={[styles.button, styles.deny]} onPress={() => onAnswer({ kind: 'deny', note: null })}>
            <Text style={styles.denyText}>Deny</Text>
          </Pressable>
          <Pressable accessibilityRole="button" style={[styles.button, styles.deny]} onPress={() => setNoting(true)}>
            <Text style={styles.denyText}>Deny with a note…</Text>
          </Pressable>
        </View>
      )}
    </View>
  )
}
