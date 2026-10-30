#!/usr/bin/env node
// Builds assets/readme/neuron-session.svg, the animated session illustration in
// the README. Run from anywhere:
//
//   node assets/readme/src/build-session-svg.mjs
//
// The picture is drawn from Neuron's own strings: the device sign-in lines of
// the `neuron` CLI crate (src/device_login.rs and src/auth.rs), the permission
// reason of rust/crates/runtime/src/permissions.rs, and the header, tool rows,
// permission dialog and footer of the full-screen UI
// (ui/neuron-tui/src/components). The block logo, its letter colours and the
// sheen timing are read from ui/neuron-tui/src/utils/logo.ts, so they follow
// the UI. The session itself (project, task, files, test names) is invented.
//
// Constraints, because GitHub serves README images with a strict CSP:
// pure vector, inline CSS animation only, no scripts, no web fonts, no data: URIs.
// Everything shares one 28 s loop; the static styles show the final frame, which
// is what viewers with reduced motion get.

import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const repo = join(here, '..', '..', '..')
const out = join(here, '..', 'neuron-session.svg')

// ---------------------------------------------------------------- logo source

const logoTs = readFileSync(join(repo, 'ui/neuron-tui/src/utils/logo.ts'), 'utf8')
const pick = (pattern, what) => {
  const match = logoTs.match(pattern)
  if (!match) throw new Error(`logo.ts: ${what} not found`)
  return match[1]
}
const BRAND = Object.fromEntries(
  ['blue', 'red', 'orange', 'green', 'cyan'].map((name) => [
    name,
    pick(new RegExp(`${name}: '(#[0-9A-Fa-f]{6})'`), `BRAND.${name}`),
  ]),
)
const LOGO_LINES = pick(/export const LOGO = `([^`]*)`/, 'LOGO')
  .split(/\r?\n/) // a Windows checkout may have CRLF line endings
  .filter((line) => line.length > 0)
if (LOGO_LINES.length !== 6) throw new Error(`expected a 6-row logo, got ${LOGO_LINES.length}`)
const LETTER_STARTS = pick(/const LOGO_LETTER_STARTS = \[([^\]]*)\]/, 'LOGO_LETTER_STARTS')
  .split(',')
  .map((value) => Number(value.trim()))
const LETTER_COLORS = pick(/LOGO_LETTER_COLORS = \[([^\]]*)\]/, 'LOGO_LETTER_COLORS')
  .split(',')
  .map((value) => value.trim())
  .filter(Boolean)
  .map((ref) => BRAND[ref.replace('BRAND.', '')])
const SHEEN_STEP = Number(pick(/SHEEN_STEP = (\d+)/, 'SHEEN_STEP'))
const SHEEN_INTERVAL = Number(pick(/SHEEN_INTERVAL_MS = (\d+)/, 'SHEEN_INTERVAL_MS')) / 1000

const letterColor = (column) => {
  let letter = 0
  while (letter + 1 < LETTER_STARTS.length && column >= LETTER_STARTS[letter + 1]) letter++
  return LETTER_COLORS[letter]
}
// logoShadowColor() for the dark theme: the letter colour mixed 45% toward black.
const shadowOf = (hex) =>
  '#' +
  [1, 3, 5]
    .map((at) => Math.round(parseInt(hex.slice(at, at + 2), 16) * 0.55))
    .map((value) => value.toString(16).padStart(2, '0'))
    .join('')

// ---------------------------------------------------------------- geometry

const W = 960
const H = 600
const CARD = { x: 16, y: 10, w: 928, h: 578, r: 12 }
const TITLE_H = 36
const CW = 8.4 // one terminal column at 14px
const LH = 20 // one terminal row
const COLS = 106
const ROWS = 26
const X0 = CARD.x + (CARD.w - COLS * CW) / 2
const Y0 = CARD.y + TITLE_H + 8
const BASELINE = 15

const LOOP = 28 // seconds
const n = (value) => Number(value.toFixed(2)).toString()
const xOf = (col) => X0 + col * CW
const yOf = (row) => Y0 + row * LH

// Zero-X surfaces and the full-screen UI's dark theme (theme-system.ts).
const C = {
  card: '#0A0A0C',
  bar: '#111114',
  edge: '#2A2A32',
  dim: '#8A8A96',
  fg: '#DCDCE6',
  muted: '#acb3bf',
  shell: '#E8E8EC',
  added: '#7ACC35', // diff-viewer.tsx, dark
  removed: '#BF6C69',
  ...BRAND,
}

// ---------------------------------------------------------------- text

const esc = (text) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')

/**
 * Text at a terminal cell. Runs of two or more spaces split the text into
 * separately placed pieces (SVG would collapse them), and every piece gets a
 * textLength of its column count, so columns line up in any monospace font.
 */
function put(col, row, text, cls) {
  const pieces = []
  const pattern = /\S+(?: \S+)*/g
  let match
  while ((match = pattern.exec(text))) {
    const piece = match[0]
    pieces.push(
      `<text x="${n(xOf(col + match.index))}" y="${n(yOf(row) + BASELINE)}" textLength="${n(
        piece.length * CW,
      )}" class="${cls}">${esc(piece)}</text>`,
    )
  }
  return pieces.join('')
}

