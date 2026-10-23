import { useState } from 'react'
import { Pressable, ScrollView, Text, View } from 'react-native'

import type { Diff } from '@neuron/client'

import { mono, type Palette } from '../theme'
import { useStyles } from './ui'

const makeStyles = (colors: Palette) => ({
  box: { backgroundColor: colors.sunken, borderRadius: 8, borderWidth: 1, borderColor: colors.border, overflow: 'hidden' as const },
  line: { flexDirection: 'row' as const },
  number: { fontFamily: mono, fontSize: 11, lineHeight: 18, color: colors.textFaint, width: 34, textAlign: 'right' as const, paddingRight: 6 },
  text: { fontFamily: mono, fontSize: 12, lineHeight: 18, color: colors.text, paddingRight: 10 },
  add: { backgroundColor: colors.diffAddBg },
  addText: { color: colors.diffAddText },
  remove: { backgroundColor: colors.diffRemoveBg },
  removeText: { color: colors.diffRemoveText },
  hunk: { color: colors.textDim },
  more: { color: colors.link, fontSize: 13, padding: 8, textAlign: 'center' as const },
  note: { color: colors.textDim, fontSize: 12, padding: 8 },
})

/** An edit's lines, added and removed, with line numbers. Long diffs show their start until expanded. */
export function DiffView({ diff, collapsedLines = 24 }: { diff: Diff; collapsedLines?: number }) {
  const styles = useStyles(makeStyles)
  const [open, setOpen] = useState(diff.lines.length <= collapsedLines)
  const lines = open ? diff.lines : diff.lines.slice(0, collapsedLines)
  return (
    <View style={styles.box} accessibilityLabel={`Diff: ${diff.added} added, ${diff.removed} removed`}>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}>
        <View>
          {lines.map((line, index) => {
            const sign = line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : line.kind === 'hunk' ? '' : ' '
            return (
              <View key={index} style={[styles.line, line.kind === 'add' && styles.add, line.kind === 'remove' && styles.remove]}>
                <Text style={styles.number}>{line.kind === 'remove' ? line.oldNumber ?? '' : line.newNumber ?? ''}</Text>
                <Text style={[styles.text, line.kind === 'add' && styles.addText, line.kind === 'remove' && styles.removeText, line.kind === 'hunk' && styles.hunk]}>
                  {sign} {line.text}
                </Text>
              </View>
            )
          })}
        </View>
      </ScrollView>
      {!open ? (
        <Pressable accessibilityRole="button" onPress={() => setOpen(true)}>
          <Text style={styles.more}>Show all {diff.lines.length} lines</Text>
        </Pressable>
      ) : null}
      {diff.truncated ? <Text style={styles.note}>The rest of this diff is left out.</Text> : null}
    </View>
  )
}
