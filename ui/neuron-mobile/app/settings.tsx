import Constants from 'expo-constants'
import { router } from 'expo-router'
import { Pressable, Text, View } from 'react-native'

import { Body, Button, Card, Screen, useStyles } from '../src/components/ui'
import type { ThemeSetting } from '../src/store/app'
import { useApp, useStore } from '../src/store/context'
import type { Palette } from '../src/theme'

const makeStyles = (colors: Palette) => ({
  segments: { flexDirection: 'row' as const, borderWidth: 1, borderColor: colors.border, borderRadius: 10, overflow: 'hidden' as const },
  segment: { flex: 1, paddingVertical: 10, alignItems: 'center' as const },
  selected: { backgroundColor: colors.accent },
  segmentText: { color: colors.text, fontSize: 14 },
  selectedText: { color: '#ffffff', fontWeight: '600' as const },
})

const THEMES: Array<{ value: ThemeSetting; label: string }> = [
  { value: 'system', label: 'System' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
]

export default function SettingsScreen() {
  const styles = useStyles(makeStyles)
  const app = useApp()
  const store = useStore()
  return (
    <Screen>
      <Card title="Appearance">
        <View style={styles.segments} accessibilityRole="radiogroup">
          {THEMES.map((theme) => {
            const selected = app.settings.theme === theme.value
            return (
              <Pressable
                key={theme.value}
                accessibilityRole="radio"
                accessibilityState={{ selected }}
                style={[styles.segment, selected && styles.selected]}
                onPress={() => void store.updateSettings({ theme: theme.value })}
              >
                <Text style={[styles.segmentText, selected && styles.selectedText]}>{theme.label}</Text>
              </Pressable>
            )
          })}
        </View>
      </Card>

      <Card title="Privacy">
        <Body>
          Sessions keep running on your computer. The phone and the computer encrypt everything they send each other; the Zero-X relay passes it on without being able to
          read it. Notifications only say that a computer needs you or finished, never what it is doing.
        </Body>
      </Card>

      {__DEV__ ? (
        <Card title="Developer">
          <Body>Connect straight to a Neuron host on this network (bun run host --lan in ui/neuron-client), without the relay.</Body>
          <Button kind="secondary" label="Connect on this network" onPress={() => router.push('/lan')} />
        </Card>
      ) : null}

      <Card title="About">
        <Body>Neuron Mobile {Constants.expoConfig?.version ?? ''}, by Zero-X. Open-source licences are listed in the Neuron repository.</Body>
      </Card>
    </Screen>
  )
}