/** Several coloured runs on one row: [[text, cls], ...], laid out left to right. */
function line(col, row, runs) {
  let at = col
  return runs
    .map(([text, cls]) => {
      const svg = put(at, row, text, cls)
      at += text.length
      return svg
    })
    .join('')
}

/** Greedy word wrap at `width` columns. */
function wrap(text, width) {
  const lines = []
  let current = ''
  for (const word of text.split(' ')) {
    if (current && current.length + 1 + word.length > width) {
      lines.push(current)
      current = word
    } else {
      current = current ? `${current} ${word}` : word
    }
  }
  if (current) lines.push(current)
  return lines
}

// ---------------------------------------------------------------- animation

const keyframes = new Map() // frames + static value -> class and keyframes name
const frameBodies = new Map() // name -> frames
const rules = []
const delays = new Map() // sheen delay classes, declared after the animation rules
const pct = (t) => `${Number(((t / LOOP) * 100).toFixed(3))}%`

function animation(property, points, finalValue, extra = '') {
  // points: [[time, value, timingFunctionForTheNextInterval?], ...], sorted.
  const frames = points.map(([t, value, timing]) =>
    `${pct(t)}{${property}:${value}${timing ? `;animation-timing-function:${timing}` : ''}}`,
  )
  const body = frames.join('')
  const key = `${body}|${finalValue}|${extra}`
  if (!keyframes.has(key)) {
    const name = `k${keyframes.size}`
    keyframes.set(key, name)
    frameBodies.set(name, body)
    rules.push(
      `.${name}{${property}:${finalValue};animation:${name} ${LOOP}s linear infinite${extra}}`,
    )
  }
  return keyframes.get(key)
}

const FADE = 0.1
/** Opacity 1 inside the [on, off] intervals, 0 outside; `final` is the static frame. */
function visible(intervals, final) {
  const points = [[0, intervals.some(([on]) => on <= 0) ? 1 : 0]]
  for (const [on, off] of intervals) {
    if (on > 0) points.push([on, 0], [Math.min(on + FADE, LOOP), 1])
    if (off < LOOP) points.push([off, 1], [Math.min(off + FADE, LOOP), 0])
  }
  const last = points[points.length - 1][1]
  points.push([LOOP, last])
  return animation('opacity', points, final ? 1 : 0)
}

const group = (cls, body, attrs = '') => `<g class="${cls}"${attrs}>${body}</g>`

/**
 * Text typed at a cell between `start` and `end`, one character per step: a
 * cover in the background colour shrinks toward its right edge, and a block
 * cursor walks along. `cursorFrom` and `cursorUntil` say when the cursor shows.
 */
