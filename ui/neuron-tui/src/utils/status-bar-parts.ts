/**
 * The prompt status bar under the input, as text: what it says, in which
 * tones, and how it gives way in a narrow terminal. Pure, so it is tested
 * without a renderer (components/prompt-status-bar.tsx draws it).
 *
 *   G Gemini 2.5 Flash ▾ │ ◑ think high │ ⌂ neuron-v2 ⎇ main +1 ~2 │ ✎ Edit workspace │ ●●●◑○○○○○○ 35% ctx      Free · 5h 12% · 37/100 req
 *
 * Segments, left to right: the model (its maker's monogram and its name,
 * never the route the gateway takes to it: utils/model-name.ts), the
 * thinking level, the workspace folder and git branch, the permission mode
 * and the context meter; on the right, the Zero-X plan and its usage. Each
 * segment opens its picker when clicked (`target`).
 *
 * When the line is too wide it gives way one step at a time
 * ({@link COLLAPSE_STEPS}): the account group shortens, labels go, then the
 * git counts, the folder name, half the meter's circles, the account group,
 * then the branch is cut, the thinking level and the circles go, and last the
 * model name is cut. The provider monogram, the permission mode and the
 * context percentage are never dropped.
 */

import stringWidth from 'string-width'

import { meterCells, meterTone, displayPercent } from '../components/context-meter'
import { needsSignIn } from '../state/engine-store'
import { contextUsage } from './context-breakdown'
import { cleanModelName, isAutoModel, modelMaker } from './model-name'
import { providerBadge } from './provider-badge'

import type { GlyphMode } from './glyphs'
import type {
  AccountDetails,
  ModelInfo,
  ModelsEvent,
  StatusEvent,
} from '../engine/protocol'

export type StatusTone =
  | 'foreground'
  | 'muted'
  | 'warning'
  | 'error'
  | 'primary'
  | 'success'
  | 'border'

/** What clicking a segment opens. */
export type StatusTarget =
  | 'model'
  | 'think'
  | 'workspace'
  | 'branch'
  | 'permission'
  | 'context'
  | 'account'

export interface StatusSpan {
  text: string
  tone: StatusTone
  bold?: boolean
  /** Drawn on the theme's surface colour (the provider monogram). */
  chip?: boolean
}

export interface StatusSegment {
  target: StatusTarget
  spans: StatusSpan[]
  /** Joined to the previous segment by a space, not a separator (folder, then branch). */
  glue?: boolean
}

export interface StatusLine {
  left: StatusSegment[]
  /** Right-aligned: the account group. */
  right: StatusSegment[]
}

export const SEGMENT_SEPARATOR = ' │ '
/** The fewest spaces between the left segments and the account group. */
export const RIGHT_GAP = 2
/** A branch name is not cut below this, nor a model name below {@link MODEL_MIN_COLUMNS}. */
export const BRANCH_MIN_COLUMNS = 8
export const MODEL_MIN_COLUMNS = 12
const ELLIPSIS = '…'

/** Levels the engine accepts from engines that report none per model. */
export const DEFAULT_REASONING_EFFORTS = ['low', 'medium', 'high'] as const

/** Levels shown in the primary colour: the most a model can think. */
const STRONG_EFFORTS = new Set(['xhigh', 'max', 'ultra'])
/** Levels that turn thinking off. */
const OFF_EFFORTS = new Set(['none', 'off', 'disabled'])

interface Glyphs {
  caret: string
  house: string
  branch: string
  untrusted: string
  readOnly: string
  edit: string
  plan: string
  fullAccess: string
  thinkOff: string
  thinkLevels: readonly string[]
  thinkDefault: string
  /** Between the permission mode and the one coming next. */
  arrow: string
}

const UNICODE: Glyphs = {
  caret: '▾',
  house: '⌂',
  branch: '⎇',
  untrusted: '⚠',
  readOnly: '◌',
  edit: '✎',
  plan: '◇',
  fullAccess: '!',
  thinkOff: '○',
  thinkLevels: ['◔', '◑', '◕', '●'],
  thinkDefault: '◑',
  arrow: '→',
}

