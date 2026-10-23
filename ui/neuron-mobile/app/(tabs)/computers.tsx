import { router } from 'expo-router'
import { Text, View } from 'react-native'

import { confirm } from '../../src/components/confirm'
import { RelayBanner } from '../../src/components/RelayBanner'
import { Body, Button, Card, Screen, useStyles } from '../../src/components/ui'
import { useApp, useStore } from '../../src/store/context'
import { ago } from '../../src/store/sessions'
import { useTheme, type Palette } from '../../src/theme'

const makeStyles = (colors: Palette) => ({
  head: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 8 },
  name: { flex: 1, color: colors.text, fontSize: 16, fontWeight: '600' as const },
  meta: { color: colors.textDim, fontSize: 13 },
})

/** The computers this phone is paired with: online or not, what they share, and Forget. */
export default function ComputersScreen() {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  const app = useApp()
  const store = useStore()

  const forget = async (hostId: string, name: string) => {
    if (await confirm(`Forget ${name}?`, 'This phone stops seeing its sessions, and the computer forgets this phone. Pair again to undo.', 'Forget')) {
      await store.forget(hostId)
    }
  }

  return (
    <View style={{ flex: 1 }}>
      <RelayBanner relay={app.relay} />
      <Screen>
        {app.computers.map((computer) => {
          const host = app.hosts.find((candidate) => candidate.hostId === computer.hostId)
          const shares = host?.shares.length ?? 0
          return (
            <Card key={computer.hostId}>
              <View style={styles.head}>
                <Text style={{ color: host?.online ? colors.success : colors.textFaint }}>●</Text>
                <Text style={styles.name} numberOfLines={1}>
                  {host?.name ?? computer.name}
                </Text>
              </View>
              <Text style={styles.meta}>
                {host?.online ? `Online · ${shares} shared session${shares === 1 ? '' : 's'}` : 'Offline'} · paired {ago(computer.pairedAt)}
              </Text>
              <Button kind="danger" label="Forget" accessibilityLabel={`Forget ${computer.name}`} onPress={() => void forget(computer.hostId, computer.name)} />
            </Card>
          )
        })}
        {app.computers.length === 0 ? (
          <Card title="No computers yet">
            <Body>Pair this phone with a computer that runs Neuron to follow its sessions from anywhere.</Body>
          </Card>
        ) : null}
        <Button label="Pair a computer" onPress={() => router.push('/pair')} />
      </Screen>
    </View>
  )
}
