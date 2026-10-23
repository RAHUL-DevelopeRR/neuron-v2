import { useEffect, useRef, useState } from 'react'
import { FlatList, KeyboardAvoidingView, Platform, Pressable, ScrollView, Text, TextInput, View } from 'react-native'

import {
  FEATURE_SESSIONS,
  formatTokens,
  hasFeature,
  needsSignIn,
  openQuestionItem,
  type SessionController,
  type SessionSummary,
  type TodoItem,
  type TranscriptItem,
} from '@neuron/client'

import { PermissionSheet } from '../components/PermissionSheet'
import { TranscriptRow } from '../components/Transcript'
import { Banner, useStyles } from '../components/ui'
import { useTheme, type Palette } from '../theme'
import { useSession } from '../useSession'

const makeStyles = (colors: Palette) => ({
  screen: { flex: 1, backgroundColor: colors.bg },
  status: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 8, paddingHorizontal: 12, paddingVertical: 8, backgroundColor: colors.panel, borderBottomWidth: 1, borderBottomColor: colors.border },
  dim: { color: colors.textDim, fontSize: 13 },
  chip: { backgroundColor: colors.chip, color: colors.chipText, fontWeight: '600' as const, fontSize: 12, paddingHorizontal: 8, paddingVertical: 3, borderRadius: 6, maxWidth: 140, overflow: 'hidden' as const },
  link: { color: colors.link, paddingHorizontal: 6, paddingVertical: 4 },
  transcript: { flex: 1 },
  empty: { color: colors.textDim, textAlign: 'center' as const, marginTop: 40, paddingHorizontal: 24 },
  working: { color: colors.accent, marginTop: 8 },
  sheet: { backgroundColor: colors.panel, borderBottomWidth: 1, borderBottomColor: colors.border, padding: 12, gap: 6 },
  sheetButton: { borderWidth: 1, borderColor: colors.accent, backgroundColor: colors.accent, borderRadius: 8, paddingHorizontal: 12, paddingVertical: 10, alignSelf: 'flex-start' as const },
  sheetButtonText: { color: '#ffffff', fontWeight: '600' as const },
  chatRow: { paddingVertical: 8, paddingHorizontal: 6, borderRadius: 6 },
  chatRowCurrent: { backgroundColor: colors.sunken },
  text: { color: colors.text, fontSize: 15 },
  tasks: { backgroundColor: colors.panel, borderTopWidth: 1, borderTopColor: colors.border, paddingHorizontal: 12, paddingVertical: 8 },
  tasksTitle: { color: colors.warning, fontWeight: '600' as const },
  todoDone: { color: colors.textDim, textDecorationLine: 'line-through' as const },
  composer: { borderTopWidth: 1, borderTopColor: colors.border, padding: 10, backgroundColor: colors.bg },
  questionHint: { color: colors.warning, fontSize: 12, marginBottom: 4 },
  composerRow: { flexDirection: 'row' as const, gap: 8, alignItems: 'flex-end' as const },
  input: {
    flex: 1,
    minHeight: 44,
    maxHeight: 140,
    backgroundColor: colors.sunken,
    color: colors.text,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: colors.border,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 15,
  },
  send: { backgroundColor: colors.accent, borderRadius: 10, paddingHorizontal: 16, paddingVertical: 12 },
  sendText: { color: '#ffffff', fontWeight: '600' as const },
  stop: { backgroundColor: 'transparent', borderWidth: 1, borderColor: colors.warning },
  stopText: { color: colors.warning, fontWeight: '600' as const },
  disabled: { opacity: 0.5 },
})

export interface SessionBanner {
  text: string
  tone?: 'warning' | 'error'
  action?: { label: string; onPress: () => void }
}

/**
 * A session running on a computer, from the phone: the transcript (Markdown,
 * reasoning, tool calls with their diffs and output), approvals, the agent's
 * questions and tasks, and the composer.
 */