/** What the classic console fonts (WGL4/CP437) can draw. */
const CONSOLE: Glyphs = {
  caret: '▼',
  house: '⌂',
  branch: '±',
  untrusted: '!',
  readOnly: '-',
  edit: '+',
  plan: '*',
  fullAccess: '!',
  thinkOff: '○',
  thinkLevels: ['●', '●', '●', '●'],
  thinkDefault: '●',
  arrow: '>',
}

export function statusGlyphs(mode: GlyphMode): Glyphs {
  return mode === 'console' ? CONSOLE : UNICODE
}

// ------------------------------------------------------------------ the view

export interface StatusBarInput {
  status: StatusEvent | null
  account: AccountDetails | null
  models: ModelsEvent | null
  mode: GlyphMode
  /** A permission choice sent that no status shows yet (engine-store's `pendingPermission`). */
  pendingPermission?: string | null
}

/** One segment's possible texts, before they are fitted. */
export interface StatusView {
  model: {
    badge: string
    badgeTone: StatusTone
    name: string
    /** The picker has a catalog to show. */
    pickable: boolean
    /** The plan cannot use the current model any more. */
    locked: boolean
  }
  think: { glyph: string; level: string; tone: StatusTone } | null
  workspace: {
    folder: string | null
    branch: string | null
    counts: string
    untrusted: boolean
  }
  permission: { glyph: string; long: string; short: string; tone: StatusTone; bold: boolean }
  /** The permission choice coming next (sent, not in effect yet): `✎ Edit → ◇ Plan`. */
  permissionNext: StatusView['permission'] | null
  context: { percent: number; estimated: boolean; thresholdPercent: number } | null
  account:
    | { kind: 'usage'; plan: string; window: number | null; requests: string | null }
    | { kind: 'signed-out' }
    | null
  glyphs: Glyphs
  mode: GlyphMode
}

/** The engine names the model with the provider's `openai/` prefix the catalog lacks. */
function isCurrentModel(id: string, current: string): boolean {
  return id === current || current === `openai/${id}` || `openai/${current}` === id
}

export function catalogModel(models: ModelsEvent | null, id: string): ModelInfo | null {
  if (!models) return null
  return models.models.find((model) => isCurrentModel(model.id, id)) ?? null
}

/** "free" → "Free". */
function titleCase(text: string): string {
  return text.length > 0 ? text[0]!.toUpperCase() + text.slice(1) : text
}

/** The folder's name, from a Windows or POSIX path. */
export function folderName(cwd: string): string | null {
  const parts = cwd.split(/[\\/]+/).filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1]! : null
}

/** Staged, unstaged, untracked and conflicted counts that are not zero: `+1 ~2 ?3 !1`. */
export function gitCounts(git: NonNullable<StatusEvent['git']>): string {
  const counts = [
    ['+', git.staged],
    ['~', git.unstaged],
    ['?', git.untracked],
    ['!', git.conflicted],
  ] as const
  return counts
    .filter(([, count]) => count > 0)
    .map(([mark, count]) => `${mark}${count}`)
    .join(' ')
}

/** The thinking levels of the current model, or `null` when it does not think. */
export function reasoningEffortsOf(
  status: StatusEvent,
  model: ModelInfo | null,
): string[] | null {
  if (model?.supports_reasoning === false) return null
  const efforts =
    status.reasoning_efforts ??
    model?.reasoning_efforts ??
    (model?.supports_reasoning ? [...DEFAULT_REASONING_EFFORTS] : null)
  if (efforts && efforts.length > 0) return efforts
  // An engine that knows the level but not the list still shows it.
  if (status.reasoning_effort && efforts === null) return [status.reasoning_effort]
  return null
}

/** The level as the bar names it: `default` for the model's own, `off` for none. */
export function effortLabel(effort: string | null | undefined): string {
  if (!effort) return 'default'
  return OFF_EFFORTS.has(effort) ? 'off' : effort
}

