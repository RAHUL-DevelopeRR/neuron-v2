import { router } from 'expo-router'

import type { RelayStatus } from '../store/app'
import { useStore } from '../store/context'
import { Banner } from './ui'

/** The relay connection, when it is not simply up: reconnecting, offline, or stopped until the user acts. */
export function RelayBanner({ relay }: { relay: RelayStatus | null }) {
  const store = useStore()
  if (!relay || relay.status === 'open' || relay.status === 'idle') return null
  if (relay.status === 'failed') {
    const signIn = /sign.?in/i.test(relay.error ?? '')
    return (
      <Banner
        tone="error"
        text={relay.error ?? 'The relay stopped this connection.'}
        action={signIn ? { label: 'Sign in', onPress: () => router.push('/sign-in') } : { label: 'Retry', onPress: () => store.retryRelays() }}
      />
    )
  }
  if (relay.status === 'offline') return <Banner text="You are offline. Neuron reconnects when the network is back." />
  const seconds = relay.delayMs ? Math.ceil(relay.delayMs / 1000) : 0
  return (
    <Banner
      text={relay.status === 'waiting' ? `Reconnecting to the relay in ${seconds} s…` : 'Connecting to the relay…'}
      action={relay.status === 'waiting' ? { label: 'Now', onPress: () => store.wake() } : undefined}
    />
  )
}