export function SessionView({
  controller,
  hostName,
  clients,
  banner,
  onLeave,
  onSeen,
}: {
  controller: SessionController
  hostName: string | null
  clients: number
  /** The connection's state, when it is not simply attached. */
  banner?: SessionBanner | null
  onLeave?: () => void
  /** Called as the transcript updates: the session was seen up to now. */
  onSeen?: () => void
}) {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  const state = useSession(controller)
  const [text, setText] = useState('')
  const [chatsOpen, setChatsOpen] = useState(false)
  const list = useRef<FlatList<TranscriptItem>>(null)
  const status = state.status
  const permission = state.permissions[0] ?? null
  const question = openQuestionItem(state)
  const ready = state.connection === 'ready'
  const history = hasFeature(state, FEATURE_SESSIONS)

  // The computer's chats, kept fresh after each turn.
  useEffect(() => {
    if (ready && history && !state.busy) controller.listSessions()
  }, [controller, ready, history, state.busy, status?.session_id])

  // Every action counts as seen while the session is open (the last ones often change no item).
  useEffect(() => {
    onSeen?.()
  }, [onSeen, state])

  const send = () => {
    if (!text.trim() || !ready) return
    controller.send(text)
    setText('')
  }

  const permissionTool = permission ? state.items.find((item) => item.kind === 'tool' && item.key === permission.toolKey) : undefined

  return (
    <KeyboardAvoidingView style={styles.screen} behavior={Platform.OS === 'ios' ? 'padding' : undefined} keyboardVerticalOffset={Platform.OS === 'ios' ? 90 : 0}>
      <View style={styles.status}>
        <Text style={[styles.dim, { flex: 1 }]} numberOfLines={1}>
          <Text style={{ color: ready ? colors.success : colors.danger }}>● </Text>
          {ready ? `on ${hostName ?? 'your computer'}` : state.connection === 'exited' ? 'disconnected' : 'connecting'}
          {status?.git?.branch ? ` · ⎇ ${status.git.branch}` : ''}
          {clients > 1 ? ` · ${clients} watching` : ''}
          {state.usage ? ` · ↑${formatTokens(state.usage.cumulative.input)} ↓${formatTokens(state.usage.cumulative.output)}` : ''}
        </Text>
        {status?.model ? (
          <Text style={styles.chip} numberOfLines={1}>
            {status.model}
          </Text>
        ) : null}
        {history ? (
          <Pressable accessibilityRole="button" accessibilityLabel="Chats on the computer" onPress={() => setChatsOpen(!chatsOpen)}>
            <Text style={styles.link}>Chats</Text>
          </Pressable>
        ) : null}
        {onLeave ? (
          <Pressable accessibilityRole="button" accessibilityLabel="Leave" onPress={onLeave}>
            <Text style={styles.link}>Leave</Text>
          </Pressable>
        ) : null}
      </View>

      {banner ? <Banner text={banner.text} tone={banner.tone} action={banner.action} /> : null}
      {needsSignIn(state) ? <Banner text="Neuron on the computer is signed out. Sign in there, or send /login and approve the code." /> : null}
      {state.connection === 'exited' && !banner ? <Banner tone="error" text={state.connectionError ?? 'Disconnected from the computer.'} /> : null}

      {chatsOpen ? (
        <ChatsSheet
          sessions={state.sessions ?? []}
          current={status?.session_id ?? null}
          busy={state.busy}
          onNew={() => {
            setChatsOpen(false)
            controller.newChat()
          }}
          onPick={(id) => {
            setChatsOpen(false)
            controller.resume(id)
          }}
        />
      ) : null}

      <FlatList
        ref={list}
        style={styles.transcript}
        contentContainerStyle={{ padding: 12, paddingBottom: 24 }}
        data={state.items}
        keyExtractor={(item) => item.key}
        renderItem={({ item }) => <TranscriptRow item={item} onAnswer={(id, answer) => controller.answer(id, answer)} />}
        onContentSizeChange={() => list.current?.scrollToEnd({ animated: false })}
        ListEmptyComponent={<Text style={styles.empty}>Nothing in this session yet. Send a prompt, or wait for the computer to start one.</Text>}
        ListFooterComponent={state.busy ? <Text style={styles.working}>working…</Text> : undefined}
      />

      {permission ? (
        <PermissionSheet
          key={permission.id}
          pending={permission}
          diff={permissionTool?.kind === 'tool' ? permissionTool.view.diff : undefined}
          cwd={status?.cwd}
          onAnswer={(decision) => controller.answerPermission(permission.id, decision)}
        />
      ) : null}

      {state.todos.length > 0 ? <TasksStrip todos={state.todos} /> : null}

      <View style={styles.composer}>
        {question ? <Text style={styles.questionHint}>Answering Neuron's question</Text> : null}
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

function ChatsSheet({
  sessions,
  current,
  busy,
  onNew,
  onPick,
}: {
  sessions: SessionSummary[]
  current: string | null
  busy: boolean
  onNew: () => void
  onPick: (id: string) => void
}) {
  const styles = useStyles(makeStyles)
  const shown = sessions.filter((session) => session.message_count > 0 || session.id === current)
  return (
    <View style={styles.sheet} accessibilityLabel="Chats on the computer">
      <Pressable accessibilityRole="button" style={styles.sheetButton} onPress={onNew} disabled={busy}>
        <Text style={styles.sheetButtonText}>+ New chat</Text>
      </Pressable>
      <ScrollView style={{ maxHeight: 260 }}>
        {shown.map((session) => (
          <Pressable
            key={session.id}
            accessibilityRole="button"
            style={[styles.chatRow, session.id === current && styles.chatRowCurrent]}
            disabled={busy || session.id === current}
            onPress={() => onPick(session.id)}
          >
            <Text style={styles.text} numberOfLines={1}>
              {session.title ?? 'Untitled chat'}
            </Text>
            <Text style={styles.dim}>{session.message_count} messages</Text>
          </Pressable>
        ))}
      </ScrollView>
    </View>
  )
}

function TasksStrip({ todos }: { todos: TodoItem[] }) {
  const styles = useStyles(makeStyles)
  const [open, setOpen] = useState(false)
  const done = todos.filter((todo) => todo.status === 'completed').length
  const now = todos.find((todo) => todo.status === 'in_progress')
  return (
    <Pressable accessibilityRole="button" accessibilityLabel="Tasks" style={styles.tasks} onPress={() => setOpen(!open)}>
      <Text style={styles.tasksTitle}>
        Tasks {done}/{todos.length}
        {now ? <Text style={styles.dim}> · {now.active_form ?? now.content}</Text> : null}
      </Text>
      {open
        ? todos.map((todo, index) => (
            <Text key={index} style={[styles.text, todo.status === 'completed' && styles.todoDone]}>
              {todo.status === 'completed' ? '✓' : todo.status === 'in_progress' ? '▸' : '○'} {todo.content}
            </Text>
          ))
        : null}
    </Pressable>
  )
}
