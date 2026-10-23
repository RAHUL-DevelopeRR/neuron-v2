import { Fragment, useMemo } from 'react'
import { Linking, ScrollView, Text, View } from 'react-native'

import { parseMarkdown, type Inline } from '../markdown/parse'
import { mono, type Palette } from '../theme'
import { useStyles } from './ui'

const makeStyles = (colors: Palette) => ({
  text: { color: colors.text, fontSize: 15, lineHeight: 22 },
  code: { fontFamily: mono, fontSize: 13, color: colors.link, backgroundColor: colors.sunken },
  strong: { fontWeight: '700' as const },
  em: { fontStyle: 'italic' as const },
  link: { color: colors.link, textDecorationLine: 'underline' as const },
  h1: { fontSize: 19, fontWeight: '700' as const },
  h2: { fontSize: 17, fontWeight: '700' as const },
  h3: { fontSize: 16, fontWeight: '600' as const },
  block: { marginVertical: 3 },
  codeBlock: { backgroundColor: colors.sunken, borderRadius: 8, borderWidth: 1, borderColor: colors.border, marginVertical: 4 },
  codeLang: { color: colors.textDim, fontSize: 11, paddingHorizontal: 10, paddingTop: 6 },
  codeText: { fontFamily: mono, fontSize: 13, lineHeight: 19, color: colors.text, padding: 10 },
  quote: { borderLeftWidth: 3, borderLeftColor: colors.border, paddingLeft: 10, marginVertical: 3 },
  quoteText: { color: colors.textMuted, fontStyle: 'italic' as const },
  item: { flexDirection: 'row' as const, gap: 6, marginVertical: 1 },
  bullet: { color: colors.textDim, fontSize: 15, lineHeight: 22, minWidth: 16 },
})

/** Links open only after the parser kept them (http and https). */
const open = (url: string) => {
  if (/^https?:\/\//i.test(url)) void Linking.openURL(url)
}

function Inlines({ inlines, styles }: { inlines: Inline[]; styles: ReturnType<typeof makeStyles> }) {
  return (
    <>
      {inlines.map((inline, index) => {
        switch (inline.kind) {
          case 'text':
            return <Fragment key={index}>{inline.text}</Fragment>
          case 'code':
            return (
              <Text key={index} style={styles.code}>
                {inline.text}
              </Text>
            )
          case 'strong':
            return (
              <Text key={index} style={styles.strong}>
                {inline.text}
              </Text>
            )
          case 'em':
            return (
              <Text key={index} style={styles.em}>
                {inline.text}
              </Text>
            )
          case 'link':
            return (
              <Text key={index} style={styles.link} accessibilityRole="link" onPress={() => open(inline.url)}>
                {inline.text}
              </Text>
            )
        }
      })}
    </>
  )
}

/** The transcript's Markdown, drawn with React Native text (everything is text, nothing is HTML). */
export function Markdown({ text }: { text: string }) {
  const styles = useStyles(makeStyles)
  const blocks = useMemo(() => parseMarkdown(text), [text])
  return (
    <View>
      {blocks.map((block, index) => {
        switch (block.kind) {
          case 'code':
            return (
              <View key={index} style={styles.codeBlock}>
                {block.lang ? <Text style={styles.codeLang}>{block.lang}</Text> : null}
                <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                  <Text style={styles.codeText} selectable>
                    {block.text}
                  </Text>
                </ScrollView>
              </View>
            )
          case 'heading':
            return (
              <Text key={index} style={[styles.text, styles.block, block.level === 1 ? styles.h1 : block.level === 2 ? styles.h2 : styles.h3]} accessibilityRole="header">
                <Inlines inlines={block.inlines} styles={styles} />
              </Text>
            )
          case 'list':
            return (
              <View key={index} style={styles.block}>
                {block.items.map((item, itemIndex) => (
                  <View key={itemIndex} style={styles.item}>
                    <Text style={styles.bullet}>{block.ordered ? `${itemIndex + 1}.` : '•'}</Text>
                    <Text style={[styles.text, { flex: 1 }]}>
                      <Inlines inlines={item} styles={styles} />
                    </Text>
                  </View>
                ))}
              </View>
            )
          case 'quote':
            return (
              <View key={index} style={styles.quote}>
                <Text style={[styles.text, styles.quoteText]}>
                  <Inlines inlines={block.inlines} styles={styles} />
                </Text>
              </View>
            )
          case 'paragraph':
            return (
              <Text key={index} style={[styles.text, styles.block]} selectable>
                {block.lines.map((line, lineIndex) => (
                  <Fragment key={lineIndex}>
                    {lineIndex > 0 ? '\n' : null}
                    <Inlines inlines={line} styles={styles} />
                  </Fragment>
                ))}
              </Text>
            )
        }
      })}
    </View>
  )
}
