import { useState } from 'react'
import { StatusBar, View } from 'react-native'

import type { Connection } from './src/connection'
import { ConnectScreen } from './src/screens/ConnectScreen'
import { SessionScreen } from './src/screens/SessionScreen'
import { colors } from './src/theme'

// For a dev preview, `EXPO_PUBLIC_NEURON_HOST_URL` fills in the address. The token
// is never baked in: it is typed, or pasted from the host's token file.
const devAddress = process.env.EXPO_PUBLIC_NEURON_HOST_URL ?? ''

export default function App() {
  const [connection, setConnection] = useState<Connection | null>(null)
  const [address, setAddress] = useState(devAddress)
  return (
    <View style={{ flex: 1, backgroundColor: colors.void }}>
      <StatusBar barStyle="light-content" />
      {connection ? (
        <SessionScreen
          connection={connection}
          onLeave={() => {
            void connection.controller.close()
            setConnection(null)
          }}
        />
      ) : (
        <ConnectScreen
          initialAddress={address}
          onConnected={(connected, used) => {
            setAddress(used)
            setConnection(connected)
          }}
        />
      )}
    </View>
  )
}