function typed(col, row, text, start, end, cls, cursorFrom, cursorUntil) {
  const count = text.length
  const width = count * CW
  const cover = animation(
    'transform',
    [[0, 'scaleX(1)'], [start, 'scaleX(1)', `steps(${count},end)`], [end, 'scaleX(0)'], [LOOP, 'scaleX(0)']],
    'scaleX(0)',
    ';transform-box:fill-box;transform-origin:100% 50%',
  )
  const walk = animation(
    'transform',
    [
      [0, 'translateX(0px)'],
      [start, 'translateX(0px)', `steps(${count},end)`],
      [end, `translateX(${n(width)}px)`],
      [LOOP, `translateX(${n(width)}px)`],
    ],
    `translateX(${n(width)}px)`,
  )
  const cursor = group(
    visible([[cursorFrom, cursorUntil]], false),
    `<rect class="${walk}" x="${n(xOf(col))}" y="${n(yOf(row) + 2)}" width="${n(CW)}" height="${LH - 4}" fill="${C.fg}" opacity=".8"/>`,
  )
  return (
    put(col, row, text, cls) +
    `<rect class="${cover}" x="${n(xOf(col) - 0.5)}" y="${n(yOf(row))}" width="${n(width + 1)}" height="${LH}" fill="${C.card}"/>` +
    cursor
  )
}

// ---------------------------------------------------------------- timeline (s)

const T = {
  loginTyped: [0.4, 1.2],
  loginEnter: 1.35,
  codeShown: 1.5,
  signedIn: 4.4,
  prompt2: 4.6,
  neuronTyped: [4.9, 5.4],
  tui: 5.9,
  sheen: 6.0,
  taskTyped: [7.2, 9.4],
  taskFocus: 7.0,
  submit: 9.7,
  preamble: 10.3,
  read: 10.9,
  search: 11.5,
  edit: 12.2,
  create: 13.0,
  bash: 14.2,
  ask: 14.6,
  pressY: 18.3,
  approved: 18.8,
  out1: 18.95,
  out2: 19.2,
  out3: 19.7,
  answer1: 21.6,
  answer2: 21.8,
  footer: 22.2,
  fadeOut: 27.4,
}

// ---------------------------------------------------------------- shell phase

const PROMPT = [['~/shop-api', 'sh-p'], [' $ ', 'sh-d']]
const promptWidth = '~/shop-api $ '.length

const shell = [
  line(0, 0, PROMPT),
  typed(promptWidth, 0, 'neuron login', ...T.loginTyped, 'sh', 0, T.loginEnter),
  // TerminalObserver::show_code (device_login.rs), after its leading newline.
  group(
    visible([[T.codeShown, LOOP]], false),
    put(0, 2, 'Sign in to Zero-X to start Neuron.', 'sh') +
      put(0, 3, '  1. Open        https://neuron.zero-x.live/device', 'sh') +
      put(0, 4, '  2. Enter code  BDWP-HQPK', 'sh') +
      put(0, 5, '  The page is open in your browser; check that it shows the same code.', 'sh') +
      put(0, 6, 'Waiting for approval (the code expires in 10 min; Ctrl+C to cancel)...', 'sh'),
  ),
  // describe_login (auth.rs): 30-day session, macOS keychain.
  group(
    visible([[T.signedIn, LOOP]], false),
    put(
      0,
      7,
      'Signed in to Zero-X (plan free); session expires 2026-11-28 16:40 UTC. Login stored in the OS keychain.',
      'sh',
    ),
  ),
  group(
    visible([[T.prompt2, LOOP]], false),
    line(0, 8, PROMPT) + typed(promptWidth, 8, 'neuron', ...T.neuronTyped, 'sh', T.prompt2, T.tui - 0.1),
  ),
].join('')

// ---------------------------------------------------------------- chat content

// Rows of the scrolling chat (0 = top of the header). The header is
// chat-header.tsx: two rows of margin, the logo, then two lines of text.
const LOGO_ROW = 2
const LOGO_COL = 1