function thinkGlyph(effort: string | null | undefined, efforts: string[], glyphs: Glyphs): string {
  if (!effort) return glyphs.thinkDefault
  if (OFF_EFFORTS.has(effort)) return glyphs.thinkOff
  const ladder = efforts.filter((level) => !OFF_EFFORTS.has(level))
  const index = ladder.indexOf(effort)
  if (index < 0 || ladder.length === 0) return glyphs.thinkDefault
  const step = Math.ceil(((index + 1) / ladder.length) * glyphs.thinkLevels.length) - 1
  return glyphs.thinkLevels[Math.max(0, Math.min(glyphs.thinkLevels.length - 1, step))]!
}

/** The permission segment for a mode: plan mode reads `Plan` whatever the mode under it. */
export function permissionView(
  mode: string,
  planMode: boolean,
  glyphs: Glyphs,
): StatusView['permission'] {
  if (planMode) {
    return { glyph: glyphs.plan, long: 'Plan', short: 'Plan', tone: 'primary', bold: false }
  }
  switch (mode) {
    case 'read-only':
      return { glyph: glyphs.readOnly, long: 'Read-only', short: 'Read-only', tone: 'muted', bold: false }
    case 'workspace-write':
      return { glyph: glyphs.edit, long: 'Edit workspace', short: 'Edit', tone: 'foreground', bold: false }
    case 'danger-full-access':
      return { glyph: glyphs.fullAccess, long: 'Full access', short: 'Full access', tone: 'error', bold: true }
    default:
      return { glyph: '', long: mode, short: mode, tone: 'muted', bold: false }
  }
}

/** The 5-hour window's used share (0–100), when the account reports it. */
export function usageWindowPercent(account: AccountDetails | null): number | null {
  const window = account?.usage_window
  if (!window) return null
  const limit = window.limit_tokens
  if (!limit || limit <= 0) return null
  const used =
    window.tokens ?? (window.remaining_tokens !== null && window.remaining_tokens !== undefined
      ? limit - window.remaining_tokens
      : null)
  if (used === null || !Number.isFinite(used)) return null
  return Math.max(0, Math.min(100, (used / limit) * 100))
}

/** A usable Zero-X login: signed in, and the engine does not wait for a sign-in. */
export function hasUsableZeroXLogin(status: StatusEvent): boolean {
  return status.signed_in === true && !needsSignIn(status)
}

/** Everything the bar can say, before it is fitted to a width. */
export function statusView({
  status,
  account,
  models,
  mode,
  pendingPermission = null,
}: StatusBarInput): StatusView | null {
  if (!status) return null
  const glyphs = statusGlyphs(mode)
  const model = catalogModel(models, status.model)
  const provider = status.provider ?? model?.provider ?? null
  const name = cleanModelName(status.model, status.model_display_name ?? model?.display_name, provider)
  const served = status.served_model
  const servedModel = served ? catalogModel(models, served) : null
  const servedName =
    served && served !== status.model
      ? cleanModelName(
          served,
          status.served_model_display_name ?? servedModel?.display_name,
          servedModel?.provider,
        )
      : null
  // `auto` is Zero-X's router: its mark, whichever model answered.
  const maker = isAutoModel(status.model) ? 'zero-x' : modelMaker(status.model, provider)
  const health = model?.status
  const efforts = reasoningEffortsOf(status, model)
  const usage = contextUsage(status)

  let accountView: StatusView['account'] = null
  if (hasUsableZeroXLogin(status)) {
    const planId = account?.plan ?? status.account?.plan ?? null
    const plan = account?.plan_name ?? (planId ? titleCase(planId) : 'Zero-X')
    const used = account?.usage_today?.requests
    const limit = account?.limits?.requests
    accountView = {
      kind: 'usage',
      plan,
      window: usageWindowPercent(account),
      requests:
        limit !== null && limit !== undefined && limit > 0
          ? `${used ?? 0}/${limit} req`
          : null,
    }
  } else if (needsSignIn(status) || status.account) {
    accountView = { kind: 'signed-out' }
  }

  return {
    model: {
      badge: providerBadge(maker, mode),
      badgeTone: health === 'degraded' || health === 'busy' ? 'warning' : 'primary',
      name: servedName ? `${name} → ${servedName}` : name,
      pickable: (models?.models.length ?? 0) > 0,
      locked: model !== null && model.available === false,
    },
    think: efforts
      ? {
          glyph: thinkGlyph(status.reasoning_effort, efforts, glyphs),
          level: effortLabel(status.reasoning_effort),
          tone: status.reasoning_effort && STRONG_EFFORTS.has(status.reasoning_effort) ? 'primary' : 'muted',
        }
      : null,
    workspace: {
      folder: folderName(status.cwd),
      branch: status.git ? (status.git.branch ?? 'detached') : null,
      counts: status.git ? gitCounts(status.git) : '',
      untrusted: !status.workspace_trusted,
    },
    permission: permissionView(status.permission_mode, status.plan_mode, glyphs),
    permissionNext:
      pendingPermission && pendingPermission !== (status.plan_mode ? 'plan' : status.permission_mode)
        ? permissionView(pendingPermission, pendingPermission === 'plan', glyphs)
        : null,
    context: usage
      ? { percent: usage.percent, estimated: usage.estimated, thresholdPercent: usage.thresholdPercent }
      : null,
    account: accountView,
    glyphs,
    mode,
  }
}

