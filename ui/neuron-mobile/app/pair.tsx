import { CameraView, useCameraPermissions } from 'expo-camera'
import { router, useLocalSearchParams } from 'expo-router'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Platform, Text, TextInput, View } from 'react-native'

import { Body, Button, Card, Label, Screen, useStyles } from '../src/components/ui'
import { SignIn } from '../src/screens/SignIn'
import { NeedsSignIn } from '../src/store/app'
import { useStore } from '../src/store/context'
import { mono, useTheme, type Palette } from '../src/theme'

const makeStyles = (colors: Palette) => ({
  camera: { height: 300, borderRadius: 12, overflow: 'hidden' as const, backgroundColor: colors.sunken },
  input: { backgroundColor: colors.sunken, color: colors.text, borderRadius: 8, borderWidth: 1, borderColor: colors.border, padding: 10, fontFamily: mono, fontSize: 13, minHeight: 64, textAlignVertical: 'top' as const },
  error: { color: colors.danger, fontSize: 14 },
  done: { color: colors.success, fontSize: 16, fontWeight: '600' as const },
})

type Step = { kind: 'scan' } | { kind: 'pairing' } | { kind: 'sign-in'; gateway: string; link: string } | { kind: 'done'; name: string } | { kind: 'error'; message: string }

/** A link that arrived as `neuron://pair?...` (the camera app, or a tap on the https link) comes in as route params. */
function linkFromParams(params: Record<string, string | string[] | undefined>): string | null {
  if (typeof params.c !== 'string' || typeof params.k !== 'string') return null
  const fields = new URLSearchParams()
  for (const key of ['v', 'r', 'h', 'c', 'k', 'n', 'u', 'e']) {
    const value = params[key]
    if (typeof value === 'string') fields.set(key, value)
  }
  return `neuron://pair?${fields.toString()}`
}

/**
 * Pairing: scan the QR code the computer shows (or paste its link). The phone
 * signs in first when it has to, and the relay checks both ends are on the
 * same Zero-X account. The keys stay on the two devices.
 */
export default function PairScreen() {
  const styles = useStyles(makeStyles)
  const { colors } = useTheme()
  const store = useStore()
  const params = useLocalSearchParams()
  const [step, setStep] = useState<Step>({ kind: 'scan' })
  const [pasted, setPasted] = useState('')
  const [permission, requestPermission] = useCameraPermissions()
  const busy = useRef(false)
  const native = Platform.OS !== 'web'

  const pair = useCallback(
    async (link: string) => {
      if (busy.current) return
      busy.current = true
      setStep({ kind: 'pairing' })
      try {
        const computer = await store.pair(link)
        setStep({ kind: 'done', name: computer.name })
      } catch (error) {
        if (error instanceof NeedsSignIn) setStep({ kind: 'sign-in', gateway: error.gateway, link })
        else setStep({ kind: 'error', message: error instanceof Error ? error.message : String(error) })
      } finally {
        busy.current = false
      }
    },
    [store],
  )

  const fromLink = useRef(linkFromParams(params))
  useEffect(() => {
    if (fromLink.current) void pair(fromLink.current)
  }, [pair])

  if (step.kind === 'sign-in') {
    return (
      <Screen>
        <Body>First, sign in to Zero-X with the account your computer uses.</Body>
        <SignIn gateway={step.gateway} onDone={() => void pair(step.link)} />
      </Screen>
    )
  }

  if (step.kind === 'done') {
    return (
      <Screen>
        <Card>
          <Text style={styles.done}>Paired with {step.name}</Text>
          <Body>Its shared sessions show in Sessions. Turn on Continue on phone for any other session you want here.</Body>
          <Button label="See sessions" onPress={() => router.replace('/')} />
        </Card>
      </Screen>
    )
  }

  return (
    <Screen>
      {native ? (
        permission?.granted ? (
          <View style={styles.camera}>
            <CameraView
              style={{ flex: 1 }}
              facing="back"
              barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
              onBarcodeScanned={step.kind === 'pairing' ? undefined : ({ data }) => void pair(data)}
            />
          </View>
        ) : (
          <Card title="Scan the code on your computer">
            <Body>Neuron uses the camera only to read the pairing code.</Body>
            <Button label={permission?.canAskAgain === false ? 'Camera blocked in Settings' : 'Use the camera'} disabled={permission?.canAskAgain === false} onPress={() => void requestPermission()} />
          </Card>
        )
      ) : (
        <Card title="Paste the pairing link">
          <Body>The web build has no camera. Copy the link the computer prints under its QR code and paste it here.</Body>
        </Card>
      )}

      {step.kind === 'pairing' ? <Body>Pairing…</Body> : null}
      {step.kind === 'error' ? <Text style={styles.error}>{step.message}</Text> : null}

      <Label>Or paste the pairing link</Label>
      <TextInput
        accessibilityLabel="Pairing link"
        style={styles.input}
        value={pasted}
        onChangeText={setPasted}
        placeholder="neuron://pair?v=1&…"
        placeholderTextColor={colors.textFaint}
        autoCapitalize="none"
        autoCorrect={false}
        multiline
      />
      <Button label="Pair" busy={step.kind === 'pairing'} disabled={!pasted.trim()} onPress={() => void pair(pasted)} />
    </Screen>
  )
}