function blockLogo() {
  const rects = []
  const shadows = new Map() // `${step}|${color}` -> path data
  LOGO_LINES.forEach((text, r) => {
    const top = yOf(LOGO_ROW + r)
    // Runs of full blocks, split where the letter colour changes.
    let c = 0
    while (c < text.length) {
      if (text[c] !== '█') {
        c++
        continue
      }
      const color = letterColor(c)
      let end = c
      while (end < text.length && text[end] === '█' && letterColor(end) === color) end++
      rects.push(
        `<rect x="${n(xOf(LOGO_COL + c))}" y="${n(top)}" width="${n((end - c) * CW)}" height="${LH}" fill="${color}"/>`,
      )
      c = end
    }
    // Double-line box characters as strokes, grouped by when the sheen reaches them.
    ;[...text].forEach((ch, col) => {
      if (!'═║╔╗╚╝'.includes(ch)) return
      const left = xOf(LOGO_COL + col)
      const right = left + CW
      const cx = left + CW / 2
      const cy = top + LH / 2
      const bottom = top + LH
      const dx = 2.1
      const dy = 2.2
      const d = {
        '═': `M${n(left)} ${n(cy - dy)}H${n(right)}M${n(left)} ${n(cy + dy)}H${n(right)}`,
        '║': `M${n(cx - dx)} ${n(top)}V${n(bottom)}M${n(cx + dx)} ${n(top)}V${n(bottom)}`,
        '╗': `M${n(left)} ${n(cy - dy)}H${n(cx + dx)}V${n(bottom)}M${n(left)} ${n(cy + dy)}H${n(cx - dx)}V${n(bottom)}`,
        '╔': `M${n(right)} ${n(cy - dy)}H${n(cx - dx)}V${n(bottom)}M${n(right)} ${n(cy + dy)}H${n(cx + dx)}V${n(bottom)}`,
        '╚': `M${n(cx - dx)} ${n(top)}V${n(cy + dy)}H${n(right)}M${n(cx + dx)} ${n(top)}V${n(cy - dy)}H${n(right)}`,
        '╝': `M${n(cx + dx)} ${n(top)}V${n(cy + dy)}H${n(left)}M${n(cx - dx)} ${n(top)}V${n(cy - dy)}H${n(left)}`,
      }[ch]
      const step = Math.ceil(col / SHEEN_STEP)
      const key = `${step}|${letterColor(col)}`
      shadows.set(key, (shadows.get(key) ?? '') + d)
    })
  })

  // use-sheen-animation.tsx: the position advances SHEEN_STEP columns every
  // SHEEN_INTERVAL; a column is lit once the sheen reaches it. The forward
  // phase runs to column 100 on a terminal this wide, then the reverse phase
  // unlights the columns in the same order.
  const forward = (100 / SHEEN_STEP) * SHEEN_INTERVAL
  const on = T.sheen
  const off = T.sheen + forward
  const sheenRules = new Map() // color -> class
  const paths = []
  for (const [key, d] of shadows) {
    const [step, color] = key.split('|')
    if (!sheenRules.has(color)) {
      const lit = animation(
        'stroke',
        [[0, shadowOf(color)], [on, shadowOf(color)], [on + 0.01, color], [off, color], [off + 0.01, shadowOf(color)], [LOOP, shadowOf(color)]],
        shadowOf(color),
      )
      sheenRules.set(color, lit)
    }
    const delay = `d${step}`
    delays.set(delay, `.${delay}{animation-delay:${n(Number(step) * SHEEN_INTERVAL)}s}`)
    paths.push(`<path class="${sheenRules.get(color)} ${delay}" d="${d}"/>`)
  }
  return `${rects.join('')}<g fill="none" stroke-width="1.3">${paths.join('')}</g>`
}

const TASK = 'Add a test for expired tokens and fix whatever fails'
const ANSWER = wrap(
  'The expiry check compared with <= instead of <. I fixed it and added tests/expired_token.rs; all 14 auth tests pass.',
  COLS - 4,
)

/** A tool row as SimpleToolCallItem draws it: "• Name description". */
const tool = (row, name, description) =>
  line(1, row, [['• ', 'fg'], [name, 'fg b'], [` ${description}`, 'fg']])

const chatRows = {
  user: 13,
  preamble: 15,
  read: 17,
  search: 18,
  edit: 19,
  create: 22,
  bash: 23,
  out: 24,
  answer: 28,
}

