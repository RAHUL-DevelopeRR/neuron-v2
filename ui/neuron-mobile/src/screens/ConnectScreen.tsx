import { useState } from 'react'
import { ActivityIndicator, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, TextInput, View } from 'react-native'

import { connect, type Connection } from '../connection'
import { PixelLogo } from '../components/PixelLogo'
import { colors, mono } from '../theme'

/** Pair with a Neuron host: the address and the token it printed. */
export function ConnectScreen({
  initialAddress,
  initialToken,
  onConnected,
}: {
  initialAddress?: string
  initialToken?: string
  onConnected: (connection: Connection, address: string) => void
}) {
  const [address, setAddress] = useState(initialAddress ?? '')
  const [token, setToken] = useState(initialToken ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      onConnected(await connect(address, token), address)
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setBusy(false)
    }
  }

  return (
    <KeyboardAvoidingView style={styles.screen} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
      <View style={styles.card}>
        <PixelLogo pixel={7} />
        <Text style={styles.title}>Remote control</Text>
        <Text style={styles.help}>
          On your computer, run <Text style={styles.code}>bun run host --lan</Text> in ui/neuron-client, then enter the address and the
          pairing token it gives you. The phone and the computer need to be on the same network.
        </Text>
        <Text style={styles.label}>Host address</Text>
        <TextInput
          accessibilityLabel="Host address"
          style={styles.input}
          value={address}
          onChangeText={setAddress}
          placeholder="192.168.1.5:51967"
          placeholderTextColor={colors.textFaint}
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="url"
        />
        <Text style={styles.label}>Pairing token</Text>
        <TextInput
          accessibilityLabel="Pairing token"
          style={styles.input}
          value={token}
          onChangeText={setToken}
          placeholder="from the host's token file"
          placeholderTextColor={colors.textFaint}
          autoCapitalize="none"
          autoCorrect={false}
          secureTextEntry
        />
        {error && (
          <Text style={styles.error} accessibilityRole="alert">
            {error}
          </Text>
        )}
        <Pressable
          accessibilityRole="button"
          style={({ pressed }) => [styles.button, (busy || pressed) && styles.buttonPressed]}
          onPress={submit}
          disabled={busy}
        >
          {busy ? <ActivityIndicator color="#fff" /> : <Text style={styles.buttonText}>Connect</Text>}
        </Pressable>
      </View>
    </KeyboardAvoidingView>
  )
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.void, justifyContent: 'center', padding: 20 },
  card: { backgroundColor: colors.panel, borderRadius: 14, padding: 20, borderWidth: 1, borderColor: colors.border, gap: 8 },
  title: { color: colors.text, fontSize: 20, fontWeight: '600', marginTop: 12 },
  help: { color: colors.textDim, fontSize: 14, lineHeight: 20, marginBottom: 8 },
  code: { fontFamily: mono, color: colors.link },
  label: { color: colors.accent, fontSize: 12, fontWeight: '600', textTransform: 'uppercase', marginTop: 6 },
  input: {
    backgroundColor: colors.sunken,
    color: colors.text,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: colors.border,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 16,
    fontFamily: mono,
  },
  error: { color: colors.diffRemoveText, marginTop: 4 },
  button: { backgroundColor: colors.accent, borderRadius: 8, paddingVertical: 12, alignItems: 'center', marginTop: 12 },
  buttonPressed: { opacity: 0.7 },
  buttonText: { color: '#fff', fontSize: 16, fontWeight: '600' },
})