// ---------------------------------------------------------------- the layout

/** How far the line has given way. */
export interface Compaction {
  accountShort: boolean
  accountShown: boolean
  /** The signed-out warning stays longer than the usage. */
  warningShown: boolean
  labels: boolean
  gitCounts: boolean
  folder: boolean
  cells: 0 | 5 | 10
  think: boolean
  /** The branch cut to this many columns, with "…". */
  branchMax: number | null
  /** The model name cut to this many columns, with "…". */
  modelMax: number | null
  /** Last resorts in a very narrow terminal: the branch, then the model's name, go. */
  branch: boolean
  modelName: boolean
}

export const FULL: Compaction = {
  accountShort: false,
  accountShown: true,
  warningShown: true,
  labels: true,
  gitCounts: true,
  folder: true,
  cells: 10,
  think: true,
  branchMax: null,
  modelMax: null,
  branch: true,
  modelName: true,
}

type Step = Partial<Compaction> | 'cut-branch' | 'cut-model'

/** What gives way first, in order. */
export const COLLAPSE_STEPS: readonly Step[] = [
  { accountShort: true },
  { labels: false },
  { gitCounts: false },
  { folder: false },
  { cells: 5 },
  { accountShown: false },
  'cut-branch',
  { think: false },
  { cells: 0, warningShown: false },
  'cut-model',
  { branch: false },
  { modelName: false },
]

/** `text` in `columns` columns, ending in "…" when it was cut. */
export function cutToWidth(text: string, columns: number): string {
  if (columns <= 0) return ''
  if (stringWidth(text) <= columns) return text
  let kept = ''
  for (const char of text) {
    if (stringWidth(kept + char) > columns - 1) break
    kept += char
  }
  return kept + ELLIPSIS
}

