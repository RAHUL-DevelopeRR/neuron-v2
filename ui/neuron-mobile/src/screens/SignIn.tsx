import * as WebBrowser from 'expo-web-browser'
import { useEffect, useRef, useState } from 'react'
import { ActivityIndicator, Text, TextInput, View } from 'react-native'

import type { DeviceCode } from '../account/gateway'
import { Body, Button, Card, Label, useStyles } from '../components/ui'
import { useStore } from '../store/context'
import { mono, useTheme, type Palette } from '../theme'

const makeStyles = (colors: Palette) => ({
  code: { color: colors.text, fontFamily: mono, fontSize: 30, letterSpacing: 4, textAlign: 'center' as const, marginVertical: 8 },
  waiting: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 8 },
  dim: { color: colors.textDim, fontSize: 13 },
  error: { color: colors.danger, fontSize: 14 },
  input: { backgroundColor: colors.sunken, color: colors.text, borderRadius: 8, borderWidth: 1, borderColor: colors.border, paddingHorizontal: 12, paddingVertical: 10, fontFamily: mono },
})

/**
 * Device-code sign-in: the gateway gives a short code, the user approves it on
 * the Zero-X page in the browser (where they are signed in already), and the
 * app picks up the session. No password is typed here.
 */
export function SignIn({ gateway: initialGateway, onDone }: { gateway: string; onDone: () => void }) {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  const store = useStore()
  const [gateway, setGateway] = useState(initialGateway)
  const [code, setCode] = useState<DeviceCode | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)
  const signal = useRef({ aborted: false })

  useEffect(() => {
    const current = { aborted: false }
    signal.current = current
    setError(null)
    setCode(null)
    store
      .startSignIn(gateway)
      .then(async (next) => {
        if (current.aborted) return
        setCode(next)
        await store.completeSignIn(gateway, next, current)
        if (!current.aborted) onDone()
      })
      .catch((failure: Error) => {
        if (!current.aborted) setError(failure.message)
      })
    return () => {
      current.aborted = true
    }
    // `gateway` changes only when the user edits it and retries.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [attempt])

  const approve = () => {
    if (code) void WebBrowser.openBrowserAsync(code.verification_uri_complete ?? code.verification_uri)
  }

  return (
    <Card title="Approve this phone in your browser">
      <Body>Open the Zero-X page, check that it shows this code, and approve. Use the account your computer is signed in to.</Body>
      {code ? <Text style={styles.code} accessibilityLabel={`Code ${code.user_code.split('').join(' ')}`}>{code.user_code}</Text> : null}
      {code ? <Button label="Open the approval page" onPress={approve} /> : null}
      {code && !error ? (
        <View style={styles.waiting}>
          <ActivityIndicator color={colors.accent} />
          <Text style={styles.dim}>Waiting for your approval…</Text>
        </View>
      ) : null}
      {!code && !error ? <ActivityIndicator color={colors.accent} /> : null}
      {error ? <Text style={styles.error}>{error}</Text> : null}
      {error ? <Button kind="secondary" label="Try again" onPress={() => setAttempt(attempt + 1)} /> : null}
      {__DEV__ ? (
        <>
          <Label>Zero-X server (development builds)</Label>
          <TextInput
            accessibilityLabel="Zero-X server"
            style={styles.input}
            value={gateway}
            onChangeText={setGateway}
            onSubmitEditing={() => setAttempt(attempt + 1)}
            autoCapitalize="none"
            autoCorrect={false}
            keyboardType="url"
          />
        </>
      ) : null}
    </Card>
  )
}