const chat = [
  blockLogo(),
  put(1, 9, 'Neuron will run commands on your behalf to help you build.', 'fg'),
  line(1, 11, [['Directory ', 'fg'], ['~/shop-api', 'mu']]),
  group(
    visible([[T.submit, LOOP]], true),
    // message-with-agents.tsx: a user message has a bar in userLine (orange).
    `<rect x="${n(xOf(1))}" y="${n(yOf(chatRows.user))}" width="3" height="${LH}" fill="${C.orange}"/>` +
      put(3, chatRows.user, TASK, 'fg'),
  ),
  group(visible([[T.preamble, LOOP]], true), put(1, chatRows.preamble, "I'll check how sessions expire, then add the test.", 'fg')),
  group(visible([[T.read, LOOP]], true), tool(chatRows.read, 'Read', 'src/auth/session.rs')),
  group(visible([[T.search, LOOP]], true), tool(chatRows.search, 'Search', 'expires_at in src (3 results)')),
  group(
    visible([[T.edit, LOOP]], true),
    tool(chatRows.edit, 'Edit', 'src/auth/session.rs') +
      put(3, chatRows.edit + 1, '-    now <= self.expires_at', 'rm') +
      put(3, chatRows.edit + 2, '+    now < self.expires_at', 'add'),
  ),
  group(visible([[T.create, LOOP]], true), tool(chatRows.create, 'Create', 'tests/expired_token.rs')),
  // terminal-command-display.tsx: "$ " in the success colour, the command in bold.
  group(visible([[T.bash, LOOP]], true), line(1, chatRows.bash, [['$ ', 'ok'], ['cargo test auth', 'fg b']])),
  group(visible([[T.out1, LOOP]], true), put(1, chatRows.out, 'running 14 tests', 'mu')),
  group(visible([[T.out2, LOOP]], true), put(1, chatRows.out + 1, 'test auth::expired_token_is_rejected ... ok', 'mu')),
  group(
    visible([[T.out3, LOOP]], true),
    put(1, chatRows.out + 2, 'test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s', 'mu'),
  ),
  group(visible([[T.answer1, LOOP]], true), put(1, chatRows.answer, ANSWER[0], 'fg')),
  group(visible([[T.answer2, LOOP]], true), ANSWER.slice(1).map((text, i) => put(1, chatRows.answer + 1 + i, text, 'fg')).join('')),
].join('')

// The chat sticks to its bottom: rows scrolled off the top = content rows - view
// rows. The view ends at the input bar, or at the permission dialog while it
// takes the input bar's place.
const INPUT_TOP = 22
const reason = wrap(
  'Run `cargo test auth`? Commands can change files and run programs, beyond what workspace-write allows.',
  COLS - 4, // inside the dialog's border and padding
)
// Border, tool and mode, command, reason, session prefix, blank, keys, esc line, border.
const DIALOG_ROWS = 8 + reason.length
const BOX_TOP = INPUT_TOP + 2 - (DIALOG_ROWS - 1)
const offset = (contentRows, viewRows) => Math.max(0, contentRows - viewRows)
const SCROLL = 0.25
const scrollPoints = [[0, 0]]
for (const [t, rows] of [
  [T.create, offset(chatRows.create + 1, INPUT_TOP)],
  [T.bash, offset(chatRows.bash + 1, INPUT_TOP)],
  [T.ask, offset(chatRows.bash + 1, BOX_TOP)],
  [T.approved, offset(chatRows.out + 3, INPUT_TOP)],
  [T.answer1, offset(chatRows.answer + ANSWER.length, INPUT_TOP)],
]) {
  const previous = scrollPoints[scrollPoints.length - 1][1]
  scrollPoints.push([t, previous], [t + SCROLL, rows])
}
const FINAL_SCROLL = scrollPoints[scrollPoints.length - 1][1]
scrollPoints.push([LOOP, FINAL_SCROLL])
const scroll = animation(
  'transform',
  scrollPoints.map(([t, rows], i) => [t, `translateY(${-rows * LH}px)`, i % 2 === 1 ? 'ease-out' : undefined]),
  `translateY(${-FINAL_SCROLL * LH}px)`,
)

// ---------------------------------------------------------------- input, dialog, footer

const boxRect = (top, bottom, stroke, extra = '') =>
  `<rect x="${n(xOf(0) + CW / 2)}" y="${n(yOf(top) + LH / 2)}" width="${n((COLS - 1) * CW)}" height="${n(
    (bottom - top) * LH,
  )}" rx="6" fill="${C.card}" stroke="${stroke}" stroke-width="1.3"${extra}/>`