/** The segments for `view` at compaction `c`. */
export function buildLine(view: StatusView, c: Compaction): StatusLine {
  const { glyphs } = view
  const left: StatusSegment[] = []

  // The model: monogram, name, and the picker's caret.
  const modelName = c.modelMax !== null ? cutToWidth(view.model.name, c.modelMax) : view.model.name
  const modelSpans: StatusSpan[] = [
    { text: view.model.badge, tone: view.model.badgeTone, bold: true, chip: true },
  ]
  if (c.modelName) modelSpans.push({ text: ` ${modelName}`, tone: 'foreground' })
  if (view.model.locked) modelSpans.push({ text: ' (locked)', tone: 'warning' })
  if (view.model.pickable && c.labels) modelSpans.push({ text: ` ${glyphs.caret}`, tone: 'muted' })
  left.push({ target: 'model', spans: modelSpans })

  if (view.think && c.think) {
    left.push({
      target: 'think',
      spans: [
        { text: `${view.think.glyph} `, tone: view.think.tone },
        { text: c.labels ? `think ${view.think.level}` : view.think.level, tone: view.think.tone },
      ],
    })
  }

  const ws = view.workspace
  let glueBranch = false
  if (ws.folder && c.folder) {
    left.push({ target: 'workspace', spans: [{ text: `${glyphs.house} ${ws.folder}`, tone: 'muted' }] })
    glueBranch = true
  }
  if (ws.branch !== null && c.branch) {
    const branch = c.branchMax !== null ? cutToWidth(ws.branch, c.branchMax) : ws.branch
    const spans: StatusSpan[] = [{ text: `${glyphs.branch} ${branch}`, tone: 'muted' }]
    if (ws.untrusted) {
      spans.push({ text: c.gitCounts ? ` ${glyphs.untrusted} untrusted` : ` ${glyphs.untrusted}`, tone: 'warning' })
    } else if (c.gitCounts && ws.counts) {
      spans.push({ text: ` ${ws.counts}`, tone: 'muted' })
    }
    left.push({ target: 'branch', spans, glue: glueBranch })
  } else if (ws.untrusted && ws.branch === null) {
    // No git: the folder carries the warning (or stands for it once the folder is gone).
    const warning = { text: `${glyphs.untrusted} untrusted`, tone: 'warning' as const }
    const last = left[left.length - 1]
    if (glueBranch && last?.target === 'workspace') last.spans.push({ ...warning, text: ` ${warning.text}` })
    else left.push({ target: 'workspace', spans: [warning] })
  }

  const p = view.permission
  const label = c.labels ? p.long : p.short
  const permissionSpans: StatusSpan[] = [
    { text: p.glyph ? `${p.glyph} ${label}` : label, tone: p.tone, bold: p.bold },
  ]
  const next = view.permissionNext
  if (next) {
    // Sent, not in effect yet (the engine answers after the running turn).
    const nextLabel = c.labels ? next.long : next.short
    permissionSpans.push(
      { text: ` ${view.glyphs.arrow} `, tone: 'muted' },
      { text: next.glyph ? `${next.glyph} ${nextLabel}` : nextLabel, tone: next.tone, bold: next.bold },
    )
    if (c.labels) permissionSpans.push({ text: ' next', tone: 'muted' })
  }
  left.push({ target: 'permission', spans: permissionSpans })

  if (view.context) {
    const ctx = view.context
    const tone = meterTone(ctx.percent)
    const spans: StatusSpan[] = meterCells(ctx.percent, c.cells, view.mode, ctx.thresholdPercent).map(
      (cell) => ({ text: cell.char, tone: cell.tone }),
    )
    const pct = `${ctx.estimated ? '~' : ''}${displayPercent(ctx.percent)}%`
    spans.push({ text: `${spans.length > 0 ? ' ' : ''}${pct}${c.labels ? ' ctx' : ''}`, tone })
    left.push({ target: 'context', spans })
  }

  const right: StatusSegment[] = []
  const account = view.account
  if (account?.kind === 'signed-out' && c.warningShown) {
    right.push({ target: 'account', spans: [{ text: 'Signed out: /login', tone: 'warning' }] })
  } else if (account?.kind === 'usage' && c.accountShown) {
    const windowSpan: StatusSpan | null =
      account.window !== null
        ? { text: `5h ${displayPercent(account.window)}%`, tone: windowTone(account.window) }
        : null
    const spans: StatusSpan[] = []
    if (c.accountShort) {
      if (windowSpan) spans.push(windowSpan)
      else if (account.requests) spans.push({ text: account.requests, tone: 'muted' })
      else spans.push({ text: account.plan, tone: 'muted' })
    } else {
      spans.push({ text: account.plan, tone: 'muted' })
      if (windowSpan) spans.push({ text: ' · ', tone: 'muted' }, windowSpan)
      if (account.requests) spans.push({ text: ` · ${account.requests}`, tone: 'muted' })
    }
    right.push({ target: 'account', spans })
  }
  return { left, right }
}

