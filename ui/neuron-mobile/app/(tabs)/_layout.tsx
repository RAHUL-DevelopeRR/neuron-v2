import { Tabs } from 'expo-router'

import { TabGlyph } from '../../src/components/ui'
import { useTheme } from '../../src/theme'

export default function TabsLayout() {
  const { colors } = useTheme()
  return (
    <Tabs
      screenOptions={{
        headerStyle: { backgroundColor: colors.panel },
        headerTintColor: colors.text,
        headerTitleStyle: { color: colors.text },
        tabBarStyle: { backgroundColor: colors.panel, borderTopColor: colors.border },
        tabBarActiveTintColor: colors.accent,
        tabBarInactiveTintColor: colors.textDim,
        sceneStyle: { backgroundColor: colors.bg },
      }}
    >
      <Tabs.Screen name="index" options={{ title: 'Sessions', tabBarIcon: ({ color }) => <TabGlyph glyph="☰" color={color} /> }} />
      <Tabs.Screen name="computers" options={{ title: 'Computers', tabBarIcon: ({ color }) => <TabGlyph glyph="▭" color={color} /> }} />
      <Tabs.Screen name="account" options={{ title: 'Account', tabBarIcon: ({ color }) => <TabGlyph glyph="◉" color={color} /> }} />
    </Tabs>
  )
}
