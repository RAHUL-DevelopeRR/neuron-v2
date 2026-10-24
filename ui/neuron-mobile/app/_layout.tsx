import '../src/polyfills'

import { Stack, router } from 'expo-router'
import { StatusBar } from 'expo-status-bar'
import { useEffect } from 'react'

import { handleNotifications } from '../src/push/push'
import { AppProvider } from '../src/store/context'
import { useTheme } from '../src/theme'

function Navigator() {
  const { colors, name } = useTheme()
  // A tapped push opens the session it is about.
  useEffect(() => handleNotifications((host, share) => router.push({ pathname: '/session/[host]/[share]', params: { host, share } })), [])
  return (
    <>
      <StatusBar style={name === 'dark' ? 'light' : 'dark'} />
      <Stack
        screenOptions={{
          headerStyle: { backgroundColor: colors.panel },
          headerTintColor: colors.text,
          headerTitleStyle: { color: colors.text },
          contentStyle: { backgroundColor: colors.bg },
        }}
      >
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen name="pair" options={{ title: 'Pair a computer', presentation: 'modal' }} />
        <Stack.Screen name="sign-in" options={{ title: 'Sign in to Zero-X', presentation: 'modal' }} />
        <Stack.Screen name="session/[host]/[share]" options={{ title: 'Session' }} />
        <Stack.Screen name="settings" options={{ title: 'Settings' }} />
        <Stack.Screen name="lan" options={{ title: 'On this network' }} />
      </Stack>
    </>
  )
}

export default function RootLayout() {
  return (
    <AppProvider>
      <Navigator />
    </AppProvider>
  )
}
