/** Small themed pieces the screens share. */

import { useMemo, type ReactNode } from 'react'
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, View, type ColorValue, type StyleProp, type ViewStyle } from 'react-native'

import type { Badge as BadgeKind } from '../store/sessions'
import { mono, useTheme, type Palette } from '../theme'

/** Styles made once per palette. */
export function useStyles<T extends object>(make: (colors: Palette) => T): T {
  const { colors } = useTheme()
  return useMemo(() => StyleSheet.create(make(colors) as never) as T, [colors, make])
}

const screenStyles = (colors: Palette) => ({
  screen: { flex: 1, backgroundColor: colors.bg },
  content: { padding: 16, gap: 12, paddingBottom: 32 },
})

export function Screen({ children, scroll = true }: { children: ReactNode; scroll?: boolean }) {
  const styles = useStyles(screenStyles)
  if (!scroll) return <View style={styles.screen}>{children}</View>
  return (
    <ScrollView style={styles.screen} contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
      {children}
    </ScrollView>
  )
}

const cardStyles = (colors: Palette) => ({
  card: { backgroundColor: colors.panel, borderRadius: 12, borderWidth: 1, borderColor: colors.border, padding: 14, gap: 8 },
  title: { color: colors.text, fontSize: 17, fontWeight: '600' as const },
  body: { color: colors.textMuted, fontSize: 14, lineHeight: 20 },
  label: { color: colors.textDim, fontSize: 12, fontWeight: '600' as const, textTransform: 'uppercase' as const, letterSpacing: 0.4 },
  code: { fontFamily: mono, color: colors.link },
})

export function Card({ title, children, style }: { title?: string; children?: ReactNode; style?: StyleProp<ViewStyle> }) {
  const styles = useStyles(cardStyles)
  return (
    <View style={[styles.card, style]}>
      {title ? <Text style={styles.title}>{title}</Text> : null}
      {children}
    </View>
  )
}

export function Body({ children }: { children: ReactNode }) {
  const styles = useStyles(cardStyles)
  return <Text style={styles.body}>{children}</Text>
}

export function Label({ children }: { children: ReactNode }) {
  const styles = useStyles(cardStyles)
  return <Text style={styles.label}>{children}</Text>
}

export function Code({ children }: { children: ReactNode }) {
  const styles = useStyles(cardStyles)
  return <Text style={styles.code}>{children}</Text>
}

const buttonStyles = (colors: Palette) => ({
  base: { borderRadius: 10, paddingVertical: 12, paddingHorizontal: 16, alignItems: 'center' as const, borderWidth: 1 },
  primary: { backgroundColor: colors.accent, borderColor: colors.accent },
  secondary: { backgroundColor: 'transparent', borderColor: colors.border },
  danger: { backgroundColor: 'transparent', borderColor: colors.danger },
  pressed: { opacity: 0.7 },
  disabled: { opacity: 0.45 },
  primaryText: { color: '#ffffff', fontSize: 15, fontWeight: '600' as const },
  secondaryText: { color: colors.text, fontSize: 15, fontWeight: '500' as const },
  dangerText: { color: colors.danger, fontSize: 15, fontWeight: '600' as const },
})

export function Button({
  label,
  onPress,
  kind = 'primary',
  busy = false,
  disabled = false,
  accessibilityLabel,
}: {
  label: string
  onPress: () => void
  kind?: 'primary' | 'secondary' | 'danger'
  busy?: boolean
  disabled?: boolean
  accessibilityLabel?: string
}) {
  const styles = useStyles(buttonStyles)
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel ?? label}
      accessibilityState={{ disabled: disabled || busy, busy }}
      disabled={disabled || busy}
      onPress={onPress}
      style={({ pressed }) => [styles.base, styles[kind], pressed && styles.pressed, (disabled || busy) && styles.disabled]}
    >
      {busy ? <ActivityIndicator color={kind === 'primary' ? '#ffffff' : undefined} /> : <Text style={styles[`${kind}Text`]}>{label}</Text>}
    </Pressable>
  )
}

const bannerStyles = (colors: Palette) => ({
  banner: { flexDirection: 'row' as const, alignItems: 'center' as const, gap: 10, paddingHorizontal: 14, paddingVertical: 10, backgroundColor: colors.sunken, borderBottomWidth: 1, borderBottomColor: colors.border },
  text: { flex: 1, fontSize: 13, color: colors.warning },
  error: { color: colors.danger },
  action: { color: colors.link, fontWeight: '600' as const },
})

export function Banner({ text, tone = 'warning', action }: { text: string; tone?: 'warning' | 'error'; action?: { label: string; onPress: () => void } }) {
  const styles = useStyles(bannerStyles)
  return (
    <View style={styles.banner} accessibilityRole="alert">
      <Text style={[styles.text, tone === 'error' && styles.error]}>{text}</Text>
      {action ? (
        <Pressable accessibilityRole="button" onPress={action.onPress} hitSlop={8}>
          <Text style={styles.action}>{action.label}</Text>
        </Pressable>
      ) : null}
    </View>
  )
}

const BADGE_TEXT: Record<Exclude<BadgeKind, null>, string> = { running: 'running', needs_me: 'needs you', failed: 'failed', unread: 'new' }

export function Badge({ kind }: { kind: BadgeKind }) {
  const { colors } = useTheme()
  if (!kind) return null
  const color = kind === 'needs_me' ? colors.warning : kind === 'failed' ? colors.danger : kind === 'running' ? colors.accent : colors.success
  return (
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 5 }} accessibilityLabel={BADGE_TEXT[kind]}>
      <View style={{ width: 8, height: 8, borderRadius: 4, backgroundColor: color }} />
      <Text style={{ color, fontSize: 12, fontWeight: '600' }}>{BADGE_TEXT[kind]}</Text>
    </View>
  )
}

/** A text glyph for a tab, since the app ships no icon font. */
export function TabGlyph({ glyph, color }: { glyph: string; color: ColorValue }) {
  return <Text style={{ color, fontSize: 18, lineHeight: 22 }}>{glyph}</Text>
}
