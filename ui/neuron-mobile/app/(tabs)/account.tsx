import { router, useFocusEffect } from 'expo-router'
import { useCallback } from 'react'
import { Pressable, Switch, Text, View } from 'react-native'

import { confirm } from '../../src/components/confirm'
import { Body, Button, Card, Label, Screen, useStyles } from '../../src/components/ui'
import type { PushState } from '../../src/store/app'
import { useApp, useStore } from '../../src/store/context'
import { ago } from '../../src/store/sessions'
import { useTheme, type Palette } from '../../src/theme'

const makeStyles = (colors: Palette) => ({
  email: { color: colors.text, fontSize: 17, fontWeight: '600' as const },
  plan: { color: colors.textMuted, fontSize: 14 },
  meterRow: { gap: 4 },
  meterLabel: { flexDirection: 'row' as const, justifyContent: 'space-between' as const },
  meterText: { color: colors.text, fontSize: 14 },
  meterDim: { color: colors.textDim, fontSize: 13 },
  track: { height: 8, borderRadius: 4, backgroundColor: colors.sunken, overflow: 'hidden' as const },
  fill: { height: 8, borderRadius: 4, backgroundColor: colors.accent },
  switchRow: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 12 },
  switchText: { flex: 1, gap: 2 },
  switchTitle: { color: colors.text, fontSize: 15 },
  switchHelp: { color: colors.textDim, fontSize: 13 },
  item: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 8, paddingVertical: 4 },
  itemText: { flex: 1, color: colors.text, fontSize: 14 },
  itemMeta: { color: colors.textDim, fontSize: 12 },
  link: { color: colors.link, fontSize: 14, fontWeight: '600' as const },
  error: { color: colors.danger, fontSize: 13 },
})

const compact = (value: number) => (value >= 1000 ? `${Math.round(value / 100) / 10}K` : String(value))

function Meter({ label, used, limit }: { label: string; used: number; limit: number | undefined }) {
  const styles = useStyles(makeStyles)
  const share = limit ? Math.min(1, used / limit) : 0
  return (
    <View style={styles.meterRow} accessibilityLabel={`${label}: ${used}${limit ? ` of ${limit}` : ''}`}>
      <View style={styles.meterLabel}>
        <Text style={styles.meterText}>{label}</Text>
        <Text style={styles.meterDim}>
          {compact(used)}
          {limit ? ` of ${compact(limit)}` : ''}
        </Text>
      </View>
      {limit ? (
        <View style={styles.track}>
          <View style={[styles.fill, { width: `${Math.round(share * 100)}%` }]} />
        </View>
      ) : null}
    </View>
  )
}

const PUSH_TEXT = (push: PushState): string | null => {
  switch (push.status) {
    case 'registered':
      return null
    case 'off':
      return 'Notifications are off.'
    case 'denied':
      return 'Notifications are blocked for Neuron in the phone settings.'
    case 'unsupported':
    case 'error':
      return push.reason
  }
}

/** Who is signed in, the plan, today's usage, linked computers and devices, notifications, sign-out. No purchases here. */
export default function AccountScreen() {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  const app = useApp()
  const store = useStore()

  useFocusEffect(
    useCallback(() => {
      void store.refreshAccount()
    }, [store]),
  )

  if (!app.signedIn) {
    return (
      <Screen>
        <Card title="Sign in to Zero-X">
          <Body>Sign in with the account your computer uses. You approve the sign-in in your browser; this app never sees your password.</Body>
          <Button label="Sign in" onPress={() => router.push('/sign-in')} />
        </Card>
        <Button kind="secondary" label="Settings" onPress={() => router.push('/settings')} />
      </Screen>
    )
  }

  const account = app.account
  const resets = account?.usage_today.resets_at ? new Date(account.usage_today.resets_at) : null
  const pushText = PUSH_TEXT(app.push)

  const signOut = async () => {
    if (await confirm('Sign out?', 'This phone forgets its paired computers and stops getting notifications. You can pair again after signing in.', 'Sign out')) {
      await store.signOut()
    }
  }

  return (
    <Screen>
      <Card>
        <Label>Signed in</Label>
        <Text style={styles.email}>{account?.user.email ?? '…'}</Text>
        <Text style={styles.plan}>{account ? `${account.plan.name} plan` : ''}</Text>
        {app.accountError ? <Text style={styles.error}>{app.accountError}</Text> : null}
      </Card>

      {account ? (
        <Card title="Today's usage">
          <Meter label="Requests" used={account.usage_today.requests} limit={account.limits.daily_requests} />
          <Meter label="Tokens" used={account.usage_today.tokens} limit={account.limits.daily_tokens} />
          {resets ? <Text style={styles.meterDim}>Resets at {resets.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</Text> : null}
        </Card>
      ) : null}

      <Card title="Linked computers">
        {app.computers.length === 0 ? <Body>None yet.</Body> : null}
        {app.computers.map((computer) => {
          const online = app.hosts.find((host) => host.hostId === computer.hostId)?.online
          return (
            <View key={computer.hostId} style={styles.item}>
              <Text style={{ color: online ? colors.success : colors.textFaint }}>●</Text>
              <Text style={styles.itemText} numberOfLines={1}>
                {computer.name}
              </Text>
              <Text style={styles.itemMeta}>paired {ago(computer.pairedAt)}</Text>
            </View>
          )
        })}
        <Pressable accessibilityRole="button" onPress={() => router.push('/computers')}>
          <Text style={styles.link}>Manage computers</Text>
        </Pressable>
      </Card>

      {app.linkedSessions ? (
        <Card title="Signed-in devices">
          {app.linkedSessions.map((session) => (
            <View key={session.id} style={styles.item}>
              <Text style={styles.itemText} numberOfLines={1}>
                {session.device_name ?? session.client ?? 'Device'}
                {session.current ? ' (this phone)' : ''}
              </Text>
              {!session.current ? (
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={`Sign out ${session.device_name ?? 'device'}`}
                  onPress={async () => {
                    if (await confirm('Sign out this device?', `${session.device_name ?? 'It'} will have to sign in again.`, 'Sign out')) await store.revokeLinkedSession(session.id)
                  }}
                >
                  <Text style={[styles.link, { color: colors.danger }]}>Sign out</Text>
                </Pressable>
              ) : null}
            </View>
          ))}
        </Card>
      ) : null}

      <Card title="Notifications">
        <View style={styles.switchRow}>
          <View style={styles.switchText}>
            <Text style={styles.switchTitle}>Needs me</Text>
            <Text style={styles.switchHelp}>An approval or a question has waited 30 seconds and the phone is not open on it.</Text>
          </View>
          <Switch accessibilityLabel="Needs me notifications" value={app.settings.notifyNeedsMe} onValueChange={(value) => void store.updateSettings({ notifyNeedsMe: value })} />
        </View>
        <View style={styles.switchRow}>
          <View style={styles.switchText}>
            <Text style={styles.switchTitle}>Finished</Text>
            <Text style={styles.switchHelp}>A session finished its turn.</Text>
          </View>
          <Switch accessibilityLabel="Finished notifications" value={app.settings.notifyFinished} onValueChange={(value) => void store.updateSettings({ notifyFinished: value })} />
        </View>
        {pushText ? <Text style={styles.switchHelp}>{pushText}</Text> : null}
      </Card>

      <Button kind="secondary" label="Settings" onPress={() => router.push('/settings')} />
      <Button kind="danger" label="Sign out" onPress={() => void signOut()} />
    </Screen>
  )
}
