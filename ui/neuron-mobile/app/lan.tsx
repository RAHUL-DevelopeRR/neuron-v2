import { useState } from 'react'

import type { Connection } from '../src/connection'
import { ConnectScreen } from '../src/screens/ConnectScreen'
import { SessionView } from '../src/screens/SessionScreen'

// For a dev preview, `EXPO_PUBLIC_NEURON_HOST_URL` fills in the address. The token
// is never baked in: it is typed, or pasted from the host's token file.
const devAddress = process.env.EXPO_PUBLIC_NEURON_HOST_URL ?? ''

/** Development: a Neuron host on this network, straight over the LAN (no relay, no encryption). */
export default function LanScreen() {
  const [connection, setConnection] = useState<Connection | null>(null)
  const [address, setAddress] = useState(devAddress)
  if (connection) {
    return (
      <SessionView
        controller={connection.controller}
        hostName={connection.transport.host?.name ?? null}
        clients={connection.transport.clients}
        onLeave={() => {
          void connection.controller.close()
          setConnection(null)
        }}
      />
    )
  }
  return (
    <ConnectScreen
      initialAddress={address}
      onConnected={(connected, used) => {
        setAddress(used)
        setConnection(connected)
      }}
    />
  )
}