// chat-input-bar.tsx: a rounded box in the mode colour (foreground), placeholder from input-modes.ts.
const input = group(
  visible([[0, T.ask], [T.approved, LOOP]], true),
  boxRect(INPUT_TOP, INPUT_TOP + 2, C.fg, ' stroke-opacity=".45"') +
    group(visible([[0, T.taskFocus], [T.submit, LOOP]], true), put(2, INPUT_TOP + 1, 'enter a coding task or / for commands', 'ph')) +
    group(visible([[T.taskFocus, T.submit]], false), typed(2, INPUT_TOP + 1, TASK, ...T.taskTyped, 'fg', T.taskFocus, T.submit)),
)

// permission-prompt.tsx, for bash `cargo test auth` in workspace-write; the
// reason is permissions.rs escalation_reason() and the session prefix its
// approval_prefixes().
const title = ' Permission needed '
const titleCol = Math.floor((COLS - title.length) / 2)
const options = [
  ['y', 'allow once   '],
  ['a', 'allow for session   '],
  ['n', 'deny   '],
  ['e', 'deny with a note'],
]
let optionCol = 2
const optionRuns = []
let yCol = 2
for (const [key, label] of options) {
  if (key === 'y') yCol = optionCol
  optionRuns.push([key, 'key'], [` ${label}`, 'fg'])
  optionCol += 1 + 1 + label.length
}
const dialogRow = (i) => BOX_TOP + 1 + i
const dialogLines = [
  line(2, dialogRow(0), [['bash', 'fg b'], [' needs danger-full-access (now workspace-write)', 'mu']]),
  put(2, dialogRow(1), 'cargo test auth', 'fg'),
  ...reason.map((text, i) => put(2, dialogRow(2 + i), text, 'mu')),
  put(2, dialogRow(2 + reason.length), 'For the session: cargo test', 'mu'),
]
const optionsRow = dialogRow(3 + reason.length + 1)
const dialogBottom = optionsRow + 2
if (dialogBottom !== INPUT_TOP + 2) throw new Error(`the dialog should end on the input bar's last row (${dialogBottom})`)
const dialog = group(
  visible([[T.ask, T.approved]], false),
  boxRect(BOX_TOP, dialogBottom, C.orange) +
    `<rect x="${n(xOf(titleCol) - 2)}" y="${n(yOf(BOX_TOP) + 3)}" width="${n(title.length * CW + 4)}" height="${LH - 6}" fill="${C.card}"/>` +
    put(titleCol + 1, BOX_TOP, title.trim(), 'warn b') +
    dialogLines.join('') +
    line(2, optionsRow, optionRuns) +
    put(2, optionsRow + 1, 'esc deny · ctrl-c stop the turn', 'mu') +
    group(
      visible([[T.pressY, T.approved]], false),
      `<rect x="${n(xOf(yCol) - 3)}" y="${n(yOf(optionsRow) + 1)}" width="${n(CW + 6)}" height="${LH - 2}" rx="3" fill="${C.blue}"/>` +
        put(yCol, optionsRow, 'y', 'inv b'),
    ),
)

// engine-footer.tsx: model, mode, git, tokens, context, plan, joined with " · ".
const FOOTER_ROW = 25
const footer =
  group(
    visible([[0, T.footer]], false),
    line(1, FOOTER_ROW, [['auto', 'fg'], [' · workspace-write · main · ctx 0% · Zero-X free', 'mu']]),
  ) +
  group(
    visible([[T.footer, LOOP]], true),
    line(1, FOOTER_ROW, [['auto', 'fg'], [' · workspace-write · main ~1 ?1 · ↑18.4K ↓1.3K · ctx 7% · Zero-X free', 'mu']]),
  )

// ---------------------------------------------------------------- assemble

const shellLayer = animation(
  'opacity',
  [[0, 0], [0.25, 1], [T.tui, 1], [T.tui + 0.05, 0], [LOOP, 0]],
  0,
)
const tuiLayer = animation(
  'opacity',
  [[0, 0], [T.tui, 0], [T.tui + 0.05, 1], [T.fadeOut, 1], [LOOP, 0]],
  1,
)
const chatClip = `<clipPath id="chat"><rect x="${n(CARD.x)}" y="${n(Y0)}" width="${CARD.w}" height="${INPUT_TOP * LH}"/></clipPath>`
// Rows scrolled off the top fade out under the title bar instead of being cut.
const topFade =
  `<linearGradient id="fade" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${C.card}"/><stop offset="1" stop-color="${C.card}" stop-opacity="0"/></linearGradient>`