function windowTone(percent: number): StatusTone {
  if (percent >= 95) return 'error'
  if (percent >= 80) return 'warning'
  return 'muted'
}

function segmentsWidth(segments: StatusSegment[]): number {
  let width = 0
  segments.forEach((segment, index) => {
    if (index > 0) width += segment.glue ? 1 : SEGMENT_SEPARATOR.length
    for (const span of segment.spans) width += stringWidth(span.text)
  })
  return width
}

/** The columns the line needs, with the account group's gap. */
export function lineWidth(line: StatusLine): number {
  const left = segmentsWidth(line.left)
  const right = segmentsWidth(line.right)
  return right > 0 ? left + RIGHT_GAP + right : left
}

function spanText(segment: StatusSegment | undefined): string {
  return segment ? segment.spans.map((span) => span.text).join('') : ''
}

/**
 * The line for `width` columns: the fullest one that fits, giving way step
 * by step ({@link COLLAPSE_STEPS}). A line that still does not fit after the
 * last step is returned as it is; the renderer clips it.
 */
export function fitStatusLine(view: StatusView, width: number): StatusLine {
  let compaction: Compaction = { ...FULL }
  let line = buildLine(view, compaction)
  if (lineWidth(line) <= width) return line
  for (const step of COLLAPSE_STEPS) {
    if (step === 'cut-branch' || step === 'cut-model') {
      const over = lineWidth(line) - width
      if (step === 'cut-branch' && view.workspace.branch !== null) {
        const full = stringWidth(view.workspace.branch)
        if (full > BRANCH_MIN_COLUMNS) {
          compaction = { ...compaction, branchMax: Math.max(BRANCH_MIN_COLUMNS, full - over) }
        }
      }
      if (step === 'cut-model') {
        const full = stringWidth(view.model.name)
        if (full > MODEL_MIN_COLUMNS) {
          compaction = { ...compaction, modelMax: Math.max(MODEL_MIN_COLUMNS, full - over) }
        }
      }
    } else {
      compaction = { ...compaction, ...step }
    }
    line = buildLine(view, compaction)
    if (lineWidth(line) <= width) return regrowCuts(view, compaction, width, line)
  }
  return line
}

/**
 * A later step can free more room than the cut before it needed: give the
 * cut branch (then the model name) back what still fits.
 */
function regrowCuts(view: StatusView, compaction: Compaction, width: number, line: StatusLine): StatusLine {
  let current = compaction
  let best = line
  for (const key of ['branchMax', 'modelMax'] as const) {
    const cut = current[key]
    if (cut === null) continue
    const full = stringWidth(key === 'branchMax' ? (view.workspace.branch ?? '') : view.model.name)
    const spare = width - lineWidth(best)
    if (spare <= 0) continue
    const grown = Math.min(full, cut + spare)
    current = { ...current, [key]: grown >= full ? null : grown }
    const candidate = buildLine(view, current)
    if (lineWidth(candidate) <= width) best = candidate
    else current = { ...current, [key]: cut }
  }
  return best
}

/**
 * The line as plain text in `width` columns, the account group right-aligned:
 * for tests and the snapshot of what a terminal shows.
 */
export function statusLineText(line: StatusLine, width: number): string {
  const left = line.left
    .map((segment, index) => (index > 0 ? (segment.glue ? ' ' : SEGMENT_SEPARATOR) : '') + spanText(segment))
    .join('')
  const right = line.right.map(spanText).join(SEGMENT_SEPARATOR)
  if (!right) return left
  const gap = Math.max(RIGHT_GAP, width - stringWidth(left) - stringWidth(right))
  return left + ' '.repeat(gap) + right
}

/** The bar for these inputs in `width` columns, or `null` before the engine's first status. */
export function statusBarLine(input: StatusBarInput, width: number): StatusLine | null {
  const view = statusView(input)
  return view ? fitStatusLine(view, width) : null
}
