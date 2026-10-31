import { StyleSheet, View } from 'react-native'

import { logoLetters } from '../theme'

const GLYPHS: Record<string, string[]> = {
  N: ['10001', '11001', '10101', '10011', '10001'],
  E: ['11111', '10000', '11110', '10000', '11111'],
  U: ['10001', '10001', '10001', '10001', '11111'],
  R: ['11110', '10001', '11110', '10100', '10011'],
  O: ['01110', '10001', '10001', '10001', '01110'],
}

/** The NEURON wordmark as pixels, like the terminal UI draws it. */
export function PixelLogo({ pixel = 6 }: { pixel?: number }) {
  return (
    <View style={styles.row} accessibilityRole="image" accessibilityLabel="Neuron">
      {logoLetters.map(([letter, color], index) => (
        <View key={index} style={{ marginRight: index < logoLetters.length - 1 ? pixel : 0 }}>
          {GLYPHS[letter].map((row, y) => (
            <View key={y} style={styles.row}>
              {[...row].map((bit, x) => (
                <View key={x} style={{ width: pixel, height: pixel, backgroundColor: bit === '1' ? color : 'transparent' }} />
              ))}
            </View>
          ))}
        </View>
      ))}
    </View>
  )
}

const styles = StyleSheet.create({ row: { flexDirection: 'row' } })
