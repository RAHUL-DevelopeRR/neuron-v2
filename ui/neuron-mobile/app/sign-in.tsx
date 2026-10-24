import { router } from 'expo-router'

import { Screen } from '../src/components/ui'
import { SignIn } from '../src/screens/SignIn'
import { useStore } from '../src/store/context'

const devGateway = process.env.EXPO_PUBLIC_NEURON_GATEWAY_URL

export default function SignInScreen() {
  const store = useStore()
  return (
    <Screen>
      <SignIn gateway={(__DEV__ && devGateway) || store.defaultGateway()} onDone={() => (router.canGoBack() ? router.back() : router.replace('/'))} />
    </Screen>
  )
}
