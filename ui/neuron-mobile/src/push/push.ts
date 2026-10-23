/**
 * Push on the phone: the Expo push token the gateway sends to, Android
 * channels for the two switches, and opening the session a push is about.
 *
 * The relay decides when to push (an approval waited 30 s with no phone
 * attached, or a turn finished) and the push says only which computer, never
 * what the session is doing (docs/RELAY_CONTRACT.md §7).
 *
 * expo-notifications is required lazily: Expo Go on Android has no remote
 * push since SDK 53, and the web build has none, so both get a reason instead
 * of a crash.
 */

import Constants, { ExecutionEnvironment } from 'expo-constants'
import { Platform } from 'react-native'

import { isRelayId, type NotifyKind } from '../relay/contract'

type Notifications = typeof import('expo-notifications')

const notifications = (): Notifications => require('expo-notifications') as Notifications

/** What a Neuron push carries in `data`: `{neuron: {host_id, share_id, kind}}`. */
export function parsePushData(data: unknown): { hostId: string; shareId: string; kind: NotifyKind } | null {
  const neuron = (data as { neuron?: Record<string, unknown> } | null)?.neuron
  if (!neuron || !isRelayId(neuron.host_id) || !isRelayId(neuron.share_id)) return null
  if (neuron.kind !== 'needs_me' && neuron.kind !== 'finished') return null
  return { hostId: neuron.host_id, shareId: neuron.share_id, kind: neuron.kind }
}

async function ensureChannels(api: Notifications): Promise<void> {
  if (Platform.OS !== 'android') return
  await api.setNotificationChannelAsync('needs-me', { name: 'Needs me', importance: api.AndroidImportance.HIGH })
  await api.setNotificationChannelAsync('finished', { name: 'Finished', importance: api.AndroidImportance.DEFAULT })
}

/** Asks for permission (once) and returns this phone's Expo push token, or why there is none. */
export async function expoPushToken(): Promise<{ token: string } | { unsupported: string } | { denied: true }> {
  if (Platform.OS === 'web') return { unsupported: 'The web build gets no notifications. Use the phone app.' }
  const Device = require('expo-device') as typeof import('expo-device')
  if (!Device.isDevice) return { unsupported: 'Notifications need a real phone, not a simulator.' }
  if (Constants.executionEnvironment === ExecutionEnvironment.StoreClient && Platform.OS === 'android') {
    return { unsupported: 'Expo Go on Android cannot receive push. The Neuron app build can.' }
  }
  const extra = Constants.expoConfig?.extra as { eas?: { projectId?: string } } | undefined
  const projectId = extra?.eas?.projectId ?? (Constants as { easConfig?: { projectId?: string } }).easConfig?.projectId
  if (!projectId) return { unsupported: 'This build has no push project yet; it comes with the store builds.' }
  const api = notifications()
  await ensureChannels(api)
  let { status } = await api.getPermissionsAsync()
  if (status !== 'granted') status = (await api.requestPermissionsAsync()).status
  if (status !== 'granted') return { denied: true }
  const { data } = await api.getExpoPushTokenAsync({ projectId })
  return { token: data }
}

/** Shows pushes while the app is open, and opens the session a tapped push is about. */
export function handleNotifications(open: (hostId: string, shareId: string) => void): () => void {
  if (Platform.OS === 'web') return () => {}
  try {
    const api = notifications()
    api.setNotificationHandler({
      handleNotification: async () => ({ shouldShowBanner: true, shouldShowList: true, shouldPlaySound: true, shouldSetBadge: false }),
    })
    const opened = (data: unknown) => {
      const target = parsePushData(data)
      if (target) open(target.hostId, target.shareId)
    }
    // A push that launched the app.
    const last = api.getLastNotificationResponse()
    if (last) opened(last.notification.request.content.data)
    const subscription = api.addNotificationResponseReceivedListener((response) => opened(response.notification.request.content.data))
    return () => subscription.remove()
  } catch {
    return () => {}
  }
}
