import { Stack, router, useLocalSearchParams } from 'expo-router'
import { useCallback, useEffect, useState } from 'react'

import { Body, Button, Card, Screen } from '../../../src/components/ui'
import type { AttachState, RelayEngineTransport } from '../../../src/relay/transport'
import { SessionView, type SessionBanner } from '../../../src/screens/SessionScreen'
import { useApp, useStore } from '../../../src/store/context'
import type { SessionController } from '@neuron/client'

/** One shared session, attached over the relay while the screen is open. */
export default function SessionRoute() {
  const { host, share } = useLocalSearchParams<{ host: string; share: string }>()
  const store = useStore()
  const app = useApp()
  const [session, setSession] = useState<{ controller: SessionController; transport: RelayEngineTransport } | null>(null)
  const [attach, setAttach] = useState<AttachState>('attaching')
  const [error, setError] = useState<string | null>(null)
  const [clients, setClients] = useState(0)

  useEffect(() => {
    let opened: { controller: SessionController; transport: RelayEngineTransport }
    try {
      opened = store.openSession(host, share)
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
      return
    }
    setSession(opened)
    setError(null)
    const off = opened.transport.onAttach((state) => {
      setAttach(state)
      setClients(opened.transport.clients)
    })
    const offState = opened.controller.subscribe(() => setClients(opened.transport.clients))
    opened.controller.start().catch((failure: Error) => setError(failure.message))
    return () => {
      off()
      offState()
      void opened.controller.close()
    }
  }, [store, host, share])

  const onSeen = useCallback(() => {
    if (session) store.markViewed(host, share, session.transport.lastSeq)
  }, [store, session, host, share])

  const presence = app.hosts.find((candidate) => candidate.hostId === host)?.shares.find((candidate) => candidate.shareId === share)?.presence
  const title = presence?.title ?? presence?.project ?? 'Session'

  if (error && !session) {
    return (
      <Screen>
        <Stack.Screen options={{ title: 'Session' }} />
        <Card title="Cannot open this session">
          <Body>{error}</Body>
          <Button label="Back to sessions" onPress={() => router.replace('/')} />
        </Card>
      </Screen>
    )
  }
  if (!session) return null

  const relay = app.relay
  let banner: SessionBanner | null = null
  if (relay && (relay.status === 'waiting' || relay.status === 'connecting' || relay.status === 'offline')) {
    banner = { text: relay.status === 'offline' ? 'Offline. The session picks up where it left off when you are back.' : 'Reconnecting… the session resumes where it left off.' }
  } else if (attach === 'reattaching') {
    banner = { text: 'Attaching to the session again…' }
  } else if (attach === 'closed' || error) {
    banner = { tone: 'error', text: error ?? 'Disconnected from the computer.', action: { label: 'Retry', onPress: () => void session.transport.reconnect().then(() => setError(null), (failure: Error) => setError(failure.message)) } }
  }

  return (
    <>
      <Stack.Screen options={{ title }} />
      <SessionView controller={session.controller} hostName={store.computerName(host)} clients={clients} banner={banner} onSeen={onSeen} />
    </>
  )
}
