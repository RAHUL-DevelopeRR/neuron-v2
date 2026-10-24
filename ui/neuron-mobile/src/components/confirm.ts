import { Alert, Platform } from 'react-native'

/** Asks before something that cannot be undone. The web build has no Alert buttons, so it uses the browser's dialog. */
export function confirm(title: string, message: string, action: string): Promise<boolean> {
  if (Platform.OS === 'web') {
    const dialog = (globalThis as { confirm?: (text: string) => boolean }).confirm
    return Promise.resolve(dialog ? dialog(`${title}\n\n${message}`) : true)
  }
  return new Promise((resolve) =>
    Alert.alert(title, message, [
      { text: 'Cancel', style: 'cancel', onPress: () => resolve(false) },
      { text: action, style: 'destructive', onPress: () => resolve(true) },
    ]),
  )
}
