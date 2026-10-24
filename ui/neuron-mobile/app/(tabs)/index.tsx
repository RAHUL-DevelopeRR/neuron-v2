import { router } from 'expo-router'
import { useMemo } from 'react'
import { Pressable, RefreshControl, SectionList, Text, View } from 'react-native'

import { PixelLogo } from '../../src/components/PixelLogo'
import { RelayBanner } from '../../src/components/RelayBanner'
import { Badge, Body, Button, Card, Code, useStyles } from '../../src/components/ui'
import { useApp, useStore } from '../../src/store/context'
import { ago, groupSessions, type SessionRow } from '../../src/store/sessions'
import type { Palette } from '../../src/theme'

const makeStyles = (colors: Palette) => ({
  screen: { flex: 1, backgroundColor: colors.bg },
  list: { padding: 12, paddingBottom: 32 },
  section: { color: colors.textDim, fontSize: 12, fontWeight: '600' as const, textTransform: 'uppercase' as const, letterSpacing: 0.4, marginTop: 14, marginBottom: 6 },
  row: { backgroundColor: colors.panel, borderRadius: 10, borderWidth: 1, borderColor: colors.border, padding: 12, marginBottom: 8, gap: 4 },
  rowHead: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 8 },
  title: { flex: 1, color: colors.text, fontSize: 15, fontWeight: '500' as const },
  meta: { color: colors.textDim, fontSize: 12 },
  footer: { color: colors.textDim, fontSize: 13, marginTop: 16, textAlign: 'center' as const },
  onboarding: { padding: 16, gap: 12 },
  logo: { alignItems: 'center' as const, marginVertical: 12 },
})

/** Every shared session on every paired computer, grouped by project. */
export default function SessionsScreen() {
  const styles = useStyles(makeStyles)
  const app = useApp()
  const store = useStore()
  const groups = useMemo(() => groupSessions(app.hosts, app.viewed), [app.hosts, app.viewed])
  const offline = app.hosts.filter((host) => !host.online)

  if (!app.loaded) return <View style={styles.screen} />

  if (!app.signedIn || app.computers.length === 0) {
    return (
      <View style={[styles.screen, styles.onboarding]}>
        <View style={styles.logo}>
          <PixelLogo pixel={6} />
        </View>
        <Card title="Continue your computer's sessions here">
          <Body>Neuron keeps running on your computer. This phone follows along from anywhere, answers approvals and sends prompts, end-to-end encrypted.</Body>
          <Body>
            On the computer, turn on <Code>Continue on phone</Code> in a session (desktop app), or type <Code>/phone</Code> (terminal). Then pair this phone with the QR code it
            shows.
          </Body>
          <Button label="Pair a computer" onPress={() => router.push('/pair')} />
        </Card>
      </View>
    )
  }

  const open = (row: SessionRow) => router.push({ pathname: '/session/[host]/[share]', params: { host: row.hostId, share: row.shareId } })

  return (
    <View style={styles.screen}>
      <RelayBanner relay={app.relay} />
      <SectionList
        contentContainerStyle={styles.list}
        sections={groups.map((group) => ({ title: group.project, data: group.rows }))}
        keyExtractor={(row) => row.key}
        refreshControl={<RefreshControl refreshing={false} onRefresh={() => store.refresh()} />}
        renderSectionHeader={({ section }) => <Text style={styles.section}>{section.title}</Text>}
        renderItem={({ item }) => (
          <Pressable accessibilityRole="button" accessibilityLabel={`${item.title} on ${item.computer}`} style={styles.row} onPress={() => open(item)}>
            <View style={styles.rowHead}>
              <Text style={styles.title} numberOfLines={1}>
                {item.title}
              </Text>
              <Badge kind={item.badge} />
            </View>
            <Text style={styles.meta} numberOfLines={1}>
              {item.computer}
              {item.branch ? ` · ⎇ ${item.branch}` : ''} · {ago(item.updatedAt)}
            </Text>
          </Pressable>
        )}
        ListEmptyComponent={
          <Card title="Nothing shared right now">
            <Body>
              Your computers are paired, but none is sharing a session. Turn on <Code>Continue on phone</Code> on the computer, or type <Code>/phone</Code> in the terminal.
            </Body>
          </Card>
        }
        ListFooterComponent={offline.length ? <Text style={styles.footer}>Offline: {offline.map((host) => host.name).join(', ')}</Text> : undefined}
      />
    </View>
  )
}
