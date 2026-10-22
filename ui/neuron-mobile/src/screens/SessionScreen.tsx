import { useRef, useState } from 'react'
import {
  FlatList,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native'

import { formatTokens, needsSignIn, openQuestion, type PendingPermission, type TranscriptItem } from '@neuron/client'

import type { Connection } from '../connection'
import { colors, mono } from '../theme'
import { useSession } from '../useSession'

const basename = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path

const TOOL_STATE: Record<string, string> = {
  running: 'running',
  awaiting_permission: 'needs you',
  done: 'done',
  error: 'failed',
  denied: 'denied',
  cut_off: 'cut off',
}

/** Watch a session running on the computer, and steer it: prompts, approvals, interrupts. */
export function SessionScreen({ connection, onLeave }: { connection: Connection; onLeave: () => void }) {
  const { controller, transport } = connection
  const state = useSession(controller)
  const [text, setText] = useState('')
  const list = useRef<FlatList<TranscriptItem>>(null)
  const status = state.status
  const permission = state.permissions[0] ?? null
  const question = openQuestion(state)
  const ready = state.connection === 'ready'

  const send = () => {
    if (!text.trim() || !ready) return
    controller.send(text)
    setText('')
  }

  return (
    <KeyboardAvoidingView style={styles.screen} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
      <View style={styles.header}>
        <View style={{ flex: 1 }}>
          <Text style={styles.workspace} numberOfLines={1}>
            {status ? basename(status.cwd) : transport.host?.name ?? 'Neuron'}
            {status?.git?.branch ? <Text style={styles.dim}>  ⎇ {status.git.branch}</Text> : null}
          </Text>
          <Text style={styles.dim} numberOfLines={1}>
            <Text style={{ color: ready ? colors.success : colors.danger }}>● </Text>
            {ready ? `on ${transport.host?.name ?? 'your computer'}` : state.connection}
            {transport.clients > 1 ? ` · ${transport.clients} watching` : ''}
            {state.usage ? ` · ↑${formatTokens(state.usage.cumulative.input)} ↓${formatTokens(state.usage.cumulative.output)}` : ''}
          </Text>
        </View>
        {status?.model ? (
          <Text style={styles.chip} numberOfLines={1}>
            {status.model}
          </Text>
        ) : null}
        <Pressable accessibilityRole="button" accessibilityLabel="Leave" onPress={onLeave} style={styles.leave}>
          <Text style={styles.leaveText}>Leave</Text>
        </Pressable>
      </View>

      {needsSignIn(state) && (
        <Text style={styles.banner}>The computer's Neuron is signed out. Sign in there (or send /login and approve the code).</Text>
      )}
      {state.connection === 'exited' && (
        <Text style={[styles.banner, styles.bannerError]}>{state.connectionError ?? 'Disconnected from the Neuron host.'}</Text>
      )}

      <FlatList
        ref={list}
        style={styles.transcript}
        contentContainerStyle={{ padding: 12, paddingBottom: 24 }}
        data={state.items}
        keyExtractor={(item) => item.key}
        renderItem={({ item }) => <Row item={item} />}
        onContentSizeChange={() => list.current?.scrollToEnd({ animated: false })}
        ListEmptyComponent={<Text style={styles.empty}>Nothing in this session yet. Send a prompt, or wait for the computer to start one.</Text>}
        ListFooterComponent={state.busy ? <Text style={styles.working}>working…</Text> : undefined}
      />

      {permission && <PermissionSheet key={permission.id} pending={permission} onAnswer={(decision) => controller.answerPermission(permission.id, decision)} />}

      <View style={styles.composer}>
        {question && <Text style={styles.questionHint}>Answering Neuron's question</Text>}
        <View style={styles.composerRow}>
          <TextInput
            accessibilityLabel="Message Neuron"
            style={styles.input}
            value={text}
            onChangeText={setText}
            placeholder={ready ? (question ? 'Your answer…' : 'Ask Neuron to do something…') : 'Not connected'}
            placeholderTextColor={colors.textFaint}
            editable={ready}
            multiline
            onSubmitEditing={send}
            blurOnSubmit
          />
          {state.busy ? (
            <Pressable accessibilityRole="button" style={[styles.send, styles.stop]} onPress={() => controller.interrupt()}>
              <Text style={styles.stopText}>Stop</Text>
            </Pressable>
          ) : (
            <Pressable accessibilityRole="button" style={[styles.send, !text.trim() && styles.disabled]} onPress={send} disabled={!text.trim()}>
              <Text style={styles.sendText}>Send</Text>
            </Pressable>
          )}
        </View>
      </View>
    </KeyboardAvoidingView>
  )
}

function Row({ item }: { item: TranscriptItem }) {
  switch (item.kind) {
    case 'user':
      return (
        <View style={[styles.message, { borderLeftColor: colors.user }]}>
          <Text style={styles.text}>{item.text}</Text>
        </View>
      )
    case 'assistant':
      return (
        <View style={[styles.message, { borderLeftColor: colors.assistant }]}>
          <Text style={styles.text}>{item.text}</Text>
        </View>
      )
    case 'reasoning':
      return <Text style={styles.dim}>{item.streaming ? 'Thinking…' : 'Reasoned'}</Text>
    case 'tool': {
      const diff = item.view.diff
      return (
        <View style={styles.tool}>
          <Text style={styles.toolHead} numberOfLines={2}>
            <Text style={styles.toolVerb}>{item.view.verb} </Text>
            <Text style={styles.toolTarget}>{item.view.target}</Text>
          </Text>
          <Text style={[styles.toolState, item.state === 'awaiting_permission' && { color: colors.warning }]}>
            {TOOL_STATE[item.state]}
            {diff ? `  +${diff.added} −${diff.removed}` : ''}
          </Text>
        </View>
      )
    }
    case 'question':
      return (
        <View style={styles.question}>
          <Text style={styles.questionLabel}>Neuron asks</Text>
          <Text style={styles.text}>{item.text}</Text>
        </View>
      )
    case 'notice':
      return <Text style={styles.notice}>{item.text}</Text>
    case 'error':
      return <Text style={styles.errorLine}>{item.text}</Text>
  }
}

function PermissionSheet({
  pending,
  onAnswer,
}: {
  pending: PendingPermission
  onAnswer: (decision: { kind: 'allow_once' } | { kind: 'allow_for_session' } | { kind: 'deny'; note: string | null }) => void
}) {
  const { request } = pending
  let detail = request.input
  try {
    const input = JSON.parse(request.input) as Record<string, unknown>
    detail = typeof input.command === 'string' ? `$ ${input.command}` : typeof input.path === 'string' ? input.path : JSON.stringify(input, null, 2)
  } catch {
    // Shown as sent.
  }
  return (
    <View style={styles.sheet} accessibilityRole="alert">
      <Text style={styles.sheetTitle}>Allow {request.tool}?</Text>
      {request.reason ? <Text style={styles.dim}>{request.reason}</Text> : null}
      <ScrollView style={styles.sheetDetail}>
        <Text style={styles.code}>{detail}</Text>
      </ScrollView>
      <View style={styles.sheetButtons}>
        <Pressable accessibilityRole="button" style={[styles.sheetButton, styles.allow]} onPress={() => onAnswer({ kind: 'allow_once' })}>
          <Text style={styles.sendText}>Allow once</Text>
        </Pressable>
        {request.session_prefixes.length > 0 && (
          <Pressable accessibilityRole="button" style={styles.sheetButton} onPress={() => onAnswer({ kind: 'allow_for_session' })}>
            <Text style={styles.text}>Allow {request.session_prefixes.join(', ')} this session</Text>
          </Pressable>
        )}
        <Pressable accessibilityRole="button" style={[styles.sheetButton, styles.deny]} onPress={() => onAnswer({ kind: 'deny', note: null })}>
          <Text style={{ color: colors.diffRemoveText }}>Deny</Text>
        </Pressable>
      </View>
    </View>
  )
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 12,
    paddingTop: 44,
    paddingBottom: 10,
    backgroundColor: colors.panel,
    borderBottomWidth: 1,
    borderBottomColor: colors.border,
  },
  workspace: { color: colors.text, fontSize: 16, fontWeight: '600' },
  dim: { color: colors.textDim, fontSize: 13 },
  chip: { backgroundColor: colors.chip, color: colors.chipText, fontWeight: '600', fontSize: 12, paddingHorizontal: 8, paddingVertical: 3, borderRadius: 6, maxWidth: 130 },
  leave: { paddingHorizontal: 8, paddingVertical: 4 },
  leaveText: { color: colors.link },
  banner: { backgroundColor: colors.sunken, color: colors.warning, padding: 10, fontSize: 13 },
  bannerError: { color: colors.diffRemoveText },
  transcript: { flex: 1 },
  empty: { color: colors.textDim, textAlign: 'center', marginTop: 40, paddingHorizontal: 24 },
  working: { color: colors.accent, marginTop: 8 },
  message: { borderLeftWidth: 3, paddingLeft: 10, marginVertical: 6 },
  text: { color: colors.text, fontSize: 15, lineHeight: 21 },
  tool: { backgroundColor: colors.panel, borderRadius: 8, borderWidth: 1, borderColor: colors.border, padding: 8, marginVertical: 4 },
  toolHead: { color: colors.text },
  toolVerb: { fontWeight: '600', color: colors.text },
  toolTarget: { fontFamily: mono, color: colors.textMuted, fontSize: 13 },
  toolState: { color: colors.textDim, fontSize: 12, marginTop: 2 },
  question: { borderWidth: 1, borderColor: colors.warning, borderRadius: 8, padding: 8, marginVertical: 6 },
  questionLabel: { color: colors.warning, fontSize: 12, fontWeight: '600' },
  notice: { color: colors.textDim, fontFamily: mono, fontSize: 12, marginVertical: 4 },
  errorLine: { color: colors.diffRemoveText, marginVertical: 4 },
  sheet: { backgroundColor: colors.panel, borderTopWidth: 2, borderTopColor: colors.warning, padding: 14, gap: 6 },
  sheetTitle: { color: colors.text, fontSize: 17, fontWeight: '600' },
  sheetDetail: { maxHeight: 140, backgroundColor: colors.sunken, borderRadius: 6, padding: 8 },
  code: { fontFamily: mono, color: colors.link, fontSize: 13 },
  sheetButtons: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginTop: 6 },
  sheetButton: { borderWidth: 1, borderColor: colors.border, borderRadius: 8, paddingHorizontal: 12, paddingVertical: 10 },
  allow: { backgroundColor: colors.accent, borderColor: colors.accent },
  deny: { borderColor: colors.danger },
  composer: { borderTopWidth: 1, borderTopColor: colors.accent, padding: 10, backgroundColor: colors.bg },
  questionHint: { color: colors.warning, fontSize: 12, marginBottom: 4 },
  composerRow: { flexDirection: 'row', gap: 8, alignItems: 'flex-end' },
  input: {
    flex: 1,
    minHeight: 44,
    maxHeight: 140,
    backgroundColor: colors.sunken,
    color: colors.text,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: colors.border,
    paddingHorizontal: 10,
    paddingVertical: 10,
    fontSize: 15,
  },
  send: { backgroundColor: colors.accent, borderRadius: 8, paddingHorizontal: 14, paddingVertical: 12 },
  sendText: { color: '#fff', fontWeight: '600' },
  stop: { backgroundColor: 'transparent', borderWidth: 1, borderColor: colors.warning },
  stopText: { color: colors.warning, fontWeight: '600' },
  disabled: { opacity: 0.5 },
})