const FONT = "ui-monospace,'JetBrains Mono','Cascadia Mono',SFMono-Regular,Menlo,Consolas,'Liberation Mono',monospace"
const SANS = "-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif"

const body = [
  `<defs><filter id="shadow" x="-10%" y="-10%" width="120%" height="130%"><feGaussianBlur stdDeviation="7"/></filter>${chatClip}${topFade}</defs>`,
  `<rect x="${CARD.x + 6}" y="${CARD.y + 10}" width="${CARD.w - 12}" height="${CARD.h - 6}" rx="${CARD.r}" fill="#000" opacity=".28" filter="url(#shadow)"/>`,
  `<rect x="${CARD.x}" y="${CARD.y}" width="${CARD.w}" height="${CARD.h}" rx="${CARD.r}" fill="${C.card}" stroke="${C.edge}"/>`,
  `<path d="M${CARD.x + 0.5} ${CARD.y + TITLE_H}V${CARD.y + CARD.r}a${CARD.r - 0.5} ${CARD.r - 0.5} 0 0 1 ${CARD.r - 0.5} -${CARD.r - 0.5}H${CARD.x + CARD.w - CARD.r}a${CARD.r - 0.5} ${CARD.r - 0.5} 0 0 1 ${CARD.r - 0.5} ${CARD.r - 0.5}V${CARD.y + TITLE_H}Z" fill="${C.bar}"/>`,
  `<path d="M${CARD.x} ${CARD.y + TITLE_H}H${CARD.x + CARD.w}" stroke="${C.edge}"/>`,
  [0, 1, 2].map((i) => `<circle cx="${CARD.x + 22 + i * 20}" cy="${CARD.y + TITLE_H / 2}" r="6" fill="#3A3A44"/>`).join(''),
  `<text x="${W / 2}" y="${CARD.y + TITLE_H / 2 + 4.5}" text-anchor="middle" class="title">neuron — ~/shop-api</text>`,
  group(shellLayer, shell),
  group(
    tuiLayer,
    `<g clip-path="url(#chat)"><g class="${scroll}">${chat}</g></g>` +
      `<rect x="${CARD.x + 1}" y="${n(Y0)}" width="${CARD.w - 2}" height="${LH}" fill="url(#fade)"/>` +
      input +
      dialog +
      footer,
  ),
].join('\n')

const css = [
  `text{font-family:${FONT};font-size:14px}`,
  `.title{font-family:${SANS};font-size:12.5px;fill:${C.dim}}`,
  `.fg{fill:${C.fg}}.mu{fill:${C.muted}}.b{font-weight:700}`,
  `.sh{fill:${C.shell}}.sh-p{fill:${C.cyan}}.sh-d{fill:${C.dim}}`,
  `.ph{fill:${C.muted};opacity:.7}.ok{fill:${C.green}}.warn{fill:${C.orange}}`,
  `.key{fill:${C.blue};font-weight:700}.inv{fill:#fff}`,
  `.add{fill:${C.added}}.rm{fill:${C.removed}}`,
  ...[...frameBodies].map(([name, frames]) => `@keyframes ${name}{${frames}}`),
  ...rules,
  ...delays.values(),
  '@media (prefers-reduced-motion:reduce){*{animation:none!important}}',
].join('\n')

const description =
  'Animated illustration of a Neuron session: neuron login shows a device code, then the full-screen UI reads and edits a file, asks permission to run cargo test, and reports 14 passing tests.'

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" role="img" aria-labelledby="title desc">
<title id="title">Neuron session illustration</title>
<desc id="desc">${esc(description)}</desc>
<style>
${css}
</style>
${body}
</svg>
`

writeFileSync(out, svg)
console.log(`wrote ${out} (${svg.length} bytes, ${(svg.match(/<[a-z]/g) ?? []).length} elements)`)
